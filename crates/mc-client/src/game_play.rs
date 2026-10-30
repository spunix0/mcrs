//! The player side of [`Game`]: interaction (mining, attacking, using), per-tick view state, camera, overlay
//! geometry, HUD and container screens. A child module of `game` so it can use the session's fields directly.

use super::*;

impl Game {
    pub(super) fn set_game_mode(&mut self, m: u8) {
        self.game_mode = m;
        if m == 3 {
            if self.mode == Mode::Walk {
                self.mode = Mode::Fly;
            }
        } else {
            self.walk_mode = m;
            if self.mode == Mode::Fly {
                self.body = Body::new(self.feet());
                self.prev_feet = self.body.pos;
                self.mode = Mode::Walk;
            }
        }
    }

    pub(super) fn blocks_changed(&mut self, changes: Vec<([i32; 3], u16)>) {
        for ([x, y, z], state) in changes {
            let (cx, cz) = (x.div_euclid(16), z.div_euclid(16));
            let Some(chunk) = self.world.get_mut(&(cx, cz)) else { continue };
            let (lx, lz) = (x.rem_euclid(16), z.rem_euclid(16));
            Arc::make_mut(chunk).set(lx as usize, y, lz as usize, state);
            let si = y.div_euclid(16) - chunk.min_section;
            let placed = self.pending_place.is_some_and(|(p, t)| p == [x, y, z] && t.elapsed() < Duration::from_secs(1));
            if placed && state != 0 {
                self.pending_place = None;
                self.block_sound([x, y, z], Some(state), |s| &s.place, |v| (v + 1.0) / 2.0, 0.8);
            }
            // A block affects meshes of every section whose 1-block border it sits in.
            let ly = y.rem_euclid(16);
            let mut sections = 0u64;
            for s in [si - 1, si, si + 1] {
                let near = s == si || (s < si && ly == 0) || (s > si && ly == 15);
                if near && (0..64).contains(&s) {
                    sections |= 1 << s;
                }
            }
            let xs = [-1, 0, 1].into_iter().filter(|d| *d == 0 || (*d == -1 && lx == 0) || (*d == 1 && lx == 15));
            for dx in xs {
                for dz in [-1, 0, 1] {
                    if dz == 0 || (dz == -1 && lz == 0) || (dz == 1 && lz == 15) {
                        self.mark(cx + dx, cz + dz, sections);
                    }
                }
            }
        }
    }

    /// Plays one of a block's sounds (break/hit/place/step) at its center, with vanilla's volume and pitch for that use.
    pub(super) fn block_sound(&mut self, pos: [i32; 3], state: Option<u16>, which: fn(&mc_assets::SoundSet) -> &str, volume: fn(f32) -> f32, pitch: f32) {
        let id = state.unwrap_or_else(|| self.state_at(pos));
        let state = self.models.states.get(id as usize);
        let Some(set) = state.and_then(|s| self.models.sound_types.get(&*s.sound)).cloned() else { return };
        let at = [pos[0] as f64 + 0.5, pos[1] as f64 + 0.5, pos[2] as f64 + 0.5];
        self.sound(which(&set), Some(at), volume(set.volume), set.pitch * pitch, category::BLOCKS);
    }

    /// The texture rectangle a block's particles use (its first face) and the tint.
    fn particle_texture(&self, state: u16) -> Option<([f32; 4], [u8; 3])> {
        let s = self.models.states.get(state as usize)?;
        let q = s.quads.first()?;
        let (mut u0, mut v0, mut u1, mut v1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for [u, v] in q.uv {
            (u0, v0, u1, v1) = (u0.min(u), v0.min(v), u1.max(u), v1.max(v));
        }
        Some(([u0, v0, u1, v1], if q.tinted { s.tint } else { [255; 3] }))
    }

    pub(super) fn break_particles(&mut self, pos: [i32; 3], state: u16) {
        if let Some((rect, tint)) = self.particle_texture(state) {
            self.particles.destroy(pos, rect, tint);
        }
    }

    fn state_at(&self, pos: [i32; 3]) -> u16 {
        self.world.get(&(pos[0].div_euclid(16), pos[2].div_euclid(16))).map_or(0, |c| c.get(pos[0].rem_euclid(16) as usize, pos[1], pos[2].rem_euclid(16) as usize))
    }

    /// Switches walking (survival, physics) and flying (spectator, noclip).
    pub fn toggle_mode(&mut self) {
        if !self.noclip {
            return;
        }
        self.mode = match self.mode {
            Mode::Walk => Mode::Fly,
            Mode::Fly => {
                self.body = Body::new(self.feet());
                self.prev_feet = self.body.pos;
                Mode::Walk
            }
        };
        self.request_gamemode();
    }

    pub(super) fn request_gamemode(&self) {
        // Only spectators may pass through blocks; needs operator rights (else the server just says no).
        if self.noclip && self.in_world && (self.mode == Mode::Fly) != (self.game_mode == 3) {
            let back = ["survival", "creative", "adventure", "survival"][self.walk_mode.min(3) as usize];
            let cmd = if self.mode == Mode::Fly { "/gamemode spectator".to_string() } else { format!("/gamemode {back}") };
            let _ = session::send_chat(&self.tx, &cmd);
        }
    }

    pub fn feet(&self) -> [f64; 3] {
        let [x, y, z] = self.cam.eye;
        [x, y - EYE_HEIGHT, z]
    }

    /// Jump was just pressed: a second press within [`DOUBLE_TAP`] toggles flying.
    pub fn jump_pressed(&mut self) {
        // Swimmers tap jump; that shouldn't toggle flying.
        if self.mode == Mode::Walk && self.body.in_fluid(&self.view()) {
            return;
        }
        match self.last_jump_press {
            Some(t) if t.elapsed() < DOUBLE_TAP => {
                self.last_jump_press = None;
                self.toggle_mode();
            }
            _ => self.last_jump_press = Some(Instant::now()),
        }
    }

    pub fn select_slot(&mut self, slot: u8) {
        self.slot = slot % 9;
        let _ = session::send_hotbar_slot(&self.tx, self.slot);
    }

    fn next_sequence(&mut self) -> i32 {
        self.sequence += 1;
        self.sequence
    }

    /// The creative search box has the keyboard (so E types instead of closing).
    pub fn typing_in_screen(&self) -> bool {
        self.creative.as_ref().is_some_and(|c| self.creative_tabs.get(c.tab).is_some_and(|t| t.kind == crate::creative::Kind::Search))
    }

    pub fn screen_open(&self) -> bool {
        self.inventory_open || self.menu.is_some() || self.creative.is_some()
    }

    /// Opens or closes the inventory (or closes an open container), telling the server like vanilla.
    pub fn toggle_inventory(&mut self) {
        if let Some(c) = self.creative.take() {
            // Closing with a stack in hand drops it, like vanilla.
            self.creative_tab = c.tab;
            if let Some(s) = c.carried {
                let _ = session::send_creative_slot(&self.tx, &self.models.components, -1, Some(&s));
            }
            return;
        }
        if self.game_mode == 1 && self.menu.is_none() {
            if self.creative_tabs.is_empty() {
                self.creative_tabs = crate::creative::tabs(&self.models);
            }
            self.creative = Some(crate::creative::Screen { tab: self.creative_tab, ..crate::creative::Screen::default() });
            return;
        }
        if let Some(m) = self.menu.take() {
            let _ = self.tx.send(mc_protocol::ids::play::c2s::CONTAINER_CLOSE, &mc_protocol::Writer::new().varint(m.id).buf);
            self.cursor = None;
            return;
        }
        self.inventory_open = !self.inventory_open;
        if !self.inventory_open {
            let _ = self.tx.send(mc_protocol::ids::play::c2s::CONTAINER_CLOSE, &mc_protocol::Writer::new().varint(0).buf);
            self.cursor = None;
        }
    }

    pub fn close_screens(&mut self) {
        if self.screen_open() {
            self.toggle_inventory();
        }
        self.inventory_open = false;
    }

    pub fn drop_item(&mut self, whole: bool) {
        let _ = session::send_drop(&self.tx, whole);
        self.start_swing();
    }

    pub fn pick_block(&mut self) {
        if let Some(t) = &self.target {
            let _ = session::send_pick_block(&self.tx, t.pos);
        }
    }

    fn held(&self) -> Option<&Stack> {
        self.inventory.hotbar(self.slot as usize)
    }

    fn efficiency(&self, s: &Stack) -> u32 {
        s.enchantments
            .iter()
            .find(|(id, _)| self.enchantments.get(*id as usize).is_some_and(|n| n.ends_with("efficiency")))
            .map_or(0, |(_, l)| *l)
    }

    fn attack_delay(&self) -> f32 {
        let name = self.held().and_then(|s| self.models.items.get(s.item as usize)).map_or("", |i| i.name.as_str());
        20.0 / attack_speed(name)
    }

    /// `Player.getAttackStrengthScale`.
    pub fn attack_strength(&self, a: f32) -> f32 {
        ((self.attack_ticker as f32 + a) / self.attack_delay()).clamp(0.0, 1.0)
    }

    fn reset_attack_strength(&mut self) {
        self.attack_ticker = 0;
        self.swap_ticker = 0;
    }

    fn start_swing(&mut self) {
        if self.swing.start() {
            // Other players see the swing through the punch packet.
            let _ = session::send_punch(&self.tx);
        }
    }

    /// One tick of mining and using, run with the physics. Mirrors vanilla's `continueDestroyBlock` and `startUseItem`.
    fn interact_tick(&mut self, c: &Controls, attack_pressed: bool) {
        self.mine_cooldown = self.mine_cooldown.saturating_sub(1);
        self.use_cooldown = self.use_cooldown.saturating_sub(1);
        let target = self.target.as_ref().map(|t| (t.pos, t.face, t.point));

        if attack_pressed {
            if let Some(id) = self.entity_target {
                let _ = self.tx.send(mc_protocol::ids::play::c2s::ATTACK, &mc_protocol::Writer::new().varint(id).buf);
                self.reset_attack_strength();
                self.start_swing();
                if let Some(m) = self.mining.take() {
                    let seq = self.next_sequence();
                    let _ = session::send_dig(&self.tx, Dig::Abort, m.pos, m.face, seq);
                }
                return;
            }
            if target.is_none() {
                // A miss: vanilla swings and resets the attack strength.
                self.reset_attack_strength();
                self.start_swing();
            }
        }
        if !c.mine || self.entity_target.is_some() {
            if let Some(m) = self.mining.take() {
                let seq = self.next_sequence();
                let _ = session::send_dig(&self.tx, Dig::Abort, m.pos, m.face, seq);
                self.reset_attack_strength();
            }
        } else if self.mine_cooldown == 0 {
            // (position, broke it?) — sounds are played after the match to keep the borrow of `mining` short.
            let mut sound: Option<([i32; 3], bool)> = None;
            let mut chip: Option<([i32; 3], u8)> = None;
            let creative = self.game_mode == 1;
            match (target, &mut self.mining) {
                (Some((pos, _, _)), Some(m)) if m.pos == pos => {
                    m.ticks += 1;
                    if m.ticks >= m.needed {
                        let (pos, face) = (m.pos, m.face);
                        self.mining = None;
                        let seq = self.next_sequence();
                        let _ = session::send_dig(&self.tx, Dig::Stop, pos, face, seq);
                        self.mine_cooldown = MINE_COOLDOWN;
                        sound = Some((pos, true));
                    } else {
                        if m.ticks % 4 == 0 {
                            sound = Some((pos, false));
                        }
                        chip = Some((pos, m.face));
                    }
                    self.start_swing();
                }
                (Some((pos, face, _)), _) => {
                    if let Some(m) = self.mining.take() {
                        let seq = self.next_sequence();
                        let _ = session::send_dig(&self.tx, Dig::Abort, m.pos, m.face, seq);
                    }
                    let held = self.held().cloned();
                    let tool = held.as_ref().and_then(|s| self.models.items.get(s.item as usize)).and_then(|i| i.tool);
                    let eff = held.as_ref().map_or(0, |s| self.efficiency(s));
                    let needed = if creative { Some(0) } else { self.view().state(pos[0], pos[1], pos[2]).and_then(|s| s.break_ticks(tool.as_ref(), self.body.on_ground, eff)) };
                    if let Some(needed) = needed {
                        let seq = self.next_sequence();
                        let _ = session::send_dig(&self.tx, Dig::Start, pos, face, seq);
                        if needed <= 1 {
                            // Breaks instantly (grass, flowers, torches, anything in creative).
                            self.mine_cooldown = MINE_COOLDOWN;
                            sound = Some((pos, true));
                        } else {
                            self.mining = Some(Mining { pos, face, ticks: 1, needed });
                            sound = Some((pos, false));
                        }
                        self.start_swing();
                    }
                }
                (None, _) => {
                    if let Some(m) = self.mining.take() {
                        let seq = self.next_sequence();
                        let _ = session::send_dig(&self.tx, Dig::Abort, m.pos, m.face, seq);
                    }
                }
            }
            if let Some((pos, face)) = chip {
                let state = self.state_at(pos);
                if let Some((rect, tint)) = self.particle_texture(state) {
                    self.particles.crack(pos, face, rect, tint);
                }
            }
            // The server leaves the breaker out of its break broadcast, so the client plays these itself (vanilla too).
            match sound {
                Some((pos, true)) => {
                    let state = self.state_at(pos);
                    self.block_sound(pos, None, |s| &s.break_, |v| (v + 1.0) / 2.0, 0.8);
                    self.break_particles(pos, state);
                }
                Some((pos, false)) => self.block_sound(pos, None, |s| &s.hit, |v| (v + 1.0) / 8.0, 0.5),
                None => {}
            }
        }

        if c.use_item && self.use_cooldown == 0 {
            let seq = self.next_sequence();
            let holding_block = self.held().and_then(|s| self.models.items.get(s.item as usize)).is_some_and(|i| i.block);
            match target {
                Some((pos, face, point)) => {
                    let o = [[0, -1, 0], [0, 1, 0], [0, 0, -1], [0, 0, 1], [-1, 0, 0], [1, 0, 0]][face as usize % 6];
                    self.pending_place = Some(([pos[0] + o[0], pos[1] + o[1], pos[2] + o[2]], Instant::now()));
                    let _ = session::send_use_on(&self.tx, pos, face, point, seq);
                    if holding_block {
                        self.start_swing();
                        // Placing re-raises the hand from the bottom, as vanilla's `itemUsed`.
                        self.main_height = 0.0;
                    }
                }
                None => {
                    let _ = session::send_use(&self.tx, seq, [self.cam.yaw, self.cam.pitch]);
                }
            }
            self.use_cooldown = USE_COOLDOWN;
        }
    }

    /// Updates the crosshair target from the current camera.
    pub fn update_target(&mut self) {
        if self.mode == Mode::Fly || !self.in_world || self.dead.is_some() {
            self.target = None;
            self.entity_target = None;
            return;
        }
        let look = self.cam.look();
        let reach = if self.game_mode == 1 { 5.0 } else { target::REACH };
        let world = self.view();
        let hit = target::raycast(self.cam.eye, [look.x as f64, look.y as f64, look.z as f64], reach, |x, y, z| {
            let Some(s) = world.state(x, y, z) else { return Vec::new() };
            let (fx, fy, fz) = (x as f64, y as f64, z as f64);
            let to_world = |b: &[f32; 6]| Aabb {
                min: [fx + b[0] as f64, fy + b[1] as f64, fz + b[2] as f64],
                max: [fx + b[3] as f64, fy + b[4] as f64, fz + b[5] as f64],
            };
            if !s.collision.is_empty() {
                s.collision.iter().map(to_world).collect()
            } else if !s.quads.is_empty() {
                // Plants and other walk-through blocks are still targetable.
                vec![to_world(&[0.0, 0.0, 0.0, 1.0, 1.0, 1.0])]
            } else {
                Vec::new()
            }
        });
        self.target = hit;
        // An entity in front of the targeted block (and within attack reach) takes the crosshair.
        let eye = self.cam.eye;
        let dir = [look.x as f64, look.y as f64, look.z as f64];
        let block_dist = self.target.as_ref().map_or(f64::MAX, |t| {
            let d = [t.point[0] - eye[0], t.point[1] - eye[1], t.point[2] - eye[2]];
            (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
        });
        let me = self.entity_id;
        self.entity_target = self
            .entities
            .hitboxes(&self.models, Instant::now())
            .filter(|(id, _)| *id != me)
            .filter_map(|(id, b)| target::hit_box(eye, dir, &b).map(|(t, _)| (id, t)))
            .filter(|&(_, t)| t <= ENTITY_REACH && t < block_dist)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(id, _)| id);
    }

    /// Advances the world by `dt` seconds: physics and interaction at 20 Hz, camera interpolated in between.
    pub fn update(&mut self, dt: f32, c: &Controls) {
        if !self.in_world {
            return;
        }
        let frozen = self.dead.is_some() || self.screen_open() || self.chat_input.is_some();
        let c = if frozen { Controls::default() } else { *c };
        // A click between ticks waits for the next tick instead of being dropped.
        self.attack_queued |= c.attack_pressed;
        let input = Input { forward: c.forward, strafe: c.strafe, jump: c.jump, sprint: c.sprint, sneak: c.sneak, pitch: self.cam.pitch };
        if self.mode == Mode::Fly {
            self.fly(dt, &c);
        }
        self.tick_time = (self.tick_time + dt).min(0.25);
        while self.tick_time >= TICK {
            self.tick_time -= TICK;
            self.tick_view(&c);
            if self.mode == Mode::Walk {
                let attack = std::mem::take(&mut self.attack_queued);
                self.interact_tick(&c, attack);
                self.walk_tick(&input);
            } else {
                let _ = session::send_tick(&self.tx, self.feet(), [self.cam.yaw, self.cam.pitch], 0);
            }
        }
        if self.mode == Mode::Walk {
            let t = (self.tick_time / TICK) as f64;
            let eye = if self.body.swimming { 0.4 } else if input.sneak { SNEAK_EYE_HEIGHT } else { EYE_HEIGHT };
            for i in 0..3 {
                self.cam.eye[i] = self.prev_feet[i] + (self.body.pos[i] - self.prev_feet[i]) * t;
            }
            self.cam.eye[1] += eye;
        }
    }

    /// The per-tick parts of the first-person view and the world around it.
    fn tick_view(&mut self, c: &Controls) {
        self.swing.tick();
        self.attack_ticker += 1;
        self.swap_ticker += 1;
        let held = self.held().map(|s| s.item);
        if held != self.last_held {
            // Switching to a different item resets the attack strength (and so dips the hand).
            self.last_held = held;
            self.reset_attack_strength();
        }
        // FirstPersonHandsAndItems.tick: lower toward 0 while the shown item is outdated, then raise with the
        // cube of the swap scale; the new item appears once the hand is nearly down.
        self.o_main_height = self.main_height;
        let swap = ((self.swap_ticker as f32 + 1.0) / self.attack_delay()).clamp(0.0, 1.0);
        let target = if self.shown_item != held { 0.0 } else { swap * swap * swap };
        self.main_height += (target - self.main_height).clamp(-0.4, 0.4);
        if self.main_height < 0.1 {
            self.shown_item = held;
        }
        // LocalPlayer: the hand lags the view by half each tick.
        self.x_bob_o = self.x_bob;
        self.y_bob_o = self.y_bob;
        self.x_bob += (self.cam.pitch - self.x_bob) * 0.5;
        self.y_bob += (self.cam.yaw - self.y_bob) * 0.5;
        self.hurt_time = (self.hurt_time - 1.0).max(0.0);
        // FOV: sprinting widens it (movement speed modifier), smoothed like vanilla.
        self.fov_mod_o = self.fov_mod;
        let target_fov = if self.mode == Mode::Fly { 1.1 } else if c.sprint && c.forward > 0.0 { 1.15 } else { 1.0 };
        self.fov_mod += (target_fov - self.fov_mod) * 0.5;
        self.entities.tick();
        let world = WorldView { chunks: &self.world, models: &self.models };
        self.particles.tick(|x, y, z| world.state(x, y, z).is_some_and(|s| !s.collision.is_empty()));
        self.sky.tick();
        // Water vision: grows while the eyes are underwater, fades fast after.
        self.water_time = if self.eye_fluid() == Some(mc_assets::Fluid::Water) { (self.water_time + 1.0).min(600.0) } else { (self.water_time - 10.0).max(0.0) };
        if self.action_bar.as_ref().is_some_and(|(_, at)| at.elapsed() > Duration::from_secs(3)) {
            self.action_bar = None;
        }
    }

    fn walk_tick(&mut self, input: &Input) {
        self.prev_feet = self.body.pos;
        let world = WorldView { chunks: &self.world, models: &self.models };
        self.body.tick(input, self.cam.yaw, &world);
        let (dx, dz) = (self.body.pos[0] - self.prev_feet[0], self.body.pos[2] - self.prev_feet[2]);
        let horizontal = (dx * dx + dz * dz).sqrt();
        // View bob: walkDist grows by 0.6 per block, bob eases toward the horizontal speed (capped at 0.1).
        self.walk_dist_o = self.walk_dist;
        self.walk_dist += horizontal as f32 * 0.6;
        self.bob_o = self.bob;
        let t_bob = if self.body.on_ground { (horizontal as f32).min(0.1) } else { 0.0 };
        self.bob += (t_bob - self.bob) * 0.4;
        // Footsteps like vanilla: walked distance * 0.6, one step sound per whole unit, from the block underfoot.
        if self.body.on_ground && !input.sneak {
            self.walked += horizontal * 0.6;
            if self.walked > self.next_step {
                self.next_step = self.walked.floor() + 1.0;
                let [x, y, z] = self.body.pos;
                let below = [x.floor() as i32, (y - 0.2).floor() as i32, z.floor() as i32];
                self.block_sound(below, None, |s| &s.step, |v| v * 0.15, 1.0);
            }
        }
        let sprinting = input.sprint && input.forward > 0.0;
        if sprinting != self.sprinting {
            self.sprinting = sprinting;
            let _ = session::send_sprint(&self.tx, self.entity_id, sprinting);
        }
        let flags = self.body.on_ground as u8 | (self.body.horizontal_collision as u8) << 1;
        let _ = session::send_tick(&self.tx, self.body.pos, [self.cam.yaw, self.cam.pitch], flags);
    }

    fn fly(&mut self, dt: f32, c: &Controls) {
        let speed = if c.sprint { 30.0 } else { 10.0 } * dt as f64;
        let y = self.cam.yaw.to_radians() as f64;
        let forward = [-y.sin(), 0.0, y.cos()];
        let left = [y.cos(), 0.0, y.sin()];
        let mut mv = [0.0f64; 3];
        for i in 0..3 {
            mv[i] = forward[i] * c.forward as f64 + left[i] * c.strafe as f64;
        }
        mv[1] += c.jump as i32 as f64 - c.sneak as i32 as f64;
        let len = (mv[0] * mv[0] + mv[1] * mv[1] + mv[2] * mv[2]).sqrt();
        if len > 0.0 {
            let scale = speed / len.max(1.0);
            for i in 0..3 {
                self.cam.eye[i] += mv[i] * scale;
            }
        }
    }

    pub fn respawn(&mut self) {
        let _ = session::send_respawn(&self.tx);
    }

    /// The camera for this frame: (world view-projection, eye, hand camera space to world).
    pub fn camera(&self, aspect: f32, o: &Options) -> (Mat4, Vec3, Mat4) {
        let a = self.tick_time / TICK;
        let eye = Vec3::new(self.cam.eye[0] as f32, self.cam.eye[1] as f32, self.cam.eye[2] as f32);
        let view = glam::camera::rh::view::look_to_mat4(eye, self.cam.look(), Vec3::Y);
        let mut bob = Mat4::IDENTITY;
        // GameRenderer.bobHurt.
        if self.hurt_time > 0.0 {
            let h = ((self.hurt_time - a).max(0.0) / 10.0).powi(4);
            let h = (h * std::f32::consts::PI).sin();
            bob *= Mat4::from_rotation_z((-h * 14.0).to_radians());
        }
        if self.dead.is_some() {
            bob *= Mat4::from_rotation_z(20f32.to_radians());
        }
        // GameRenderer.bobView.
        if o.view_bobbing && self.mode == Mode::Walk {
            let wd = -(self.walk_dist + (self.walk_dist - self.walk_dist_o) * a);
            let b = lerp(self.bob_o, self.bob, a);
            let pi = std::f32::consts::PI;
            bob *= Mat4::from_translation(Vec3::new((wd * pi).sin() * b * 0.5, -((wd * pi).cos() * b).abs(), 0.0))
                * Mat4::from_rotation_z(((wd * pi).sin() * b * 3.0).to_radians())
                * Mat4::from_rotation_x((((wd * pi - 0.2).cos() * b).abs() * 5.0).to_radians());
        }
        let view = bob * view;
        let fov = o.fov * lerp(self.fov_mod_o, self.fov_mod, a);
        let far = o.render_distance as f32 * 16.0 * 1.5 + 400.0;
        let proj = glam::camera::rh::proj::directx::perspective(fov.to_radians(), aspect, 0.05, far);
        // The hand is drawn as if with vanilla's fixed 70 degree hand FOV.
        let k = (fov.to_radians() / 2.0).tan() / 35f32.to_radians().tan();
        let hand = view.inverse() * Mat4::from_scale(Vec3::new(k, k, 1.0)) * bob;
        (proj * view, eye, hand)
    }

    /// World-space geometry besides the chunks for this frame. `visible` culls entities (see [`entities::mesh`]).
    pub fn overlay(&mut self, o: &Options, hand_to_world: Mat4, visible: &dyn Fn(Vec3, Vec3) -> bool) -> Overlay {
        let mut ov = Overlay::default();
        let now = Instant::now();
        let env = self.sky.environment(o.gamma);
        let models = self.models.clone();
        let me = (self.mode == Mode::Walk).then_some(self.entity_id);
        let world = &self.world;
        let light_fn = |p: [f64; 3]| {
            let (x, y, z) = (p[0].floor() as i32, p[1].floor() as i32, p[2].floor() as i32);
            world.get(&(x.div_euclid(16), z.div_euclid(16))).map_or(1.0, |c| {
                let (s, b) = c.light(x.rem_euclid(16) as usize, y, z.rem_euclid(16) as usize);
                mc_render::brightness(s as f32, b as f32, &env)
            })
        };
        entities::mesh(&mut self.entities, &models, now, light_fn, &mut ov.entities, &mut ov.entity_items, me, visible);

        let eye = Vec3::new(self.cam.eye[0] as f32, self.cam.eye[1] as f32, self.cam.eye[2] as f32);
        if self.eye_fluid().is_none() {
            ov.celestial = self.sky.celestial(eye, &models.gui, self.rain);
        }
        let look = self.cam.look();
        let right = look.cross(Vec3::Y).normalize_or_zero();
        let up = right.cross(look);
        let t = self.tick_time / TICK;
        ov.quads = self.particles.mesh(right, up, t, |p| light_fn([p.x as f64, p.y as f64, p.z as f64]));

        if let Some(tg) = &self.target {
            for b in &tg.boxes {
                push_box(&mut ov.world, b.min.map(|v| v as f32 - 0.002), b.max.map(|v| v as f32 + 0.002), [0, 0, 0, 102]);
            }
        }
        // Cracks on the block being mined, one of vanilla's 10 destroy stages by progress.
        if let (Some(m), Some(tg)) = (&self.mining, &self.target) {
            if m.pos == tg.pos {
                let stage = (m.ticks * 10 / m.needed.max(1)).min(9) as usize;
                let [u0, v0, u1, v1] = models.crack_uv[stage];
                for b in &tg.boxes {
                    let (lo, hi) = (b.min.map(|v| v as f32 - 0.003), b.max.map(|v| v as f32 + 0.003));
                    for axis in 0..3 {
                        for side in [lo[axis], hi[axis]] {
                            let (a, cc) = ((axis + 1) % 3, (axis + 2) % 3);
                            let corner = |i: f32, j: f32| {
                                let mut p = [0.0; 3];
                                p[axis] = side;
                                p[a] = if i == 0.0 { lo[a] } else { hi[a] };
                                p[cc] = if j == 0.0 { lo[cc] } else { hi[cc] };
                                mc_render::Vertex { pos: p, uv: [u0 + (u1 - u0) * i, v0 + (v1 - v0) * j], color: [255, 255, 255, 255] }
                            };
                            let q = [corner(0.0, 0.0), corner(0.0, 1.0), corner(1.0, 1.0), corner(1.0, 0.0)];
                            ov.quads.extend_from_slice(&[q[0], q[1], q[2], q[0], q[2], q[3]]);
                        }
                    }
                }
            }
        }
        if self.show_hitboxes {
            let me = self.entity_id;
            for (id, b) in self.entities.hitboxes(&models, now) {
                if id != me {
                    push_box(&mut ov.world, b.min.map(|v| v as f32), b.max.map(|v| v as f32), [255, 255, 255, 255]);
                }
            }
        }
        if self.show_chunk_borders {
            chunk_borders(&mut ov.world, self.cam.eye, self.sky.dimension.min_section());
        }

        // The hand: hidden in spectator and while dead.
        if self.mode == Mode::Walk && self.dead.is_none() && self.game_mode != 3 {
            let pose = hand::Pose {
                cam_to_world: hand_to_world,
                attack: self.swing.get(t),
                inverse_height: 1.0 - lerp(self.o_main_height, self.main_height, t),
                sway: [(self.cam.pitch - lerp(self.x_bob_o, self.x_bob, t)) * 0.1, (self.cam.yaw - lerp(self.y_bob_o, self.y_bob, t)) * 0.1],
                light: light_fn(self.cam.eye),
            };
            match self.shown_item.and_then(|i| models.items.get(i as usize)) {
                Some(item) if item.model.is_some() => hand::item(&mut ov.hand_item, &pose, item),
                _ => hand::arm(&mut ov.hand_skin, &pose, &self.player_model, models.gui.sprite("steve")),
            }
        }
        ov
    }

    /// The fluid the camera is inside, below its surface (vanilla's `isEyeInFluid`).
    pub fn eye_fluid(&self) -> Option<mc_assets::Fluid> {
        let [x, y, z] = self.cam.eye;
        let (bx, by, bz) = (x.floor() as i32, y.floor() as i32, z.floor() as i32);
        let s = self.models.states.get(self.state_at([bx, by, bz]) as usize)?;
        let fluid = s.fluid?;
        let above = self.models.states.get(self.state_at([bx, by + 1, bz]) as usize).and_then(|a| a.fluid) == Some(fluid);
        let height = if above || s.fluid_level >= 8 || s.fluid_level == 0 { if above { 1.0 } else { 8.0 / 9.0 } } else { (8 - s.fluid_level) as f64 / 9.0 };
        (y < by as f64 + height).then_some(fluid)
    }

    /// `LocalPlayer.getWaterVision`: how far you see underwater grows over 30 seconds.
    fn water_vision(&self) -> f32 {
        let t = self.water_time;
        if t >= 600.0 {
            return 1.0;
        }
        let a = (t / 100.0).clamp(0.0, 1.0);
        let b = if t < 100.0 { 0.0 } else { ((t - 100.0) / 500.0).clamp(0.0, 1.0) };
        a * 0.6 + b * 0.39999998
    }

    /// Sky, fog and light for this frame, with vanilla's water and lava fog when the camera is in them.
    pub fn environment(&self, o: &Options) -> mc_render::Environment {
        let mut env = self.sky.environment(o.gamma);
        let fog = match self.eye_fluid() {
            Some(mc_assets::Fluid::Water) => Some(([5.0 / 255.0, 5.0 / 255.0, 51.0 / 255.0], [-8.0, 96.0 * self.water_vision().max(0.25)])),
            Some(mc_assets::Fluid::Lava) => Some(([0.6, 0.04, 0.0], [0.25, 1.0])),
            None => None,
        };
        if let Some((color, range)) = fog {
            // Fog colors darken with the sky light like the rest of the world.
            let k = env.daylight.max(0.3);
            env.fog = color.map(|c| c * k);
            env.sky = env.fog;
            env.sunrise = [0.0; 4];
            env.fog_range = Some(range);
        }
        env
    }

    /// Full-screen effects drawn under the HUD: the underwater texture and fire while burning.
    fn screen_effects(&self, p: &mut Painter) {
        if self.eye_fluid() == Some(mc_assets::Fluid::Water) {
            // ScreenEffectRenderer.renderWater: the texture tiled 4x4 at 10% opacity.
            // shortcut: vanilla also drifts it with the view; needs a wrapping sampler to do here.
            let (w, h) = (p.width / 4.0, p.height / 4.0);
            for i in 0..4 {
                for j in 0..4 {
                    p.sprite_rect("underwater", i as f32 * w, j as f32 * h, w, h, [255, 255, 255, 26]);
                }
            }
        }
        if self.on_fire && self.game_mode != 1 && self.game_mode != 3 {
            // ScreenEffectRenderer.renderFire: two flames rising from the bottom corners.
            let [u0, v0, u1, v1] = self.models.atlas.rect("minecraft:block/fire_1");
            let (w, h, k) = (p.width, p.height, p.scale);
            for (x0, x1, flip) in [(-w * 0.15, w * 0.45, false), (w * 0.55, w * 1.15, true)] {
                let (y0, y1) = (h * 0.35, h * 1.05);
                let (a, b) = if flip { (u1, u0) } else { (u0, u1) };
                p.gui.poly(mc_render::GuiTexture::Blocks, [[x0 * k, y0 * k], [x0 * k, y1 * k], [x1 * k, y1 * k], [x1 * k, y0 * k]], [[a, v0], [a, v1], [b, v1], [b, v0]], [[255, 255, 255, 230]; 4]);
            }
        }
    }

    /// The in-game HUD for this frame.
    pub fn hud(&self, p: &mut Painter, o: &Options, show_tab: bool) {
        self.screen_effects(p);
        let survival = matches!(self.game_mode, 0 | 2);
        if self.game_mode != 3 {
            hud::hotbar(p, &self.inventory, self.slot);
        }
        if survival {
            let status = hud::Status {
                health: self.health,
                food: self.food,
                hardcore: self.hardcore,
                air: self.air,
                xp_progress: self.xp.0,
                xp_level: self.xp.1,
                blink: self.hurt_time > 0.0 && (self.hurt_time as i32) % 2 == 0,
            };
            hud::status(p, &status);
        }
        if !self.screen_open() && self.mode == Mode::Walk && self.dead.is_none() {
            hud::crosshair(p, self.attack_strength(self.tick_time / TICK), o.attack_indicator && survival);
        }
        if let Some((text, at)) = &self.action_bar {
            let age = at.elapsed().as_secs_f32();
            hud::action_bar(p, text, (3.0 - age).min(1.0));
        }
        if let Some(start) = self.title.2 {
            let [fade_in, stay, fade_out] = self.title.3.map(|t| t as f32 / 20.0);
            let age = start.elapsed().as_secs_f32();
            let alpha = if age < fade_in { age / fade_in.max(0.01) } else if age < fade_in + stay { 1.0 } else { 1.0 - (age - fade_in - stay) / fade_out.max(0.01) };
            hud::title(p, &self.title.0, &self.title.1, alpha);
        }
        hud::chat(p, &self.chat_log, self.chat_input.as_deref(), o.chat_opacity, o.chat_background);
        if show_tab {
            self.tab_list(p);
        }
    }

    /// The player list (Tab): names in columns with ping bars, the server's header and footer.
    fn tab_list(&self, p: &mut Painter) {
        let mut names: Vec<&(String, i32)> = self.players.values().collect();
        names.sort_by(|a, b| a.0.to_lowercase().cmp(&b.0.to_lowercase()));
        if names.is_empty() {
            return;
        }
        let rows = names.len().min(20);
        let cols = names.len().div_ceil(20);
        let col_w = names.iter().map(|n| p.text_width(&n.0)).fold(0.0, f32::max) + 13.0;
        let total_w = col_w * cols as f32 + (cols as f32 - 1.0) * 5.0;
        let x0 = (p.width / 2.0 - total_w / 2.0).floor();
        let mut y = 10.0;
        let header: Vec<String> = self.tab.0.lines().filter(|l| !l.is_empty()).map(str::to_string).collect();
        let footer: Vec<String> = self.tab.1.lines().filter(|l| !l.is_empty()).map(str::to_string).collect();
        let box_h = (header.len() + rows + footer.len()) as f32 * 9.0 + 2.0;
        p.fill(x0 - 1.0, y - 1.0, total_w + 2.0, box_h + 1.0, [0, 0, 0, 128]);
        for l in &header {
            p.text_centered(l, p.width / 2.0, y, hud::WHITE);
            y += 9.0;
        }
        for (i, (name, ms)) in names.iter().enumerate() {
            let (c, r) = (i / 20, i % 20);
            let x = x0 + c as f32 * (col_w + 5.0);
            let ry = y + r as f32 * 9.0;
            p.fill(x, ry, col_w, 8.0, [255, 255, 255, 32]);
            p.text(name, x, ry, hud::WHITE);
            let bars = match *ms {
                ..0 => "ping_unknown",
                0..150 => "ping_5",
                150..300 => "ping_4",
                300..600 => "ping_3",
                600..1000 => "ping_2",
                _ => "ping_1",
            };
            p.sprite_rect(bars, x + col_w - 11.0, ry, 10.0, 8.0, hud::WHITE);
        }
        y += rows as f32 * 9.0 + 1.0;
        for l in &footer {
            p.text_centered(l, p.width / 2.0, y, hud::WHITE);
            y += 9.0;
        }
    }

    /// The open inventory or container screen, with vanilla's clicks: left/right click, shift-click, double-click
    /// to collect, number keys to swap with the hotbar, Q to throw, click outside to drop.
    pub fn container_screen(&mut self, p: &mut Painter, input: &crate::ui::Input) {
        if let Some(c) = &mut self.creative {
            let models = self.models.clone();
            let max = |i: u32| models.items.get(i as usize).map_or(64, |it| crate::creative::max_stack(&it.name));
            for out in c.frame(p, &self.creative_tabs, &mut self.inventory, input, max) {
                let _ = match out {
                    crate::creative::Out::SetSlot(slot, s) => session::send_creative_slot(&self.tx, &models.components, slot, s.as_ref()),
                    crate::creative::Out::Drop(s) => session::send_creative_slot(&self.tx, &models.components, -1, Some(&s)),
                };
            }
            return;
        }
        let mouse = input.mouse;
        let open = match &self.menu {
            Some(m) => containers::Open::Menu(m),
            None => containers::Open::Player(&self.inventory),
        };
        let (hovered, outside) = containers::draw(p, &open, self.cursor.as_ref(), mouse, mouse, &self.enchantments);
        let (container, state) = match &self.menu {
            Some(m) => (m.id, m.state_id),
            None => (0, self.inventory.state_id),
        };
        let tx = self.tx.clone();
        let click = |slot: i16, button: u8, kind: Click| inventory::send_click(&tx, container, state, slot, button, kind);
        if input.clicked || input.right_clicked {
            let button = if input.right_clicked { 1 } else { 0 };
            match hovered {
                Some(slot) if input.double_click && button == 0 && self.cursor.is_some() => click(slot as i16, 0, Click::PickupAll),
                Some(slot) if input.shift => click(slot as i16, button, Click::QuickMove),
                Some(slot) => click(slot as i16, button, Click::Pickup),
                None if outside && self.cursor.is_some() => click(-999, button, Click::Pickup),
                None => {}
            }
        }
        if let Some(slot) = hovered {
            use winit::keyboard::KeyCode as K;
            let digits = [K::Digit1, K::Digit2, K::Digit3, K::Digit4, K::Digit5, K::Digit6, K::Digit7, K::Digit8, K::Digit9];
            for (i, d) in digits.iter().enumerate() {
                if input.pressed(*d) {
                    click(slot as i16, i as u8, Click::Swap);
                }
            }
            if input.pressed(K::KeyQ) {
                click(slot as i16, input.ctrl as u8, Click::Throw);
            }
        }
    }

    pub fn debug_info(&self, fps: f32, o: &Options, display: (u32, u32), gpu: &str, server: &str) -> crate::debug::Info {
        let feet = self.feet();
        let (bx, by, bz) = (feet[0].floor() as i32, (feet[1] + 0.5).floor() as i32, feet[2].floor() as i32);
        let (s, b) = self.world.get(&(bx.div_euclid(16), bz.div_euclid(16))).map_or((0, 0), |c| c.light(bx.rem_euclid(16) as usize, by, bz.rem_euclid(16) as usize));
        let dim = match self.sky.dimension {
            Dimension::Overworld => "overworld",
            Dimension::Nether => "the_nether",
            Dimension::End => "the_end",
        };
        crate::debug::Info {
            fps,
            max_fps: if o.max_fps >= 260 { "inf".into() } else { o.max_fps.to_string() },
            vsync: o.vsync,
            server: server.to_string(),
            chunks_drawn: self.stats.0,
            chunks_loaded: self.stats.1,
            view_distance: o.render_distance,
            triangles: self.stats.2,
            gpu_mb: self.stats.3 / 1_048_576,
            entities: self.entities.map.len(),
            dimension: format!("minecraft:{dim}"),
            feet,
            yaw: self.cam.yaw,
            pitch: self.cam.pitch,
            sky_light: s,
            block_light: b,
            day_time: self.sky.day_time,
            game_mode: ["Survival", "Creative", "Adventure", "Spectator"][self.game_mode.min(3) as usize].into(),
            display,
            gpu: gpu.to_string(),
            target_block: self.target.as_ref().map(|t| (t.pos, self.models.states.get(self.state_at(t.pos) as usize).map_or("air".to_string(), |s| s.block.to_string()))),
            target_entity: self.entity_target.and_then(|id| self.entities.map.get(&id)).and_then(|e| self.models.entities.get(e.kind as usize)).map(|i| i.name.clone()),
            particles: self.particles.len(),
        }
    }
}

fn push_box(out: &mut Vec<LineVertex>, lo: [f32; 3], hi: [f32; 3], c: [u8; 4]) {
    let p = |i: usize| [if i & 1 == 0 { lo[0] } else { hi[0] }, if i & 2 == 0 { lo[1] } else { hi[1] }, if i & 4 == 0 { lo[2] } else { hi[2] }];
    for (a, b) in [(0, 1), (2, 3), (4, 5), (6, 7), (0, 2), (1, 3), (4, 6), (5, 7), (0, 4), (1, 5), (2, 6), (3, 7)] {
        out.push(LineVertex { pos: p(a), color: c });
        out.push(LineVertex { pos: p(b), color: c });
    }
}

/// F3 + G: chunk corner posts (yellow for the current chunk, red around it) and blue lines every 16 blocks up.
fn chunk_borders(out: &mut Vec<LineVertex>, eye: [f64; 3], min_section: i32) {
    let (cx, cz) = ((eye[0].floor() as i32).div_euclid(16), (eye[2].floor() as i32).div_euclid(16));
    let (y0, y1) = (min_section as f32 * 16.0, 320.0);
    for dz in -1..=2 {
        for dx in -1..=2 {
            let (x, z) = (((cx + dx) * 16) as f32, ((cz + dz) * 16) as f32);
            let col = if (0..=1).contains(&dx) && (0..=1).contains(&dz) { [255, 255, 0, 255] } else { [255, 0, 0, 255] };
            out.push(LineVertex { pos: [x, y0, z], color: col });
            out.push(LineVertex { pos: [x, y1, z], color: col });
        }
    }
    let (x, z) = ((cx * 16) as f32, (cz * 16) as f32);
    let mut y = y0;
    while y <= y1 {
        for (a, b) in [([x, y, z], [x + 16.0, y, z]), ([x + 16.0, y, z], [x + 16.0, y, z + 16.0]), ([x + 16.0, y, z + 16.0], [x, y, z + 16.0]), ([x, y, z + 16.0], [x, y, z])] {
            out.push(LineVertex { pos: a, color: [64, 64, 255, 160] });
            out.push(LineVertex { pos: b, color: [64, 64, 255, 160] });
        }
        y += 16.0;
    }
}
