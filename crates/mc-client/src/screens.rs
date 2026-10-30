//! Vanilla's menus: title screen, singleplayer world list and world creation, multiplayer server list, options (with
//! video, sound, controls, key binds, chat, controller, accessibility, language), pause menu, death screen, and the
//! connecting / loading / disconnected screens. Layouts follow the vanilla screen classes.

use crate::hud::{self, Painter};
use crate::options::{Action as Key, Options, key_name};
use crate::servers::{Ping, Server, ServerList};
use crate::ui::{Scroll, Ui};
use crate::worlds::{self, NewWorld, WorldInfo};
use glam::{Mat3, Vec3};
use mc_render::GuiTexture;

pub enum Screen {
    Title { splash: String },
    SelectWorld { worlds: Vec<WorldInfo>, selected: Option<usize>, search: String, scroll: Scroll, error: Option<String> },
    CreateWorld { w: NewWorld, tab: usize },
    EditWorld { folder: String, name: String, cheats: bool },
    ConfirmDelete { folder: String },
    Multiplayer { selected: Option<usize>, scroll: Scroll },
    EditServer { index: Option<usize>, name: String, ip: String },
    DirectConnect { ip: String },
    Connecting { target: String },
    Loading { title: String },
    Message { title: String, text: String, back_to_servers: bool },
    Options,
    Video,
    Sound,
    Controls,
    Mouse,
    KeyBinds { waiting: Option<usize>, scroll: Scroll },
    Chat,
    Controller,
    Accessibility,
    Language,
    Pause,
    Death { message: String, score: i32, hardcore: bool, since: std::time::Instant },
}

/// What a screen wants the app to do.
pub enum Action {
    None,
    Push(Screen),
    Pop,
    Replace(Screen),
    /// Back to the title screen (clearing the stack).
    Title,
    Quit,
    PlayWorld(String),
    CreateWorld(NewWorld),
    Connect(String),
    /// Leave the world (save and quit / disconnect).
    Disconnect,
    Respawn,
    /// Options changed; apply and save.
    OptionsChanged,
    /// Send a command (difficulty in the options of a singleplayer world).
    Command(String),
}

/// Shared state screens read and change.
pub struct Ctx<'a> {
    pub options: &'a mut Options,
    pub servers: &'a mut ServerList,
    pub worlds_dir: &'a std::path::Path,
    pub player: &'a str,
    pub in_world: bool,
    pub singleplayer: bool,
    /// The world's difficulty (0..3), for the in-game options button.
    pub difficulty: u8,
    /// Progress text while loading.
    pub status: &'a str,
    /// Seconds since start, for the panorama spin.
    pub time: f32,
}

const SPLASH_FALLBACK: &str = "Also try Rust!";

pub fn title(splashes: &[String]) -> Screen {
    let i = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos() as usize);
    let splash = if splashes.is_empty() { SPLASH_FALLBACK.to_string() } else { splashes[i % splashes.len()].clone() };
    Screen::Title { splash }
}

pub fn select_world(dir: &std::path::Path) -> Screen {
    Screen::SelectWorld { worlds: worlds::list(dir), selected: None, search: String::new(), scroll: Scroll::default(), error: None }
}

/// The rotating title panorama (`CubeMap`): six faces seen from inside, 85 degree FOV, pitched down 10 degrees.
pub fn panorama(p: &mut Painter, time: f32, blur: bool) {
    // Face corners (cube space) and their UVs, as vanilla's cube map vertex buffer.
    const FACES: [[[f32; 3]; 4]; 6] = [
        [[-1., -1., 1.], [-1., 1., 1.], [1., 1., 1.], [1., -1., 1.]],
        [[1., -1., 1.], [1., 1., 1.], [1., 1., -1.], [1., -1., -1.]],
        [[1., -1., -1.], [1., 1., -1.], [-1., 1., -1.], [-1., -1., -1.]],
        [[-1., -1., -1.], [-1., 1., -1.], [-1., 1., 1.], [-1., -1., 1.]],
        [[-1., -1., -1.], [-1., -1., 1.], [1., -1., 1.], [1., -1., -1.]],
        [[-1., 1., 1.], [-1., 1., -1.], [1., 1., -1.], [1., 1., 1.]],
    ];
    const UV: [[f32; 2]; 4] = [[0., 0.], [0., 1.], [1., 1.], [1., 0.]];
    let spin = -(time * 2.0) % 360.0;
    let rot = Mat3::from_rotation_x(std::f32::consts::PI) * Mat3::from_rotation_x(10f32.to_radians()) * Mat3::from_rotation_y(spin.to_radians());
    let (w, h) = (p.width * p.scale, p.height * p.scale);
    let f = 1.0 / (85f32.to_radians() / 2.0).tan();
    let aspect = w / h;
    let project = |v: Vec3| -> Option<[f32; 2]> {
        let c = rot * v;
        if c.z > -0.05 {
            return None;
        }
        let (x, y) = (c.x / -c.z * f / aspect, c.y / -c.z * f);
        Some([(x * 0.5 + 0.5) * w, (0.5 - y * 0.5) * h])
    };
    let tex = if blur { GuiTexture::PanoramaBlur } else { GuiTexture::Panorama };
    const N: usize = 12;
    for (k, face) in FACES.iter().enumerate() {
        let [a, b, c, d] = face.map(Vec3::from);
        let at = |s: f32, t: f32| {
            // Bilinear over the face: s along a->d, t along a->b.
            let top = a.lerp(d, s);
            let bottom = b.lerp(c, s);
            top.lerp(bottom, t)
        };
        let uv = |s: f32, t: f32| {
            let u = UV[0][0] + (UV[3][0] - UV[0][0]) * s;
            let v = UV[0][1] + (UV[1][1] - UV[0][1]) * t;
            let inset = 0.5 / 1024.0;
            [(k as f32 + u.clamp(inset, 1.0 - inset)) / 6.0, v.clamp(inset, 1.0 - inset)]
        };
        for i in 0..N {
            for j in 0..N {
                let (s0, s1, t0, t1) = (i as f32 / N as f32, (i + 1) as f32 / N as f32, j as f32 / N as f32, (j + 1) as f32 / N as f32);
                let corners = [(s0, t0), (s0, t1), (s1, t1), (s1, t0)];
                let pts: Option<Vec<[f32; 2]>> = corners.iter().map(|&(s, t)| project(at(s, t))).collect();
                let Some(pts) = pts else { continue };
                p.gui.poly(tex, [pts[0], pts[1], pts[2], pts[3]], corners.map(|(s, t)| uv(s, t)), [[255; 4]; 4]);
            }
        }
    }
}

/// Background for menus: the world dimmed when in one, else the blurred panorama.
fn background(p: &mut Painter, ctx: &Ctx) {
    if ctx.in_world {
        p.gradient(0.0, 0.0, p.width, p.height, [16, 16, 16, 192], [16, 16, 16, 208]);
    } else {
        panorama(p, ctx.time, true);
        p.fill(0.0, 0.0, p.width, p.height, [0, 0, 0, 90]);
    }
}

/// The dark header/footer bands with separators that list screens have.
fn list_frame(p: &mut Painter, top: f32, bottom: f32) {
    p.tiled("menu_list_background", 0.0, top, p.width, bottom - top, [255, 255, 255, 255], 0.0);
    p.tiled("header_separator", 0.0, top - 2.0, p.width, 2.0, hud::WHITE, 0.0);
    p.tiled("footer_separator", 0.0, bottom, p.width, 2.0, hud::WHITE, 0.0);
}

fn title_text(ui: &mut Ui, text: &str, y: f32) {
    let w = ui.p.width;
    ui.label(text, w / 2.0, y, hud::WHITE);
}

fn date(ms: i64) -> String {
    if ms <= 0 {
        return String::new();
    }
    // Civil date from days since 1970 (Howard Hinnant's algorithm), UTC.
    let secs = ms / 1000;
    let days = secs.div_euclid(86400);
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + (m <= 2) as i64;
    let tod = secs.rem_euclid(86400);
    format!("{m}/{d}/{:02}, {}:{:02}", y % 100, tod / 3600, (tod / 60) % 60)
}

/// Runs one frame of the top screen.
pub fn run(screen: &mut Screen, ui: &mut Ui, ctx: &mut Ctx) -> Action {
    let (w, h) = (ui.p.width, ui.p.height);
    let cx = (w / 2.0).floor();
    let back = ui.input.back || ui.input.pressed(winit::keyboard::KeyCode::Escape);
    match screen {
        Screen::Title { splash } => {
            panorama(ui.p, ctx.time, false);
            let lx = cx - 128.0;
            ui.p.sprite("logo", lx, 30.0);
            let ed = "RUST EDITION";
            let ew = ui.p.text_width(ed) * 1.5;
            ui.p.text_ex(ed, cx - ew / 2.0, 30.0 + 44.0 - 4.0, [0xf0, 0x8a, 0x3c, 255], 1.5, true);
            if !ctx.options.hide_splashes {
                let t = (ctx.time * 1000.0 % 1000.0) / 1000.0 * std::f32::consts::TAU;
                let s = (1.8 - t.sin().abs() * 0.1) * 100.0 / (ui.p.text_width(splash) + 32.0);
                ui.p.text_rotated(splash, cx + 123.0, 69.0, -20.0, s, [255, 255, 0, 255]);
            }
            let top = (h / 4.0 + 48.0).floor();
            if ui.button("Singleplayer", cx - 100.0, top, 200.0, 20.0, true) {
                return Action::Push(select_world(ctx.worlds_dir));
            }
            if ui.button("Multiplayer", cx - 100.0, top + 24.0, 200.0, 20.0, true) {
                ctx.servers.refresh();
                return Action::Push(Screen::Multiplayer { selected: None, scroll: Scroll::default() });
            }
            ui.button("Minecraft Realms", cx - 100.0, top + 48.0, 200.0, 20.0, false);
            let icons_y = top + 72.0;
            if icon_button(ui, "icon_language", cx - 22.0, icons_y) {
                return Action::Push(Screen::Language);
            }
            if icon_button(ui, "icon_accessibility", cx + 2.0, icons_y) {
                return Action::Push(Screen::Accessibility);
            }
            if ui.button("Options...", cx - 100.0, top + 96.0, 98.0, 20.0, true) {
                return Action::Push(Screen::Options);
            }
            if ui.button("Quit Game", cx + 2.0, top + 96.0, 98.0, 20.0, true) {
                return Action::Quit;
            }
            ui.p.text("Minecraft 26.3 (mcrs)", 2.0, h - 10.0, hud::WHITE);
            let c = "Copyright Mojang AB. Do not distribute!";
            let cw = ui.p.text_width(c);
            ui.p.text(c, w - cw - 2.0, h - 10.0, hud::WHITE);
            Action::None
        }

        Screen::SelectWorld { worlds, selected, search, scroll, error } => {
            background(ui.p, ctx);
            title_text(ui, "Select World", 8.0);
            ui.text_field(search, "Search...", cx - 100.0, 22.0, 200.0, 20.0, 64);
            let (top, bottom) = (48.0, h - 64.0);
            list_frame(ui.p, top, bottom);
            let shown: Vec<usize> = (0..worlds.len()).filter(|&i| search.is_empty() || worlds[i].name.to_lowercase().contains(&search.to_lowercase())).collect();
            let row = 36.0;
            scroll.update(ui.input, ui.input.mouse.1 >= top && ui.input.mouse.1 < bottom, shown.len() as f32 * row + 4.0, bottom - top);
            ui.p.clip(Some([0.0, top, w, bottom - top]));
            let mut play = None;
            for (n, &i) in shown.iter().enumerate() {
                let y = top + 4.0 + n as f32 * row - scroll.offset;
                if y + row < top || y > bottom {
                    continue;
                }
                let x = cx - 110.0;
                let (clicked, hot) = ui.area(x, y, 220.0, row - 4.0);
                if clicked {
                    if *selected == Some(i) && ui.input.double_click {
                        play = Some(i);
                    }
                    *selected = Some(i);
                }
                let sel = *selected == Some(i);
                if sel || hot {
                    ui.p.fill(x - 2.0, y - 2.0, 224.0, row, if sel { [255, 255, 255, 255] } else { [128, 128, 128, 255] });
                    ui.p.fill(x - 1.0, y - 1.0, 222.0, row - 2.0, [0, 0, 0, 255]);
                }
                let wi = &worlds[i];
                ui.p.sprite_rect("pack", x, y, 32.0, 32.0, hud::WHITE);
                ui.p.text(&wi.name, x + 35.0, y + 1.0, hud::WHITE);
                ui.p.text(&format!("{} ({})", wi.folder, date(wi.last_played)), x + 35.0, y + 12.0, [128, 128, 128, 255]);
                let cheats = if wi.cheats { ", Cheats" } else { "" };
                ui.p.text(&format!("{}{cheats}, Version: {}", wi.mode_name(), wi.version), x + 35.0, y + 22.0, [128, 128, 128, 255]);
            }
            ui.p.clip(None);
            if worlds.is_empty() {
                ui.label("No worlds yet. Create one!", cx, top + 20.0, hud::GRAY);
            }
            if let Some(e) = error {
                ui.label(&e.clone(), cx, bottom - 12.0, [255, 85, 85, 255]);
            }
            let has = selected.is_some_and(|s| s < worlds.len());
            if ui.button("Play Selected World", cx - 154.0, h - 52.0, 150.0, 20.0, has) {
                play = *selected;
            }
            if let Some(i) = play {
                return Action::PlayWorld(worlds[i].folder.clone());
            }
            if ui.button("Create New World", cx + 4.0, h - 52.0, 150.0, 20.0, true) {
                return Action::Push(Screen::CreateWorld { w: NewWorld::default(), tab: 0 });
            }
            if ui.button("Edit", cx - 154.0, h - 28.0, 72.0, 20.0, has) {
                let wi = &worlds[selected.unwrap()];
                return Action::Push(Screen::EditWorld { folder: wi.folder.clone(), name: wi.name.clone(), cheats: wi.cheats });
            }
            if ui.button("Delete", cx - 76.0, h - 28.0, 72.0, 20.0, has) {
                return Action::Push(Screen::ConfirmDelete { folder: worlds[selected.unwrap()].folder.clone() });
            }
            if ui.button("Re-Create", cx + 4.0, h - 28.0, 72.0, 20.0, has) {
                let wi = &worlds[selected.unwrap()];
                let w = NewWorld { name: wi.name.clone(), mode: if wi.hardcore { 2 } else if wi.game_mode == 1 { 1 } else { 0 }, cheats: wi.cheats, ..NewWorld::default() };
                return Action::Push(Screen::CreateWorld { w, tab: 0 });
            }
            if ui.button("Back", cx + 82.0, h - 28.0, 72.0, 20.0, true) || back {
                return Action::Pop;
            }
            Action::None
        }

        Screen::CreateWorld { w: nw, tab } => {
            background(ui.p, ctx);
            let tabs = ["Game", "World", "More"];
            let tw = 70.0;
            let tx = cx - tw * 1.5;
            for (i, t) in tabs.iter().enumerate() {
                let x = tx + i as f32 * tw;
                let (clicked, hot) = ui.area(x, 2.0, tw, 24.0);
                let sprite = if i == *tab { "tab_selected" } else if hot { "tab_highlighted" } else { "tab" };
                ui.p.nine_slice(sprite, x, 2.0, tw, 24.0, 2.0);
                ui.p.text_centered(t, x + tw / 2.0, 10.0, if i == *tab { hud::WHITE } else { hud::GRAY });
                if clicked {
                    *tab = i;
                }
            }
            let y0 = 40.0;
            match *tab {
                0 => {
                    ui.p.text("World Name", cx - 104.0, y0, hud::GRAY);
                    ui.text_field(&mut nw.name, "", cx - 104.0, y0 + 12.0, 208.0, 20.0, 32);
                    ui.cycle("Game Mode", &worlds::MODES, &mut nw.mode, cx - 104.0, y0 + 40.0, 208.0, true);
                    let desc = ["Explore, gather, craft, and fight", "Unlimited resources, free flying", "Survival with one life, Hard"][nw.mode];
                    ui.label(desc, cx, y0 + 64.0, hud::GRAY);
                    ui.cycle("Difficulty", &worlds::DIFFICULTIES, &mut nw.difficulty, cx - 104.0, y0 + 80.0, 208.0, nw.mode != 2);
                    let mut cheats = nw.cheats && nw.mode != 2;
                    let mut i = cheats as usize;
                    if ui.cycle("Allow Commands", &["OFF", "ON"], &mut i, cx - 104.0, y0 + 104.0, 208.0, nw.mode != 2) {
                        cheats = i == 1;
                        nw.cheats = cheats;
                    }
                    ui.label("Commands like /gamemode; also needed for noclip flying", cx, y0 + 128.0, hud::GRAY);
                }
                1 => {
                    ui.cycle("World Type", &worlds::WORLD_TYPES, &mut nw.world_type, cx - 104.0, y0, 208.0, true);
                    ui.p.text("Seed for the world generator", cx - 104.0, y0 + 28.0, hud::GRAY);
                    ui.text_field(&mut nw.seed, "Leave blank for a random seed", cx - 104.0, y0 + 40.0, 208.0, 20.0, 32);
                    ui.toggle("Generate Structures", &mut nw.structures, cx - 104.0, y0 + 68.0, 208.0);
                }
                _ => {
                    ui.label("Game rules, data packs and experiments", cx, y0 + 10.0, hud::GRAY);
                    ui.label("can be changed in the world with /gamerule.", cx, y0 + 22.0, hud::GRAY);
                }
            }
            let ok = !nw.name.trim().is_empty();
            if ui.button("Create New World", cx - 155.0, h - 28.0, 150.0, 20.0, ok) {
                return Action::CreateWorld(nw.clone());
            }
            if ui.button("Cancel", cx + 5.0, h - 28.0, 150.0, 20.0, true) || back {
                return Action::Pop;
            }
            Action::None
        }

        Screen::EditWorld { folder, name, cheats } => {
            background(ui.p, ctx);
            title_text(ui, "Edit World", 15.0);
            ui.p.text("World Name", cx - 100.0, h / 4.0 - 4.0, hud::GRAY);
            ui.text_field(name, "", cx - 100.0, h / 4.0 + 8.0, 200.0, 20.0, 32);
            ui.toggle("Allow Commands", cheats, cx - 100.0, h / 4.0 + 36.0, 200.0);
            ui.label(&format!("Folder: {folder}"), cx, h / 4.0 + 62.0, hud::GRAY);
            if ui.button("Save", cx - 100.0, h / 4.0 + 120.0, 98.0, 20.0, !name.trim().is_empty()) {
                let dir = ctx.worlds_dir.to_path_buf();
                let mut target = folder.clone();
                if name.trim() != folder.as_str() {
                    if let Ok(t) = worlds::rename(&dir, folder, name) {
                        target = t;
                    }
                }
                let _ = worlds::set_cheats(&dir.join(&target), ctx.player, *cheats);
                return Action::Replace(select_world(&dir));
            }
            if ui.button("Cancel", cx + 2.0, h / 4.0 + 120.0, 98.0, 20.0, true) || back {
                return Action::Pop;
            }
            Action::None
        }

        Screen::ConfirmDelete { folder } => {
            background(ui.p, ctx);
            ui.label("Are you sure you want to delete this world?", cx, 70.0, hud::WHITE);
            ui.label(&format!("'{folder}' will be lost forever! (A long time!)"), cx, 90.0, hud::GRAY);
            if ui.button("Delete", cx - 155.0, h / 6.0 + 96.0, 150.0, 20.0, true) {
                let _ = worlds::delete(ctx.worlds_dir, folder);
                return Action::Replace(select_world(ctx.worlds_dir));
            }
            if ui.button("Cancel", cx + 5.0, h / 6.0 + 96.0, 150.0, 20.0, true) || back {
                return Action::Pop;
            }
            Action::None
        }

        Screen::Multiplayer { selected, scroll } => {
            background(ui.p, ctx);
            title_text(ui, "Play Multiplayer", 16.0);
            let (top, bottom) = (32.0, h - 64.0);
            list_frame(ui.p, top, bottom);
            let row = 36.0;
            let n = ctx.servers.servers.len();
            scroll.update(ui.input, ui.input.mouse.1 >= top && ui.input.mouse.1 < bottom, n as f32 * row + 4.0, bottom - top);
            ui.p.clip(Some([0.0, top, w, bottom - top]));
            let mut join = None;
            for i in 0..n {
                let y = top + 4.0 + i as f32 * row - scroll.offset;
                if y + row < top || y > bottom {
                    continue;
                }
                let x = cx - 152.0;
                let (clicked, hot) = ui.area(x, y, 304.0, row - 4.0);
                if clicked {
                    if *selected == Some(i) && ui.input.double_click {
                        join = Some(i);
                    }
                    *selected = Some(i);
                }
                let sel = *selected == Some(i);
                if sel || hot {
                    ui.p.fill(x - 2.0, y - 2.0, 308.0, row, if sel { [255, 255, 255, 255] } else { [128, 128, 128, 255] });
                    ui.p.fill(x - 1.0, y - 1.0, 306.0, row - 2.0, [0, 0, 0, 255]);
                }
                let s = ctx.servers.servers[i].clone();
                ui.p.sprite_rect("server_icon", x, y, 32.0, 32.0, hud::WHITE);
                ui.p.text(&s.name, x + 35.0, y + 1.0, hud::WHITE);
                match ctx.servers.ping_of(&s.ip) {
                    Some(Ping::Ok { motd, online, max, version, protocol, ms }) => {
                        for (l, line) in motd.lines().take(2).enumerate() {
                            ui.p.text(line, x + 35.0, y + 12.0 + l as f32 * 9.0, [128, 128, 128, 255]);
                        }
                        let ok = protocol == mc_protocol::PROTOCOL_VERSION as i64;
                        let players = if ok { format!("\u{a7}7{online}\u{a7}8/\u{a7}7{max}") } else { format!("\u{a7}c{version}") };
                        let pw = ui.p.text_width(&players);
                        ui.p.text(&players, x + 304.0 - pw - 15.0, y + 1.0, hud::WHITE);
                        let bars = match ms {
                            0..150 => "ping_5",
                            150..300 => "ping_4",
                            300..600 => "ping_3",
                            600..1000 => "ping_2",
                            _ => "ping_1",
                        };
                        ui.p.sprite(if ok { bars } else { "unreachable" }, x + 304.0 - 12.0, y);
                    }
                    Some(Ping::Failed(e)) => {
                        ui.p.text(&format!("\u{a7}4{e}"), x + 35.0, y + 12.0, hud::WHITE);
                        ui.p.sprite("unreachable", x + 304.0 - 12.0, y);
                    }
                    Some(Ping::Pinging) | None => {
                        ui.p.text("Pinging...", x + 35.0, y + 12.0, [128, 128, 128, 255]);
                        ui.p.sprite("ping_unknown", x + 304.0 - 12.0, y);
                    }
                }
            }
            ui.p.clip(None);
            if n == 0 {
                ui.label("No servers yet: Add Server or Direct Connection.", cx, top + 20.0, hud::GRAY);
            }
            let has = selected.is_some_and(|s| s < n);
            if ui.button("Join Server", cx - 154.0, h - 52.0, 100.0, 20.0, has) {
                join = *selected;
            }
            if let Some(i) = join {
                return Action::Connect(ctx.servers.servers[i].ip.clone());
            }
            if ui.button("Direct Connection", cx - 50.0, h - 52.0, 100.0, 20.0, true) {
                return Action::Push(Screen::DirectConnect { ip: ctx.options.last_server.clone() });
            }
            if ui.button("Add Server", cx + 54.0, h - 52.0, 100.0, 20.0, true) {
                return Action::Push(Screen::EditServer { index: None, name: "Minecraft Server".into(), ip: String::new() });
            }
            if ui.button("Edit", cx - 154.0, h - 28.0, 70.0, 20.0, has) {
                let s = &ctx.servers.servers[selected.unwrap()];
                return Action::Push(Screen::EditServer { index: *selected, name: s.name.clone(), ip: s.ip.clone() });
            }
            if ui.button("Delete", cx - 74.0, h - 28.0, 70.0, 20.0, has) {
                ctx.servers.servers.remove(selected.unwrap());
                ctx.servers.save();
                *selected = None;
            }
            if ui.button("Refresh", cx + 4.0, h - 28.0, 70.0, 20.0, true) {
                ctx.servers.refresh();
            }
            if ui.button("Back", cx + 80.0, h - 28.0, 74.0, 20.0, true) || back {
                return Action::Pop;
            }
            Action::None
        }

        Screen::EditServer { index, name, ip } => {
            background(ui.p, ctx);
            title_text(ui, if index.is_some() { "Edit Server Info" } else { "Add Server" }, 17.0);
            ui.p.text("Server Name", cx - 100.0, 53.0, hud::GRAY);
            ui.text_field(name, "", cx - 100.0, 66.0, 200.0, 20.0, 64);
            ui.p.text("Server Address", cx - 100.0, 94.0, hud::GRAY);
            let enter = ui.text_field(ip, "", cx - 100.0, 106.0, 200.0, 20.0, 128);
            let ok = !ip.trim().is_empty();
            if ui.button("Done", cx - 100.0, h / 4.0 + 96.0 + 18.0, 200.0, 20.0, ok) || (enter && ok) {
                let s = Server { name: if name.trim().is_empty() { "Minecraft Server".into() } else { name.clone() }, ip: ip.trim().to_string() };
                match index {
                    Some(i) if *i < ctx.servers.servers.len() => ctx.servers.servers[*i] = s,
                    _ => ctx.servers.servers.push(s),
                }
                ctx.servers.save();
                ctx.servers.refresh();
                return Action::Pop;
            }
            if ui.button("Cancel", cx - 100.0, h / 4.0 + 120.0 + 18.0, 200.0, 20.0, true) || back {
                return Action::Pop;
            }
            Action::None
        }

        Screen::DirectConnect { ip } => {
            background(ui.p, ctx);
            title_text(ui, "Direct Connection", 20.0);
            ui.p.text("Server Address", cx - 100.0, 100.0, hud::GRAY);
            let enter = ui.text_field(ip, "", cx - 100.0, 116.0, 200.0, 20.0, 128);
            let ok = !ip.trim().is_empty();
            if ui.button("Join Server", cx - 100.0, h / 4.0 + 96.0 + 12.0, 200.0, 20.0, ok) || (enter && ok) {
                ctx.options.last_server = ip.trim().to_string();
                return Action::Connect(ip.trim().to_string());
            }
            if ui.button("Cancel", cx - 100.0, h / 4.0 + 120.0 + 12.0, 200.0, 20.0, true) || back {
                return Action::Pop;
            }
            Action::None
        }

        Screen::Connecting { target } => {
            background(ui.p, ctx);
            ui.label("Connecting to the server...", cx, h / 2.0 - 50.0, hud::WHITE);
            ui.label(target, cx, h / 2.0 - 36.0, hud::GRAY);
            if !ctx.status.is_empty() {
                ui.label(ctx.status, cx, h / 2.0 - 20.0, hud::GRAY);
            }
            if ui.button("Cancel", cx - 100.0, h / 4.0 + 120.0 + 12.0, 200.0, 20.0, true) || back {
                return Action::Disconnect;
            }
            Action::None
        }

        Screen::Loading { title } => {
            background(ui.p, ctx);
            ui.label(title, cx, h / 2.0 - 30.0, hud::WHITE);
            ui.label(ctx.status, cx, h / 2.0 - 14.0, hud::GRAY);
            // A little activity bar so it doesn't look frozen.
            let t = (ctx.time * 1.5).fract();
            ui.p.fill(cx - 50.0, h / 2.0 + 4.0, 100.0, 2.0, [60, 60, 60, 255]);
            ui.p.fill(cx - 50.0 + t * 80.0, h / 2.0 + 4.0, 20.0, 2.0, [128, 255, 128, 255]);
            Action::None
        }

        Screen::Message { title, text, back_to_servers } => {
            background(ui.p, ctx);
            let lines = wrap(ui.p, text, w - 50.0);
            let total = lines.len() as f32 * 9.0;
            let y = h / 2.0 - total / 2.0 - 9.0 * 2.0;
            ui.label(title, cx, y, hud::GRAY);
            for (i, l) in lines.iter().enumerate() {
                ui.label(l, cx, y + 18.0 + i as f32 * 9.0, hud::WHITE);
            }
            let by = (y + 18.0 + total + 12.0).min(h - 30.0);
            let label = if *back_to_servers { "Back to Server List" } else { "Back to Title Screen" };
            if ui.button(label, cx - 100.0, by, 200.0, 20.0, true) || back {
                if *back_to_servers {
                    return Action::Pop;
                }
                return Action::Title;
            }
            Action::None
        }

        Screen::Options => {
            background(ui.p, ctx);
            title_text(ui, "Options", 15.0);
            let y = (h / 6.0 - 12.0).floor();
            let o = &mut *ctx.options;
            let mut fov = (o.fov - 30.0) / 80.0;
            let label = if o.fov.round() == 70.0 { "FOV: Normal".into() } else if o.fov >= 110.0 { "FOV: Quake Pro".into() } else { format!("FOV: {}", o.fov.round()) };
            let mut changed = false;
            if ui.slider(&mut fov, &label, cx - 155.0, y, 150.0, Some(80)) {
                o.fov = 30.0 + fov * 80.0;
                changed = true;
            }
            let mut act = Action::None;
            if ctx.in_world && ctx.singleplayer {
                let mut d = ctx.difficulty as usize;
                if ui.cycle("Difficulty", &worlds::DIFFICULTIES, &mut d, cx + 5.0, y, 150.0, true) {
                    act = Action::Command(format!("/difficulty {}", ["peaceful", "easy", "normal", "hard"][d]));
                }
            } else {
                ui.button("Online...", cx + 5.0, y, 150.0, 20.0, false);
            }
            let rows = [
                ("Skin Customization...", None, "Music & Sounds...", Some(Screen::Sound)),
                ("Video Settings...", Some(Screen::Video), "Controls...", Some(Screen::Controls)),
                ("Language...", Some(Screen::Language), "Chat Settings...", Some(Screen::Chat)),
                ("Resource Packs...", None, "Accessibility Settings...", Some(Screen::Accessibility)),
                ("Telemetry Data...", None, "Controller...", Some(Screen::Controller)),
            ];
            for (i, (l, ls, r, rs)) in rows.into_iter().enumerate() {
                let ry = h / 6.0 + 48.0 - 6.0 + i as f32 * 24.0;
                let enabled_l = ls.is_some();
                if ui.button(l, cx - 155.0, ry, 150.0, 20.0, enabled_l) {
                    return Action::Push(ls.unwrap());
                }
                let enabled_r = rs.is_some();
                if ui.button(r, cx + 5.0, ry, 150.0, 20.0, enabled_r) {
                    return Action::Push(rs.unwrap());
                }
            }
            if ui.button("Done", cx - 100.0, h / 6.0 + 168.0, 200.0, 20.0, true) || back {
                return Action::Pop;
            }
            if changed { Action::OptionsChanged } else { act }
        }

        Screen::Video => {
            background(ui.p, ctx);
            title_text(ui, "Video Settings", 15.0);
            let o = &mut *ctx.options;
            let mut changed = false;
            let col = |i: usize| if i % 2 == 0 { cx - 155.0 } else { cx + 5.0 };
            let row = |i: usize| 32.0 + (i / 2) as f32 * 24.0;
            let mut i = 0;
            let mut rd = (o.render_distance as f32 - 2.0) / 30.0;
            if ui.slider(&mut rd, &format!("Render Distance: {} chunks", o.render_distance), col(i), row(i), 150.0, Some(30)) {
                o.render_distance = (2.0 + rd * 30.0).round() as u8;
                changed = true;
            }
            i += 1;
            let mut fps = (o.max_fps as f32 - 10.0) / 250.0;
            if ui.slider(&mut fps, &format!("Max Framerate: {}", o.fps_label()), col(i), row(i), 150.0, Some(25)) {
                o.max_fps = (10.0 + fps * 250.0).round() as u32;
                changed = true;
            }
            i += 1;
            changed |= ui.toggle("VSync", &mut o.vsync, col(i), row(i), 150.0);
            i += 1;
            let mut scale = o.gui_scale as usize;
            changed |= ui.cycle("GUI Scale", &["Auto", "1", "2", "3", "4", "5", "6"], &mut scale, col(i), row(i), 150.0, true);
            o.gui_scale = scale as u32;
            i += 1;
            let label = match (o.gamma * 100.0).round() as i32 {
                0 => "Brightness: Moody".to_string(),
                100 => "Brightness: Bright".to_string(),
                v => format!("Brightness: +{v}%"),
            };
            changed |= ui.slider(&mut o.gamma, &label, col(i), row(i), 150.0, Some(100));
            i += 1;
            changed |= ui.toggle("View Bobbing", &mut o.view_bobbing, col(i), row(i), 150.0);
            i += 1;
            changed |= ui.toggle("Fullscreen", &mut o.fullscreen, col(i), row(i), 150.0);
            i += 1;
            changed |= ui.toggle("Clouds", &mut o.clouds, col(i), row(i), 150.0);
            i += 1;
            changed |= ui.toggle("Attack Indicator", &mut o.attack_indicator, col(i), row(i), 150.0);
            i += 1;
            let mut fov = (o.fov - 30.0) / 80.0;
            if ui.slider(&mut fov, &format!("FOV: {}", o.fov.round()), col(i), row(i), 150.0, Some(80)) {
                o.fov = 30.0 + fov * 80.0;
                changed = true;
            }
            i += 1;
            changed |= ui.toggle("Cave Culling", &mut o.cave_culling, col(i), row(i), 150.0);
            i += 1;
            changed |= ui.toggle("Entity Culling", &mut o.entity_culling, col(i), row(i), 150.0);
            i += 1;
            changed |= ui.toggle("FPS Graph", &mut o.fps_graph, col(i), row(i), 150.0);
            if ui.button("Done", cx - 100.0, h - 27.0, 200.0, 20.0, true) || back {
                return Action::Pop;
            }
            if changed { Action::OptionsChanged } else { Action::None }
        }

        Screen::Sound => {
            background(ui.p, ctx);
            title_text(ui, "Music & Sound Options", 15.0);
            const LABELS: [&str; 11] = ["Master Volume", "Music", "Jukebox/Note Blocks", "Weather", "Blocks", "Hostile Creatures", "Friendly Creatures", "Players", "Ambient/Environment", "Voice/Speech", "UI"];
            let o = &mut *ctx.options;
            let mut changed = false;
            let pct = |v: f32| if v <= 0.0 { "OFF".to_string() } else { format!("{}%", (v * 100.0).round()) };
            let v = o.volumes[0];
            changed |= ui.slider(&mut o.volumes[0], &format!("{}: {}", LABELS[0], pct(v)), cx - 155.0, 32.0, 310.0, Some(100));
            for i in 1..11 {
                let (x, y) = (if i % 2 == 1 { cx - 155.0 } else { cx + 5.0 }, 32.0 + 24.0 * ((i + 1) / 2) as f32);
                let v = o.volumes[i];
                changed |= ui.slider(&mut o.volumes[i], &format!("{}: {}", LABELS[i], pct(v)), x, y, 150.0, Some(100));
            }
            if ui.button("Done", cx - 100.0, h - 27.0, 200.0, 20.0, true) || back {
                return Action::Pop;
            }
            if changed { Action::OptionsChanged } else { Action::None }
        }

        Screen::Controls => {
            background(ui.p, ctx);
            title_text(ui, "Controls", 15.0);
            let y = h / 6.0;
            if ui.button("Mouse Settings...", cx - 155.0, y, 150.0, 20.0, true) {
                return Action::Push(Screen::Mouse);
            }
            if ui.button("Key Binds...", cx + 5.0, y, 150.0, 20.0, true) {
                return Action::Push(Screen::KeyBinds { waiting: None, scroll: Scroll::default() });
            }
            if ui.button("Controller...", cx - 155.0, y + 24.0, 150.0, 20.0, true) {
                return Action::Push(Screen::Controller);
            }
            if ui.button("Done", cx - 100.0, h - 27.0, 200.0, 20.0, true) || back {
                return Action::Pop;
            }
            Action::None
        }

        Screen::Mouse => {
            background(ui.p, ctx);
            title_text(ui, "Mouse Settings", 15.0);
            let o = &mut *ctx.options;
            let mut changed = false;
            let label = format!("Sensitivity: {}%", (o.sensitivity * 200.0).round());
            changed |= ui.slider(&mut o.sensitivity, &label, cx - 155.0, 32.0, 150.0, Some(200));
            changed |= ui.toggle("Invert Mouse", &mut o.invert_mouse, cx + 5.0, 32.0, 150.0);
            if ui.button("Done", cx - 100.0, h - 27.0, 200.0, 20.0, true) || back {
                return Action::Pop;
            }
            if changed { Action::OptionsChanged } else { Action::None }
        }

        Screen::KeyBinds { waiting, scroll } => {
            background(ui.p, ctx);
            title_text(ui, "Key Binds", 15.0);
            let o = &mut *ctx.options;
            let mut changed = false;
            if let Some(i) = *waiting {
                // The next key pressed becomes the bind (Escape cancels).
                if let Some(&k) = ui.input.keys.first() {
                    if k != winit::keyboard::KeyCode::Escape {
                        o.keys[i] = k;
                        changed = true;
                    }
                    *waiting = None;
                }
            }
            let (top, bottom) = (32.0, h - 32.0);
            list_frame(ui.p, top, bottom);
            let row = 24.0;
            scroll.update(ui.input, true, Key::ALL.len() as f32 * row + 8.0, bottom - top);
            ui.p.clip(Some([0.0, top, w, bottom - top]));
            for (i, a) in Key::ALL.iter().enumerate() {
                let y = top + 4.0 + i as f32 * row - scroll.offset;
                if y + 20.0 < top || y > bottom {
                    continue;
                }
                let (_, name, default) = a.info();
                ui.p.text(name, cx - 150.0, y + 6.0, hud::WHITE);
                let conflict = o.keys.iter().enumerate().any(|(j, k)| j != i && *k == o.keys[i]);
                let label = if *waiting == Some(i) {
                    format!("> \u{a7}e{} \u{a7}f<", key_name(o.keys[i]))
                } else if conflict {
                    format!("\u{a7}c{}", key_name(o.keys[i]))
                } else {
                    key_name(o.keys[i])
                };
                if ui.button(&label, cx + 20.0, y, 75.0, 20.0, true) && waiting.is_none() {
                    *waiting = Some(i);
                }
                if ui.button("Reset", cx + 100.0, y, 50.0, 20.0, o.keys[i] != default) {
                    o.keys[i] = default;
                    changed = true;
                }
            }
            ui.p.clip(None);
            if ui.button("Reset Keys", cx - 155.0, h - 27.0, 150.0, 20.0, true) {
                for (i, a) in Key::ALL.iter().enumerate() {
                    o.keys[i] = a.info().2;
                }
                changed = true;
            }
            if ui.button("Done", cx + 5.0, h - 27.0, 150.0, 20.0, true) || (back && waiting.is_none()) {
                return Action::Pop;
            }
            if changed { Action::OptionsChanged } else { Action::None }
        }

        Screen::Chat => {
            background(ui.p, ctx);
            title_text(ui, "Chat Settings", 15.0);
            let o = &mut *ctx.options;
            let mut changed = false;
            let l = format!("Chat Text Opacity: {}%", (o.chat_opacity * 100.0).round());
            changed |= ui.slider(&mut o.chat_opacity, &l, cx - 155.0, 32.0, 150.0, Some(100));
            let l = format!("Text Background Opacity: {}%", (o.chat_background * 100.0).round());
            changed |= ui.slider(&mut o.chat_background, &l, cx + 5.0, 32.0, 150.0, Some(100));
            if ui.button("Done", cx - 100.0, h - 27.0, 200.0, 20.0, true) || back {
                return Action::Pop;
            }
            if changed { Action::OptionsChanged } else { Action::None }
        }

        Screen::Controller => {
            background(ui.p, ctx);
            title_text(ui, "Controller", 15.0);
            let o = &mut *ctx.options;
            let l = format!("Look Sensitivity: {}%", (o.controller_sensitivity * 200.0).round());
            let changed = ui.slider(&mut o.controller_sensitivity, &l, cx - 100.0, 32.0, 200.0, Some(200));
            let lines = [
                "Left stick: move      Right stick: look",
                "A: jump (double-tap: fly)   B: back",
                "RT: mine / attack     LT: use / place",
                "LB / RB: hotbar      Y: inventory",
                "L3: sprint      R3: sneak      Start: pause",
                "Menus: D-pad or stick to move, A to press",
                "Inventory: stick moves the cursor, A/X click",
            ];
            for (i, l) in lines.iter().enumerate() {
                ui.label(l, cx, 64.0 + i as f32 * 12.0, hud::GRAY);
            }
            if ui.button("Done", cx - 100.0, h - 27.0, 200.0, 20.0, true) || back {
                return Action::Pop;
            }
            if changed { Action::OptionsChanged } else { Action::None }
        }

        Screen::Accessibility => {
            background(ui.p, ctx);
            title_text(ui, "Accessibility Settings", 15.0);
            let o = &mut *ctx.options;
            let mut changed = false;
            let mut show = !o.hide_splashes;
            if ui.toggle("Splash Texts", &mut show, cx - 155.0, 32.0, 150.0) {
                o.hide_splashes = !show;
                changed = true;
            }
            changed |= ui.toggle("View Bobbing", &mut o.view_bobbing, cx + 5.0, 32.0, 150.0);
            let l = format!("Text Background Opacity: {}%", (o.chat_background * 100.0).round());
            changed |= ui.slider(&mut o.chat_background, &l, cx - 155.0, 56.0, 150.0, Some(100));
            if ui.button("Done", cx - 100.0, h - 27.0, 200.0, 20.0, true) || back {
                return Action::Pop;
            }
            if changed { Action::OptionsChanged } else { Action::None }
        }

        Screen::Language => {
            background(ui.p, ctx);
            title_text(ui, "Language", 15.0);
            list_frame(ui.p, 32.0, h - 40.0);
            ui.p.fill(cx - 110.0, 38.0, 220.0, 18.0, [255, 255, 255, 255]);
            ui.p.fill(cx - 109.0, 39.0, 218.0, 16.0, [0, 0, 0, 255]);
            ui.label("English (US)", cx, 43.0, hud::WHITE);
            ui.label("Other languages need the font pages; English only for now.", cx, h - 52.0, hud::GRAY);
            if ui.button("Done", cx - 100.0, h - 27.0, 200.0, 20.0, true) || back {
                return Action::Pop;
            }
            Action::None
        }

        Screen::Pause => {
            ui.p.gradient(0.0, 0.0, w, h, [16, 16, 16, 192], [16, 16, 16, 208]);
            title_text(ui, "Game Menu", 40.0);
            let y = (h / 4.0 + 8.0).floor();
            if ui.button("Back to Game", cx - 102.0, y, 204.0, 20.0, true) || back {
                return Action::Pop;
            }
            ui.button("Advancements", cx - 102.0, y + 24.0, 98.0, 20.0, false);
            ui.button("Statistics", cx + 4.0, y + 24.0, 98.0, 20.0, false);
            ui.button("Send Feedback", cx - 102.0, y + 48.0, 98.0, 20.0, false);
            ui.button("Report Bugs", cx + 4.0, y + 48.0, 98.0, 20.0, false);
            if ui.button("Options...", cx - 102.0, y + 72.0, 98.0, 20.0, true) {
                return Action::Push(Screen::Options);
            }
            ui.button("Open to LAN", cx + 4.0, y + 72.0, 98.0, 20.0, false);
            let quit = if ctx.singleplayer { "Save and Quit to Title" } else { "Disconnect" };
            if ui.button(quit, cx - 102.0, y + 96.0 + 8.0, 204.0, 20.0, true) {
                return Action::Disconnect;
            }
            Action::None
        }

        Screen::Death { message, score, hardcore, since } => {
            ui.p.gradient(0.0, 0.0, w, h, [80, 0, 0, 96], [128, 48, 48, 160]);
            let t = if *hardcore { "Game Over!" } else { "You Died!" };
            let tw = ui.p.text_width(t) * 2.0;
            ui.p.text_ex(t, cx - tw / 2.0, 30.0, hud::WHITE, 2.0, true);
            ui.label(message, cx, 85.0, hud::WHITE);
            ui.label(&format!("Score: \u{a7}e{score}"), cx, 100.0, hud::WHITE);
            // Vanilla enables the buttons after a second so a held click doesn't skip the screen.
            let ready = since.elapsed().as_secs_f32() > 1.0;
            let first = if *hardcore { "Spectate World" } else { "Respawn" };
            if ui.button(first, cx - 100.0, h / 4.0 + 72.0, 200.0, 20.0, ready) {
                return Action::Respawn;
            }
            if ui.button("Title Screen", cx - 100.0, h / 4.0 + 96.0, 200.0, 20.0, ready) {
                return Action::Disconnect;
            }
            Action::None
        }
    }
}

fn icon_button(ui: &mut Ui, icon: &str, x: f32, y: f32) -> bool {
    let pressed = ui.button("", x, y, 20.0, 20.0, true);
    ui.p.sprite(icon, x + 2.0, y + 2.0);
    pressed
}

/// Splits text into lines that fit `width` GUI pixels.
pub fn wrap(p: &Painter, text: &str, width: f32) -> Vec<String> {
    let mut lines = Vec::new();
    for para in text.lines() {
        let mut line = String::new();
        for word in para.split(' ') {
            let candidate = if line.is_empty() { word.to_string() } else { format!("{line} {word}") };
            if p.text_width(&candidate) > width && !line.is_empty() {
                lines.push(std::mem::take(&mut line));
                line = word.to_string();
            } else {
                line = candidate;
            }
        }
        lines.push(line);
    }
    lines
}
