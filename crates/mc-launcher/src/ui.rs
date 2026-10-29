//! The launcher window: sidebar, play screen with animated voxel backdrop, versions, account, settings.

use crate::install::{self, Paths, Prepared, Progress};
use crate::launch::{self, Account, valid_name};
use crate::meta::Manifest;
use crate::settings::Settings;
use crate::singleplayer;
use crate::theme::{self, *};
use eframe::egui::*;
use std::process::Child;
use std::sync::atomic::Ordering::Relaxed;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(PartialEq, Clone, Copy)]
enum Page {
    Play,
    Versions,
    Account,
    Settings,
}

enum State {
    Idle,
    Working,
    Running,
    Failed(String),
}

enum Msg {
    Manifest(Result<Manifest, String>),
    Prepared(Result<Prepared, String>),
    Singleplayer(Result<singleplayer::Running, String>),
}

pub struct App {
    paths: Paths,
    settings: Settings,
    saved: Settings,
    manifest: Option<Manifest>,
    manifest_err: Option<String>,
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    progress: Arc<Progress>,
    state: State,
    page: Page,
    child: Option<Child>,
    /// A running singleplayer world (server + Rust client).
    world: Option<singleplayer::Running>,
    speed: f64,
    sample: (Instant, u64),
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        theme::install(&cc.egui_ctx);
        let paths = Paths::new();
        let settings = Settings::load(&paths.root);
        let (tx, rx) = channel();
        let app = Self {
            paths,
            saved: settings.clone(),
            settings,
            manifest: None,
            manifest_err: None,
            tx,
            rx,
            progress: Arc::new(Progress::default()),
            state: State::Idle,
            page: Page::Play,
            child: None,
            world: None,
            speed: 0.0,
            sample: (Instant::now(), 0),
        };
        app.fetch_manifest(&cc.egui_ctx);
        app
    }

    fn fetch_manifest(&self, ctx: &Context) {
        let (tx, ctx) = (self.tx.clone(), ctx.clone());
        std::thread::spawn(move || {
            let r = install::fetch_manifest().map_err(|e| format!("{e:#}"));
            let _ = tx.send(Msg::Manifest(r));
            ctx.request_repaint();
        });
    }

    fn selected_id(&self) -> String {
        if !self.settings.selected.is_empty() {
            return self.settings.selected.clone();
        }
        self.manifest.as_ref().map(|m| m.latest.release.clone()).unwrap_or_else(|| "26.3".into())
    }

    /// Installs the selected version, then launches the Rust client in singleplayer (`rust`) or the Java game.
    fn start_play(&mut self, ctx: &Context, rust: bool) {
        if !valid_name(&self.settings.username) {
            self.page = Page::Account;
            return;
        }
        let id = self.selected_id();
        self.progress.cancel.store(false, Relaxed);
        self.progress.reset_counts();
        self.progress.set_stage("Starting");
        self.speed = 0.0;
        self.sample = (Instant::now(), 0);
        self.state = State::Working;
        let (paths, manifest, progress, tx, ctx) =
            (self.paths.clone(), self.manifest.clone(), self.progress.clone(), self.tx.clone(), ctx.clone());
        let (name, world, eula) = (self.settings.username.clone(), self.settings.world.clone(), self.settings.eula_accepted);
        std::thread::spawn(move || {
            let r = install::prepare(&paths, &id, manifest.as_ref(), &progress).map_err(|e| format!("{e:#}"));
            let msg = match r {
                Ok(p) if rust => {
                    Msg::Singleplayer(singleplayer::start(&paths, &p, &name, &world, eula, &progress).map_err(|e| format!("{e:#}")))
                }
                r => Msg::Prepared(r),
            };
            let _ = tx.send(msg);
            ctx.request_repaint();
        });
    }

    fn drain_messages(&mut self) {
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                Msg::Manifest(Ok(m)) => {
                    self.manifest = Some(m);
                    self.manifest_err = None;
                }
                Msg::Manifest(Err(e)) => self.manifest_err = Some(e),
                Msg::Prepared(Err(e)) | Msg::Singleplayer(Err(e)) => {
                    self.state = if e == "cancelled" { State::Idle } else { State::Failed(e) };
                }
                Msg::Singleplayer(Ok(running)) => {
                    self.world = Some(running);
                    self.state = State::Running;
                }
                Msg::Prepared(Ok(p)) => {
                    let account = Account::offline(&self.settings.username);
                    let java = (!self.settings.java_override.trim().is_empty()).then_some(self.settings.java_override.as_str());
                    match launch::launch(&self.paths, &p, &account, self.settings.memory_mb, java) {
                        Ok(child) => {
                            self.child = Some(child);
                            self.state = State::Running;
                        }
                        Err(e) => self.state = State::Failed(format!("Couldn't start Java: {e:#}")),
                    }
                }
            }
        }
    }

    fn poll_child(&mut self) {
        if let Some(w) = &mut self.world {
            if let Ok(Some(status)) = w.client.try_wait() {
                let running = self.world.take().unwrap();
                // Save and stop the world in the background so the launcher stays responsive.
                std::thread::spawn(move || singleplayer::stop(running));
                self.state = if status.success() {
                    State::Idle
                } else {
                    State::Failed(format!("The Rust client exited with {status}. See game/logs/mcrs-client.log in the data folder."))
                };
            } else if let Ok(Some(status)) = w.server.try_wait() {
                let log = w.world_dir.join("logs").join("latest.log");
                let _ = w.client.kill();
                self.world = None;
                self.state = State::Failed(format!("The world server stopped ({status}). See {}", log.display()));
            }
        }
        let Some(child) = &mut self.child else { return };
        if let Ok(Some(status)) = child.try_wait() {
            self.child = None;
            self.state = if status.success() {
                State::Idle
            } else {
                let code = status.code().map_or("unknown".into(), |c| c.to_string());
                State::Failed(format!("Minecraft exited with code {code}. See game/logs/mcrs-launch.log in the data folder."))
            };
        }
    }

    fn sample_speed(&mut self) {
        let now = Instant::now();
        let done = self.progress.bytes_done.load(Relaxed);
        let dt = now.duration_since(self.sample.0).as_secs_f64();
        if dt >= 0.5 {
            let inst = done.saturating_sub(self.sample.1) as f64 / dt;
            self.speed = if self.speed == 0.0 { inst } else { self.speed * 0.6 + inst * 0.4 };
            self.sample = (now, done);
        }
    }

    fn frac(&self) -> f32 {
        let total = self.progress.bytes_total.load(Relaxed);
        if total == 0 { 0.0 } else { (self.progress.bytes_done.load(Relaxed) as f64 / total as f64).min(1.0) as f32 }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.drain_messages();
        self.poll_child();
        if matches!(self.state, State::Working) {
            self.sample_speed();
            ctx.request_repaint_after(Duration::from_millis(100));
        } else if matches!(self.state, State::Running) {
            ctx.request_repaint_after(Duration::from_millis(500));
        }

        Panel::left("sidebar")
            .exact_size(210.0)
            .resizable(false)
            .show_separator_line(false)
            .frame(Frame::new().fill(SIDEBAR).inner_margin(Margin::same(16)))
            .show(ui, |ui| self.sidebar(ui));

        CentralPanel::default()
            .frame(Frame::new().fill(BG))
            .show(ui, |ui| match self.page {
                Page::Play => self.play_page(ui, &ctx),
                Page::Versions => self.page_frame(ui, "Versions", |s, ui| s.versions_page(ui)),
                Page::Account => self.page_frame(ui, "Account", |s, ui| s.account_page(ui)),
                Page::Settings => self.page_frame(ui, "Settings", |s, ui| s.settings_page(ui)),
            });

        if self.settings != self.saved {
            self.settings.save(&self.paths.root);
            self.saved = self.settings.clone();
        }
    }
}

// ---------- sidebar ----------

impl App {
    fn sidebar(&mut self, ui: &mut Ui) {
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            let (r, _) = ui.allocate_exact_size(vec2(30.0, 30.0), Sense::hover());
            let p = ui.painter();
            p.rect_filled(r, CornerRadius::same(9), ACCENT);
            p.rect_filled(r.shrink(8.0), CornerRadius::same(3), SIDEBAR);
            ui.label(bold("mcrs", 21.0, TEXT));
        });
        ui.add_space(22.0);
        for (page, label) in [(Page::Play, "Play"), (Page::Versions, "Versions"), (Page::Account, "Account"), (Page::Settings, "Settings")] {
            if nav_item(ui, label, self.page == page).clicked() {
                self.page = page;
            }
        }
        ui.with_layout(Layout::bottom_up(Align::LEFT), |ui| {
            ui.add_space(4.0);
            ui.label(text(format!("v{}", env!("CARGO_PKG_VERSION")), 12.0, MUTED));
            let (dot, label) = match (&self.manifest, &self.manifest_err) {
                (Some(_), _) => (ACCENT, "Online"),
                (None, Some(_)) => (DANGER, "Offline"),
                _ => (MUTED, "Connecting"),
            };
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(vec2(8.0, 8.0), Sense::hover());
                ui.painter().circle_filled(r.center(), 4.0, dot);
                ui.label(text(label, 12.5, MUTED));
            });
        });
    }
}

fn nav_item(ui: &mut Ui, label: &str, selected: bool) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 42.0), Sense::click());
    let t = ui.ctx().animate_bool_with_time(resp.id, resp.hovered(), 0.12);
    let p = ui.painter();
    if selected {
        p.rect_filled(rect, CornerRadius::same(10), CARD);
        p.rect_filled(Rect::from_min_size(rect.left_center() - vec2(0.0, 10.0), vec2(3.0, 20.0)), CornerRadius::same(2), ACCENT);
    } else if t > 0.0 {
        p.rect_filled(rect, CornerRadius::same(10), CARD.gamma_multiply(t * 0.7));
    }
    let color = if selected { TEXT } else { MUTED.lerp_to_gamma(TEXT, t) };
    p.text(rect.left_center() + vec2(18.0, 0.0), Align2::LEFT_CENTER, label, FontId::new(15.5, theme::bold_family()), color);
    resp.on_hover_cursor(CursorIcon::PointingHand)
}

// ---------- play page ----------

impl App {
    fn play_page(&mut self, ui: &mut Ui, ctx: &Context) {
        let rect = ui.max_rect();
        let t = ui.input(|i| i.time) as f32;
        let painter = ui.painter_at(rect);

        let mut mesh = Mesh::default();
        let bottom_glow = ACCENT_DEEP.gamma_multiply(0.35);
        mesh.colored_vertex(rect.left_top(), BG);
        mesh.colored_vertex(rect.right_top(), BG);
        mesh.colored_vertex(rect.right_bottom(), bottom_glow);
        mesh.colored_vertex(rect.left_bottom(), BG);
        mesh.add_triangle(0, 1, 2);
        mesh.add_triangle(0, 2, 3);
        painter.add(Shape::mesh(mesh));
        draw_voxels(&painter, pos2(rect.right() - 300.0, rect.bottom() - 190.0), t);
        ctx.request_repaint_after(Duration::from_millis(33));

        let inner = Rect::from_min_size(rect.min + vec2(56.0, 56.0), vec2((rect.width() - 112.0).max(320.0), rect.height() - 112.0));
        ui.scope_builder(UiBuilder::new().max_rect(inner), |ui| {
            let id = self.selected_id();
            let kind = self
                .manifest
                .as_ref()
                .and_then(|m| m.versions.iter().find(|v| v.id == id))
                .map(|v| v.kind.clone())
                .unwrap_or_else(|| "release".into());
            let installed = self.paths.is_installed(&id);

            ui.label(text("MINECRAFT  ·  RUST EDITION", 12.5, ACCENT));
            ui.add_space(2.0);
            ui.label(bold(&id, 84.0, TEXT));
            ui.horizontal(|ui| {
                pill(ui, &kind, MUTED);
                pill(ui, if installed { "Installed" } else { "Not installed" }, if installed { ACCENT } else { MUTED });
                if let Some(m) = &self.manifest {
                    if m.latest.release == id {
                        pill(ui, "Latest", ACCENT);
                    }
                }
            });
            ui.add_space(26.0);

            // The Rust client speaks protocol 777, i.e. 26.3 only.
            let rust_ready = id == "26.3" && singleplayer::client_exe().is_some();
            match &self.state {
                State::Idle | State::Failed(_) => {
                    if rust_ready {
                        let label = if installed { "PLAY" } else { "INSTALL & PLAY" };
                        let accepted = self.settings.eula_accepted;
                        if play_button(ui, label, None).clicked() && accepted {
                            self.start_play(ctx, true);
                        }
                        ui.add_space(6.0);
                        ui.horizontal(|ui| {
                            ui.checkbox(&mut self.settings.eula_accepted, "");
                            ui.label(text("I accept the", 13.0, MUTED));
                            ui.hyperlink_to(text("Minecraft EULA", 13.0, ACCENT), "https://aka.ms/MinecraftEULA");
                            ui.label(text("(runs your world locally)", 13.0, MUTED));
                        });
                        ui.label(text(format!("Singleplayer world \"{}\" in the Rust client", self.settings.world), 13.0, MUTED));
                        if ui.add(Button::new(text("Play Java Edition instead", 13.0, MUTED)).frame(false)).on_hover_cursor(CursorIcon::PointingHand).clicked() {
                            self.start_play(ctx, false);
                        }
                    } else {
                        let label = if installed { "PLAY JAVA" } else { "INSTALL & PLAY JAVA" };
                        if play_button(ui, label, None).clicked() {
                            self.start_play(ctx, false);
                        }
                        ui.label(text("The Rust client supports 26.3; other versions launch the Java game.", 13.0, MUTED));
                    }
                    if let State::Failed(msg) = &self.state {
                        ui.add_space(6.0);
                        ui.add(Label::new(text(msg, 13.5, DANGER)).wrap());
                    }
                }
                State::Working => {
                    let frac = self.frac();
                    play_button(ui, &format!("{:.0}%", frac * 100.0), Some(frac));
                    self.progress_card(ui);
                }
                State::Running => {
                    play_button(ui, "RUNNING", Some(1.0));
                    let note = if self.world.is_some() {
                        "Playing in the Rust client. Closing its window saves and stops the world."
                    } else {
                        "Minecraft is running. This window stays out of the way."
                    };
                    ui.label(text(note, 13.5, MUTED));
                }
            }
        });

        let who = Rect::from_min_size(pos2(rect.left() + 56.0, rect.bottom() - 84.0), vec2(300.0, 44.0));
        ui.scope_builder(UiBuilder::new().max_rect(who), |ui| {
            ui.horizontal(|ui| {
                let (r, resp) = ui.allocate_exact_size(vec2(40.0, 40.0), Sense::click());
                ui.painter().circle_filled(r.center(), 20.0, CARD_HOVER);
                let initial = self.settings.username.chars().next().unwrap_or('?').to_uppercase().to_string();
                ui.painter().text(r.center(), Align2::CENTER_CENTER, initial, FontId::new(18.0, theme::bold_family()), ACCENT);
                if resp.on_hover_cursor(CursorIcon::PointingHand).clicked() {
                    self.page = Page::Account;
                }
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    ui.label(bold(&self.settings.username, 15.0, TEXT));
                    ui.label(text("Offline account", 12.5, MUTED));
                });
            });
        });
    }

    fn progress_card(&mut self, ui: &mut Ui) {
        let stage = self.progress.stage.lock().unwrap().clone();
        let (fd, ft) = (self.progress.files_done.load(Relaxed), self.progress.files_total.load(Relaxed));
        let (bd, bt) = (self.progress.bytes_done.load(Relaxed), self.progress.bytes_total.load(Relaxed));
        let mb = |b: u64| b as f64 / 1_048_576.0;
        let eta = if self.speed > 1.0 && bt > bd { format!("  ·  {}", fmt_eta((bt - bd) as f64 / self.speed)) } else { String::new() };
        ui.add_space(8.0);
        ui.label(bold(&stage, 15.0, TEXT));
        let detail = if ft > 0 {
            format!("{fd} / {ft} files  ·  {:.0} / {:.0} MB  ·  {:.1} MB/s{eta}", mb(bd), mb(bt), self.speed / 1_048_576.0)
        } else {
            "Checking files".to_string()
        };
        ui.label(text(detail, 13.0, MUTED));
        ui.add_space(2.0);
        if ui.add(Button::new(text("Cancel", 13.5, MUTED)).frame(false)).on_hover_cursor(CursorIcon::PointingHand).clicked() {
            self.progress.cancel.store(true, Relaxed);
        }
    }
}

fn fmt_eta(secs: f64) -> String {
    let s = secs as u64;
    if s >= 3600 { format!("{}h {}m left", s / 3600, (s % 3600) / 60) }
    else if s >= 60 { format!("{}m {}s left", s / 60, s % 60) }
    else { format!("{s}s left") }
}

fn pill(ui: &mut Ui, label: &str, color: Color32) {
    let galley = ui.painter().layout_no_wrap(label.to_string(), FontId::new(13.0, theme::bold_family()), color);
    let size = galley.size() + vec2(20.0, 10.0);
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    ui.painter().rect_filled(rect, CornerRadius::same(255), color.gamma_multiply(0.14));
    ui.painter().galley(rect.center() - galley.size() / 2.0, galley, color);
}

fn play_button(ui: &mut Ui, label: &str, progress: Option<f32>) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(280.0, 64.0), Sense::click());
    let enabled = progress.is_none();
    let hover = ui.ctx().animate_bool_with_time(resp.id, resp.hovered() && enabled, 0.12);
    let p = ui.painter();
    let radius = CornerRadius::same(16);
    match progress {
        None => {
            let fill = ACCENT.lerp_to_gamma(Color32::WHITE, hover * 0.18);
            p.rect_filled(rect.expand(hover * 3.0).expand(6.0), CornerRadius::same(22), ACCENT.gamma_multiply(0.05 + hover * 0.12));
            p.rect_filled(rect, radius, fill);
            p.text(rect.center(), Align2::CENTER_CENTER, label, FontId::new(19.0, theme::bold_family()), BG);
        }
        Some(f) => {
            p.rect_filled(rect, radius, CARD);
            let mut filled = rect;
            filled.set_width(rect.width() * f.clamp(0.0, 1.0));
            p.with_clip_rect(filled).rect_filled(rect, radius, ACCENT_DEEP);
            p.rect_stroke(rect, radius, Stroke::new(1.0, STROKE), StrokeKind::Inside);
            p.text(rect.center(), Align2::CENTER_CENTER, label, FontId::new(19.0, theme::bold_family()), TEXT);
        }
    }
    if enabled { resp.on_hover_cursor(CursorIcon::PointingHand) } else { resp }
}

/// Slowly breathing isometric terrain, drawn from plain polygons (no game assets involved).
fn draw_voxels(painter: &Painter, center: Pos2, t: f32) {
    const N: i32 = 11;
    let (w, h, step) = (34.0f32, 19.6f32, 13.0f32);
    let c = (N - 1) as f32 / 2.0;
    for sum in 0..(2 * N - 1) {
        for x in 0..N {
            let y = sum - x;
            if !(0..N).contains(&y) {
                continue;
            }
            let (fx, fy) = (x as f32 - c, y as f32 - c);
            let d = (fx * fx + fy * fy).sqrt() / (c * 1.15);
            if d >= 1.0 {
                continue;
            }
            let wave = 0.5 + 0.5 * (x as f32 * 0.8 + t * 0.55).sin() * (y as f32 * 0.7 - t * 0.4).cos();
            let blocks = (1.0 + wave * 3.4 * (1.0 - d * 0.65)).floor();
            let height = blocks * step;
            let fade = (1.0 - d * d).clamp(0.0, 1.0) * 0.62;
            let (sx, sy) = (center.x + (fx - fy) * w, center.y + (fx + fy) * h);
            let top = pos2(sx, sy - height - h);
            let right = pos2(sx + w, sy - height);
            let bottom = pos2(sx, sy - height + h);
            let left = pos2(sx - w, sy - height);
            let edge = Stroke::new(1.0, BG.gamma_multiply(0.9));
            painter.add(Shape::convex_polygon(vec![left, bottom, pos2(sx, sy + h), pos2(sx - w, sy)], ACCENT_DEEP.gamma_multiply(fade * 0.9), edge));
            painter.add(Shape::convex_polygon(vec![bottom, right, pos2(sx + w, sy), pos2(sx, sy + h)], ACCENT_DEEP.gamma_multiply(fade * 0.55), edge));
            painter.add(Shape::convex_polygon(vec![top, right, bottom, left], ACCENT.gamma_multiply(fade * (0.55 + 0.4 * wave)), edge));
        }
    }
}

// ---------- other pages ----------

impl App {
    fn page_frame(&mut self, ui: &mut Ui, title: &str, body: impl FnOnce(&mut Self, &mut Ui)) {
        Frame::new().inner_margin(Margin::symmetric(40, 34)).show(ui, |ui| {
            ui.label(bold(title, 30.0, TEXT));
            ui.add_space(14.0);
            body(self, ui);
        });
    }

    fn versions_page(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            ui.checkbox(&mut self.settings.show_snapshots, "Show snapshots");
            if self.manifest.is_none() {
                if let Some(e) = &self.manifest_err {
                    ui.label(text(format!("Couldn't load versions: {e}"), 13.0, DANGER));
                    if ui.button("Retry").clicked() {
                        self.manifest_err = None;
                        self.fetch_manifest(ui.ctx());
                    }
                } else {
                    ui.label(text("Loading versions", 13.0, MUTED));
                }
            }
        });
        let Some(manifest) = self.manifest.clone() else { return };
        let selected = self.selected_id();
        ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            for v in manifest.versions.iter().filter(|v| v.kind == "release" || (self.settings.show_snapshots && v.kind == "snapshot")) {
                let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width() - 6.0, 54.0), Sense::click());
                let t = ui.ctx().animate_bool_with_time(resp.id, resp.hovered(), 0.12);
                let is_sel = v.id == selected;
                let p = ui.painter();
                p.rect_filled(rect, CornerRadius::same(12), CARD.lerp_to_gamma(CARD_HOVER, t));
                p.rect_stroke(rect, CornerRadius::same(12), Stroke::new(1.0, if is_sel { ACCENT } else { STROKE }), StrokeKind::Inside);
                p.text(rect.left_center() + vec2(20.0, 0.0), Align2::LEFT_CENTER, &v.id, FontId::new(17.0, theme::bold_family()), TEXT);
                let date = v.release_time.get(..10).unwrap_or("");
                p.text(rect.left_center() + vec2(190.0, 0.0), Align2::LEFT_CENTER, format!("{}  ·  {date}", v.kind), FontId::new(13.5, FontFamily::Proportional), MUTED);
                let (label, col) = if self.paths.is_installed(&v.id) { ("Installed", ACCENT) } else { ("", MUTED) };
                p.text(rect.right_center() - vec2(20.0, 0.0), Align2::RIGHT_CENTER, if is_sel && label.is_empty() { "Selected" } else { label }, FontId::new(13.5, theme::bold_family()), col);
                if resp.on_hover_cursor(CursorIcon::PointingHand).clicked() {
                    self.settings.selected = v.id.clone();
                }
            }
        });
    }

    fn account_page(&mut self, ui: &mut Ui) {
        card(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(bold("Offline account", 17.0, TEXT));
                pill(ui, "Active", ACCENT);
            });
            ui.label(text("Plays singleplayer and offline-mode servers. The name is used to derive a stable player UUID.", 13.5, MUTED));
            ui.add_space(6.0);
            ui.add(TextEdit::singleline(&mut self.settings.username).hint_text("Player name").desired_width(280.0).margin(Margin::symmetric(12, 9)));
            if !valid_name(&self.settings.username) {
                ui.label(text("3 to 16 characters: letters, numbers and underscores.", 13.0, DANGER));
            }
        });
        card(ui, |ui| {
            ui.label(bold("Microsoft account", 17.0, TEXT));
            ui.label(text("Online servers need a Microsoft sign-in. This needs an Azure app registration that Mojang has approved, which isn't set up yet.", 13.5, MUTED));
            ui.add_space(6.0);
            ui.add_enabled(false, Button::new("Sign in with Microsoft"));
        });
    }

    fn settings_page(&mut self, ui: &mut Ui) {
        card(ui, |ui| {
            ui.label(bold("Memory", 17.0, TEXT));
            ui.label(text("Maximum heap given to the game.", 13.5, MUTED));
            let mut gb = self.settings.memory_mb as f32 / 1024.0;
            if ui.add(Slider::new(&mut gb, 1.0..=12.0).step_by(0.5).suffix(" GB")).changed() {
                self.settings.memory_mb = (gb * 1024.0) as u32;
            }
        });
        card(ui, |ui| {
            ui.label(bold("Java", 17.0, TEXT));
            ui.label(text("Leave empty to use the runtime Mojang ships for each version (downloaded automatically).", 13.5, MUTED));
            ui.add(TextEdit::singleline(&mut self.settings.java_override).hint_text("Path to java.exe (optional)").desired_width(420.0).margin(Margin::symmetric(12, 9)));
        });
        card(ui, |ui| {
            ui.label(bold("Singleplayer world", 17.0, TEXT));
            ui.label(text("Folder name under worlds/ in the data folder. A new name creates a new world.", 13.5, MUTED));
            ui.add(TextEdit::singleline(&mut self.settings.world).hint_text("world").desired_width(280.0).margin(Margin::symmetric(12, 9)));
            // Keep it a safe folder name.
            self.settings.world.retain(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == ' ');
            if self.settings.world.trim().is_empty() {
                self.settings.world = "world".into();
            }
        });
        card(ui, |ui| {
            ui.label(bold("Folders", 17.0, TEXT));
            ui.label(text(self.paths.root.to_string_lossy(), 13.0, MUTED));
            ui.horizontal(|ui| {
                if ui.button("Open data folder").clicked() {
                    let _ = std::fs::create_dir_all(&self.paths.root);
                    let _ = std::process::Command::new("explorer").arg(&self.paths.root).spawn();
                }
                if ui.button("Open game folder").clicked() {
                    let _ = std::fs::create_dir_all(self.paths.game());
                    let _ = std::process::Command::new("explorer").arg(self.paths.game()).spawn();
                }
            });
        });
    }
}

fn card(ui: &mut Ui, add: impl FnOnce(&mut Ui)) {
    Frame::new()
        .fill(CARD)
        .stroke(Stroke::new(1.0, STROKE))
        .corner_radius(CornerRadius::same(14))
        .inner_margin(Margin::same(20))
        .show(ui, |ui| {
            ui.set_width(ui.available_width().min(640.0));
            add(ui);
        });
}
