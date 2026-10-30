//! Vanilla player movement for one 20 Hz tick: input, jumping, friction, gravity, collision and step-up.
//! Ported from `LivingEntity.travelInAir` / `travelInFluid`, `Entity.move` / `collide` and `LocalPlayer.modifyInput` (26.3).

pub const WIDTH: f64 = 0.6;
pub const HEIGHT: f64 = 1.8;
const STEP_HEIGHT: f64 = 0.6;
const GRAVITY: f64 = 0.08;
const EPSILON: f64 = 1.0e-7;

#[derive(Clone, Copy, Debug)]
pub struct Aabb {
    pub min: [f64; 3],
    pub max: [f64; 3],
}

impl Aabb {
    fn offset(&self, d: [f64; 3]) -> Aabb {
        Aabb { min: [self.min[0] + d[0], self.min[1] + d[1], self.min[2] + d[2]], max: [self.max[0] + d[0], self.max[1] + d[1], self.max[2] + d[2]] }
    }

    fn expand_towards(&self, d: [f64; 3]) -> Aabb {
        let mut b = *self;
        for i in 0..3 {
            if d[i] < 0.0 { b.min[i] += d[i] } else { b.max[i] += d[i] }
        }
        b
    }
}

/// What the physics needs to know about the world.
pub trait World {
    /// Collision boxes of the block at a position, in world coordinates. Unloaded blocks should be solid.
    fn boxes(&self, x: i32, y: i32, z: i32, out: &mut Vec<Aabb>);
    fn friction(&self, x: i32, y: i32, z: i32) -> f32;
    fn water(&self, x: i32, y: i32, z: i32) -> bool;
    fn lava(&self, _x: i32, _y: i32, _z: i32) -> bool {
        false
    }
}

#[derive(Default, Clone, Copy)]
pub struct Input {
    /// -1..1, positive = forward.
    pub forward: f32,
    /// -1..1, positive = left (vanilla's `xxa`).
    pub strafe: f32,
    pub jump: bool,
    pub sprint: bool,
    pub sneak: bool,
    /// Look pitch in degrees (down positive), for steering while swimming.
    pub pitch: f32,
}

pub struct Body {
    /// Feet position.
    pub pos: [f64; 3],
    pub vel: [f64; 3],
    pub on_ground: bool,
    pub horizontal_collision: bool,
    jump_delay: u8,
    /// Sprint-swimming: the 0.6-tall swim pose, steered by where you look.
    pub swimming: bool,
}

/// Vanilla scales diagonal stick/key input so the square of possible inputs maps onto the unit circle.
fn square_input(v: [f32; 2]) -> [f32; 2] {
    let len = (v[0] * v[0] + v[1] * v[1]).sqrt();
    if len <= 0.0 {
        return v;
    }
    let dir = [v[0] / len, v[1] / len];
    let (ax, ay) = (dir[0].abs(), dir[1].abs());
    let tan = if ay > ax { ax / ay } else { ay / ax };
    let modified = (len * (1.0 + tan * tan).sqrt()).min(1.0);
    [dir[0] * modified, dir[1] * modified]
}

fn collide_axis(axis: usize, b: &Aabb, shapes: &[Aabb], mut d: f64) -> f64 {
    let (u, v) = ((axis + 1) % 3, (axis + 2) % 3);
    for s in shapes {
        if s.max[u] - EPSILON <= b.min[u] || s.min[u] + EPSILON >= b.max[u] || s.max[v] - EPSILON <= b.min[v] || s.min[v] + EPSILON >= b.max[v] {
            continue;
        }
        if d > 0.0 && s.min[axis] >= b.max[axis] - EPSILON {
            d = d.min(s.min[axis] - b.max[axis]);
        } else if d < 0.0 && s.max[axis] <= b.min[axis] + EPSILON {
            d = d.max(s.max[axis] - b.min[axis]);
        }
    }
    d
}

/// Y first, then the larger horizontal axis last (vanilla's `Direction.axisStepOrder`).
fn collide_with_shapes(movement: [f64; 3], b: &Aabb, shapes: &[Aabb]) -> [f64; 3] {
    if shapes.is_empty() {
        return movement;
    }
    let order = if movement[0].abs() < movement[2].abs() { [1, 2, 0] } else { [1, 0, 2] };
    let mut done = [0.0; 3];
    for axis in order {
        if movement[axis] != 0.0 {
            done[axis] = collide_axis(axis, &b.offset(done), shapes, movement[axis]);
        }
    }
    done
}

fn gather(world: &impl World, region: &Aabb, out: &mut Vec<Aabb>) {
    out.clear();
    // One extra block down so 1.5-tall fences/walls below are included.
    let (x0, y0, z0) = ((region.min[0] - EPSILON).floor() as i32, (region.min[1] - EPSILON).floor() as i32 - 1, (region.min[2] - EPSILON).floor() as i32);
    let (x1, y1, z1) = ((region.max[0] + EPSILON).floor() as i32, (region.max[1] + EPSILON).floor() as i32, (region.max[2] + EPSILON).floor() as i32);
    for x in x0..=x1 {
        for y in y0..=y1 {
            for z in z0..=z1 {
                world.boxes(x, y, z, out);
            }
        }
    }
}

impl Body {
    pub fn new(pos: [f64; 3]) -> Self {
        Self { pos, vel: [0.0; 3], on_ground: false, horizontal_collision: false, jump_delay: 0, swimming: false }
    }

    pub fn aabb(&self) -> Aabb {
        let h = WIDTH / 2.0;
        Aabb { min: [self.pos[0] - h, self.pos[1], self.pos[2] - h], max: [self.pos[0] + h, self.pos[1] + if self.swimming { 0.6 } else { HEIGHT }, self.pos[2] + h] }
    }

    pub fn in_fluid(&self, world: &impl World) -> bool {
        self.in_water(world) || self.in_lava(world)
    }

    fn in_water(&self, world: &impl World) -> bool {
        let [x, y, z] = self.pos;
        let (bx, bz) = (x.floor() as i32, z.floor() as i32);
        world.water(bx, y.floor() as i32, bz) || world.water(bx, (y + 0.9).floor() as i32, bz)
    }

    fn in_lava(&self, world: &impl World) -> bool {
        let [x, y, z] = self.pos;
        let (bx, bz) = (x.floor() as i32, z.floor() as i32);
        world.lava(bx, y.floor() as i32, bz) || world.lava(bx, (y + 0.4).floor() as i32, bz)
    }

    /// `Entity.collide`: collision with step-up onto blocks up to 0.6 high.
    fn collide(&self, movement: [f64; 3], world: &impl World, shapes: &mut Vec<Aabb>) -> [f64; 3] {
        let aabb = self.aabb();
        gather(world, &aabb.expand_towards(movement).expand_towards([0.0, STEP_HEIGHT, 0.0]), shapes);
        let step = collide_with_shapes(movement, &aabb, shapes);
        let (xc, yc, zc) = (movement[0] != step[0], movement[1] != step[1], movement[2] != step[2]);
        let grounded_after = yc && movement[1] < 0.0;
        if (grounded_after || self.on_ground) && (xc || zc) {
            let grounded = if grounded_after { aabb.offset([0.0, step[1], 0.0]) } else { aabb };
            // Candidate step heights are the tops of nearby boxes within reach, lowest first.
            let mut candidates: Vec<f64> = shapes
                .iter()
                .flat_map(|s| [s.min[1], s.max[1]])
                .map(|c| c - grounded.min[1])
                .filter(|&h| h >= 0.0 && h <= STEP_HEIGHT && h != step[1])
                .collect();
            candidates.sort_by(f64::total_cmp);
            candidates.dedup();
            let horiz = |v: [f64; 3]| v[0] * v[0] + v[2] * v[2];
            for h in candidates {
                let up = collide_with_shapes([movement[0], h, movement[2]], &grounded, shapes);
                if horiz(up) > horiz(step) {
                    let to_ground = aabb.min[1] - grounded.min[1];
                    return [up[0], up[1] - to_ground, up[2]];
                }
            }
        }
        step
    }

    /// `Entity.isFree`: nothing solid in the box moved by `d`.
    fn is_free(&self, d: [f64; 3], world: &impl World, shapes: &mut Vec<Aabb>) -> bool {
        let b = self.aabb().offset(d);
        gather(world, &b, shapes);
        !shapes.iter().any(|s| (0..3).all(|i| s.min[i] < b.max[i] - EPSILON && s.max[i] > b.min[i] + EPSILON))
    }

    /// `Entity.canFallAtLeast`: nothing solid within `dist` below the box moved by (dx, dz).
    fn can_fall(&self, dx: f64, dz: f64, dist: f64, world: &impl World, shapes: &mut Vec<Aabb>) -> bool {
        let b = self.aabb();
        let probe = Aabb { min: [b.min[0] + EPSILON + dx, b.min[1] - dist - EPSILON, b.min[2] + EPSILON + dz], max: [b.max[0] - EPSILON + dx, b.min[1], b.max[2] - EPSILON + dz] };
        gather(world, &probe, shapes);
        !shapes.iter().any(|s| (0..3).all(|i| s.min[i] < probe.max[i] && s.max[i] > probe.min[i]))
    }

    /// `Player.maybeBackOffFromEdge`: while sneaking on the ground, shorten the move in 0.05 steps until it
    /// wouldn't drop more than a step height.
    fn back_off_edge(&self, mut d: [f64; 3], world: &impl World, shapes: &mut Vec<Aabb>) -> [f64; 3] {
        if d[1] > 0.0 || !self.on_ground || self.can_fall(0.0, 0.0, STEP_HEIGHT, world, shapes) {
            return d;
        }
        let shrink = |v: f64| if v.abs() <= 0.05 { 0.0 } else { v - 0.05 * v.signum() };
        while d[0] != 0.0 && self.can_fall(d[0], 0.0, STEP_HEIGHT, world, shapes) {
            d[0] = shrink(d[0]);
        }
        while d[2] != 0.0 && self.can_fall(0.0, d[2], STEP_HEIGHT, world, shapes) {
            d[2] = shrink(d[2]);
        }
        while d[0] != 0.0 && d[2] != 0.0 && self.can_fall(d[0], d[2], STEP_HEIGHT, world, shapes) {
            d[0] = shrink(d[0]);
            d[2] = shrink(d[2]);
        }
        d
    }

    /// `Entity.move`: applies collision, updates ground/collision flags and stops velocity on blocked axes.
    fn do_move(&mut self, delta: [f64; 3], world: &impl World, shapes: &mut Vec<Aabb>) {
        let actual = self.collide(delta, world, shapes);
        for i in 0..3 {
            self.pos[i] += actual[i];
        }
        let differs = |a: f64, b: f64| (a - b).abs() > 1.0e-5;
        let (xc, zc) = (differs(delta[0], actual[0]), differs(delta[2], actual[2]));
        let vertical = delta[1] != actual[1];
        self.horizontal_collision = xc || zc;
        self.on_ground = vertical && delta[1] < 0.0;
        if xc {
            self.vel[0] = 0.0;
        }
        if zc {
            self.vel[2] = 0.0;
        }
        if vertical {
            self.vel[1] = 0.0;
        }
    }

    fn move_relative(&mut self, speed: f64, input: [f64; 3], yaw: f32) {
        let len2 = input[0] * input[0] + input[1] * input[1] + input[2] * input[2];
        if len2 < 1.0e-7 {
            return;
        }
        let s = if len2 > 1.0 { speed / len2.sqrt() } else { speed };
        let m = [input[0] * s, input[1] * s, input[2] * s];
        let (sin, cos) = (yaw.to_radians() as f64).sin_cos();
        self.vel[0] += m[0] * cos - m[2] * sin;
        self.vel[1] += m[1];
        self.vel[2] += m[2] * cos + m[0] * sin;
    }

    /// Advances one game tick (1/20 s).
    pub fn tick(&mut self, input: &Input, yaw: f32, world: &impl World) {
        let mut shapes = Vec::new();
        for v in &mut self.vel {
            if v.abs() < 0.003 {
                *v = 0.0;
            }
        }
        let in_water = self.in_water(world);
        let in_lava = !in_water && self.in_lava(world);
        self.jump_delay = self.jump_delay.saturating_sub(1);
        if input.jump {
            if in_water || in_lava {
                self.vel[1] += 0.04;
            } else if self.on_ground && self.jump_delay == 0 {
                self.vel[1] = self.vel[1].max(0.42);
                if input.sprint {
                    let a = yaw.to_radians() as f64;
                    self.vel[0] += -a.sin() * 0.2;
                    self.vel[2] += a.cos() * 0.2;
                }
                self.jump_delay = 10;
            }
        } else {
            self.jump_delay = 0;
        }

        let mut v = [input.strafe, input.forward];
        if v != [0.0, 0.0] {
            v = [v[0] * 0.98, v[1] * 0.98];
            if input.sneak {
                v = [v[0] * 0.3, v[1] * 0.3];
            }
            v = square_input(v);
        }
        let dir = [v[0] as f64, 0.0, v[1] as f64];

        // Entity.updateSwimming: start when sprinting with eyes underwater, stop when no longer sprinting in water.
        let [px, py, pz] = self.pos;
        let water_at = |dy: f64| world.water(px.floor() as i32, (py + dy).floor() as i32, pz.floor() as i32);
        let eye_in_water = water_at(if self.swimming { 0.4 } else { 1.62 });
        self.swimming = input.sprint && in_water && (self.swimming || eye_in_water);
        if in_water && input.sneak {
            // goDownInWater.
            self.vel[1] -= 0.04;
        }
        if self.swimming {
            // Player.travel: ease the vertical speed toward where you look.
            let look_y = -(input.pitch.to_radians() as f64).sin();
            let k = if look_y < -0.2 { 0.085 } else { 0.06 };
            if look_y <= 0.0 || input.jump || water_at(0.9) {
                self.vel[1] += (look_y - self.vel[1]) * k;
            }
        }

        if in_water {
            let drag = if input.sprint { 0.9 } else { 0.8 };
            let falling = self.vel[1] <= 0.0;
            let old_y = self.pos[1];
            self.move_relative(0.02, dir, yaw);
            self.do_move(self.vel, world, &mut shapes);
            self.vel = [self.vel[0] * drag, self.vel[1] * 0.8, self.vel[2] * drag];
            if !input.sprint {
                // getFluidFallingAdjustedMovement: sink slowly.
                let y = self.vel[1];
                self.vel[1] = if falling && (y - 0.005).abs() >= 0.003 && (y - GRAVITY / 16.0).abs() < 0.003 { -0.003 } else { y - GRAVITY / 16.0 };
            }
            // jumpOutOfFluid: a boost onto the bank, only if there's room above.
            let lift = [self.vel[0], self.vel[1] + 0.6 - self.pos[1] + old_y, self.vel[2]];
            if self.horizontal_collision && self.is_free(lift, world, &mut shapes) {
                self.vel[1] = 0.3;
            }
            return;
        }

        if in_lava {
            // travelInLava: slow, heavy drag, a quarter of gravity.
            self.move_relative(0.02, dir, yaw);
            self.do_move(self.vel, world, &mut shapes);
            self.vel = [self.vel[0] * 0.5, self.vel[1] * 0.5 - GRAVITY / 4.0, self.vel[2] * 0.5];
            if self.horizontal_collision {
                self.vel[1] = 0.3;
            }
            return;
        }

        let [x, y, z] = self.pos;
        let friction = if self.on_ground { world.friction(x.floor() as i32, (y - 0.500001).floor() as i32, z.floor() as i32) as f64 } else { 1.0 };
        let speed = if self.on_ground {
            let base = if input.sprint { 0.13 } else { 0.1 };
            if friction > 0.6 { base * (0.21600002 / (friction * friction * friction)) } else { base }
        } else if input.sprint {
            0.025999999
        } else {
            0.02
        };
        self.move_relative(speed, dir, yaw);
        let delta = if input.sneak { self.back_off_edge(self.vel, world, &mut shapes) } else { self.vel };
        self.do_move(delta, world, &mut shapes);
        self.vel[1] = (self.vel[1] - GRAVITY) * 0.98;
        let f = friction * 0.91;
        self.vel[0] *= f;
        self.vel[2] *= f;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Stone below y=64, plus a slab at x=3 and a wall at x=6 along z=0.
    struct Flat;
    impl World for Flat {
        fn boxes(&self, x: i32, y: i32, z: i32, out: &mut Vec<Aabb>) {
            let (fx, fy, fz) = (x as f64, y as f64, z as f64);
            if y < 64 || (x == 6 && y < 66) {
                out.push(Aabb { min: [fx, fy, fz], max: [fx + 1.0, fy + 1.0, fz + 1.0] });
            } else if x == 3 && y == 64 {
                out.push(Aabb { min: [fx, fy, fz], max: [fx + 1.0, fy + 0.5, fz + 1.0] });
            }
        }
        fn friction(&self, _: i32, _: i32, _: i32) -> f32 {
            0.6
        }
        fn water(&self, _: i32, _: i32, _: i32) -> bool {
            false
        }
    }

    fn settle() -> Body {
        let mut b = Body::new([0.5, 70.0, -10.5]);
        for _ in 0..60 {
            b.tick(&Input::default(), 0.0, &Flat);
        }
        b
    }

    #[test]
    fn falls_and_lands_on_the_floor() {
        let b = settle();
        assert!(b.on_ground);
        assert!((b.pos[1] - 64.0).abs() < 1e-9, "y = {}", b.pos[1]);
    }

    #[test]
    fn walk_speed_matches_vanilla() {
        let mut b = settle();
        let forward = Input { forward: 1.0, ..Default::default() };
        // Facing -X (yaw 90) on open ground, away from the slab and wall.
        let start = b.pos[0];
        for _ in 0..100 {
            b.tick(&forward, 90.0, &Flat);
        }
        let before = b.pos[0];
        for _ in 0..20 {
            b.tick(&forward, 90.0, &Flat);
        }
        let per_second = (b.pos[0] - before).abs();
        // Vanilla walking: ~4.317 blocks per second.
        assert!((per_second - 4.317).abs() < 0.01, "walked {per_second} b/s");
        assert!(b.pos[0] < start);
    }

    #[test]
    fn jump_height_matches_vanilla() {
        let mut b = settle();
        let mut peak: f64 = 0.0;
        b.tick(&Input { jump: true, ..Default::default() }, 0.0, &Flat);
        for _ in 0..30 {
            b.tick(&Input::default(), 0.0, &Flat);
            peak = peak.max(b.pos[1] - 64.0);
        }
        assert!((peak - 1.2522).abs() < 0.001, "peak {peak}");
        assert!(b.on_ground);
    }

    #[test]
    fn steps_onto_a_slab_but_not_a_wall() {
        let mut b = settle();
        b.pos[2] = 0.5;
        let east = Input { forward: 1.0, ..Default::default() };
        for _ in 0..60 {
            b.tick(&east, -90.0, &Flat); // yaw -90 faces +X
        }
        // Walked up the slab at x=3 and stopped at the 2-high wall at x=6.
        assert!((b.pos[0] - 5.7).abs() < 1e-6, "x = {}", b.pos[0]);
        assert!(b.horizontal_collision);
        assert!((b.pos[1] - 64.0).abs() < 1e-9);
    }
}

#[cfg(test)]
mod fluid_and_edge_tests {
    use super::*;

    /// A 1-wide stone platform along x at y=63 (top at 64), water below y=60, and a lava pool around x=20.
    struct Ledge;
    impl World for Ledge {
        fn boxes(&self, x: i32, y: i32, z: i32, out: &mut Vec<Aabb>) {
            let (fx, fy, fz) = (x as f64, y as f64, z as f64);
            if (y == 63 && z == 0 && x < 10) || y < 40 {
                out.push(Aabb { min: [fx, fy, fz], max: [fx + 1.0, fy + 1.0, fz + 1.0] });
            }
        }
        fn friction(&self, _: i32, _: i32, _: i32) -> f32 {
            0.6
        }
        fn water(&self, x: i32, y: i32, _: i32) -> bool {
            y < 60 && x < 15
        }
        fn lava(&self, x: i32, y: i32, _: i32) -> bool {
            y < 60 && x >= 15
        }
    }

    #[test]
    fn sneaking_stops_at_the_edge() {
        let mut b = Body::new([0.5, 64.0, 0.5]);
        // Walk forward (+z, off the platform) while sneaking for two seconds.
        for _ in 0..40 {
            b.tick(&Input { forward: 1.0, sneak: true, ..Default::default() }, 0.0, &Ledge);
        }
        assert!(b.pos[1] > 63.99 && b.on_ground, "fell off: {:?}", b.pos);
        let mut b = Body::new([0.5, 64.0, 0.5]);
        for _ in 0..40 {
            b.tick(&Input { forward: 1.0, ..Default::default() }, 0.0, &Ledge);
        }
        assert!(b.pos[1] < 63.0, "walking should fall: {:?}", b.pos);
    }

    #[test]
    fn swims_up_in_water_and_lava() {
        for x in [5.5, 20.5] {
            let mut b = Body::new([x, 50.0, 5.5]);
            for _ in 0..40 {
                b.tick(&Input { jump: true, ..Default::default() }, 0.0, &Ledge);
            }
            assert!(b.pos[1] > 51.0, "didn't rise at x={x}: {:?}", b.pos);
        }
    }
}

#[cfg(test)]
mod swim_tests {
    use super::*;

    /// Deep water everywhere above a stone floor at y=40.
    struct Sea;
    impl World for Sea {
        fn boxes(&self, x: i32, y: i32, z: i32, out: &mut Vec<Aabb>) {
            if y < 40 {
                out.push(Aabb { min: [x as f64, y as f64, z as f64], max: [x as f64 + 1.0, y as f64 + 1.0, z as f64 + 1.0] });
            }
        }
        fn friction(&self, _: i32, _: i32, _: i32) -> f32 {
            0.6
        }
        fn water(&self, _: i32, y: i32, _: i32) -> bool {
            y < 60
        }
    }

    #[test]
    fn sprint_swimming_dives_where_you_look_and_sneak_sinks() {
        let mut b = Body::new([0.5, 50.0, 0.5]);
        for _ in 0..40 {
            b.tick(&Input { forward: 1.0, sprint: true, pitch: 45.0, ..Default::default() }, 0.0, &Sea);
        }
        assert!(b.swimming && b.pos[1] < 47.0 && b.pos[2] > 4.0, "dive: {:?}", b.pos);
        let mut b = Body::new([0.5, 50.0, 0.5]);
        for _ in 0..20 {
            b.tick(&Input { sneak: true, ..Default::default() }, 0.0, &Sea);
        }
        assert!(b.pos[1] < 48.0, "sneak should sink: {:?}", b.pos);
    }
}
