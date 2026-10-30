//! Immediate-mode widgets in vanilla's style (buttons, sliders, text fields, lists) on top of [`Painter`].
//! Each frame a screen lays out its widgets; mouse, keyboard and the controller (D-pad / stick to move focus,
//! A to press, B to go back) all drive them.

use crate::hud::{self, Painter};
use winit::keyboard::KeyCode;

/// This frame's input, in GUI pixels.
#[derive(Default, Clone)]
pub struct Input {
    pub mouse: (f32, f32),
    pub clicked: bool,
    pub right_clicked: bool,
    pub held: bool,
    pub double_click: bool,
    /// Wheel lines (positive = up).
    pub scroll: f32,
    pub typed: String,
    pub keys: Vec<KeyCode>,
    pub shift: bool,
    pub ctrl: bool,
    /// Controller: move focus, press, back.
    pub nav: Option<Nav>,
    pub accept: bool,
    pub back: bool,
    /// Last input came from the controller (show focus instead of hover).
    pub pad: bool,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Nav {
    Up,
    Down,
    Left,
    Right,
}

impl Input {
    pub fn pressed(&self, k: KeyCode) -> bool {
        self.keys.contains(&k)
    }
}

/// Focus that survives between frames.
#[derive(Default)]
pub struct Focus {
    /// Widget index (in layout order) the controller or Tab has selected.
    pub index: Option<usize>,
    /// Text field with the keyboard caret.
    pub text: Option<usize>,
    /// Slider being dragged.
    pub dragging: Option<usize>,
    /// Rectangles of last frame's widgets, for moving focus spatially.
    rects: Vec<[f32; 4]>,
}

pub struct Ui<'a, 'b> {
    pub p: &'a mut Painter<'b>,
    pub input: &'a Input,
    pub focus: &'a mut Focus,
    next: usize,
    rects: Vec<[f32; 4]>,
    /// A widget was pressed this frame (so the screen shouldn't also treat the click as "outside").
    pub used: bool,
}

fn inside(m: (f32, f32), x: f32, y: f32, w: f32, h: f32) -> bool {
    m.0 >= x && m.0 < x + w && m.1 >= y && m.1 < y + h
}

impl<'a, 'b> Ui<'a, 'b> {
    pub fn new(p: &'a mut Painter<'b>, input: &'a Input, focus: &'a mut Focus) -> Self {
        // Move focus from last frame's layout.
        if let Some(nav) = input.nav {
            focus.index = Some(match focus.index {
                None => 0,
                Some(i) => nearest(&focus.rects, i, nav).unwrap_or(i),
            });
        }
        if input.pressed(KeyCode::Tab) && !focus.rects.is_empty() {
            let n = focus.rects.len();
            focus.index = Some(match focus.index {
                None => 0,
                Some(i) if input.shift => (i + n - 1) % n,
                Some(i) => (i + 1) % n,
            });
        }
        Self { p, input, focus, next: 0, rects: Vec::new(), used: false }
    }

    fn id(&mut self, x: f32, y: f32, w: f32, h: f32) -> usize {
        let id = self.next;
        self.next += 1;
        self.rects.push([x, y, w, h]);
        id
    }

    /// Hovered by the mouse, or focused by the controller.
    fn hot(&self, id: usize, x: f32, y: f32, w: f32, h: f32) -> bool {
        if self.input.pad { self.focus.index == Some(id) } else { inside(self.input.mouse, x, y, w, h) }
    }

    fn activated(&mut self, id: usize, x: f32, y: f32, w: f32, h: f32) -> bool {
        let clicked = self.input.clicked && inside(self.input.mouse, x, y, w, h);
        let pad = self.input.accept && self.focus.index == Some(id);
        let enter = self.focus.index == Some(id) && self.focus.text.is_none() && (self.input.pressed(KeyCode::Enter) || self.input.pressed(KeyCode::Space));
        if clicked || pad || enter {
            self.focus.index = Some(id);
            self.used = true;
            true
        } else {
            false
        }
    }

    /// Vanilla's 20-pixel-tall button. Returns true when pressed.
    pub fn button(&mut self, label: &str, x: f32, y: f32, w: f32, h: f32, enabled: bool) -> bool {
        let id = self.id(x, y, w, h);
        let hot = enabled && self.hot(id, x, y, w, h);
        let sprite = if !enabled { "button_disabled" } else if hot { "button_highlighted" } else { "button" };
        self.p.nine_slice(sprite, x, y, w, h, 3.0);
        let color = if enabled { hud::WHITE } else { hud::GRAY };
        // Vanilla clips long labels; we shrink the gap instead.
        self.p.text_centered(label, x + w / 2.0, y + (h - 8.0) / 2.0, color);
        let pressed = enabled && self.activated(id, x, y, w, h);
        if pressed {
            click_sound();
        }
        pressed
    }

    /// A button showing `Label: Value` that cycles through `values` (vanilla's `CycleButton`).
    pub fn cycle(&mut self, label: &str, values: &[&str], value: &mut usize, x: f32, y: f32, w: f32, enabled: bool) -> bool {
        let text = if label.is_empty() { values[*value % values.len()].to_string() } else { format!("{label}: {}", values[*value % values.len()]) };
        if self.button(&text, x, y, w, 20.0, enabled) {
            *value = if self.input.shift { (*value + values.len() - 1) % values.len() } else { (*value + 1) % values.len() };
            return true;
        }
        false
    }

    pub fn toggle(&mut self, label: &str, value: &mut bool, x: f32, y: f32, w: f32) -> bool {
        let mut i = *value as usize;
        if self.cycle(label, &["OFF", "ON"], &mut i, x, y, w, true) {
            *value = i == 1;
            return true;
        }
        false
    }

    /// Vanilla's slider: `value` 0..1, the label is drawn from `text(value)`. Returns true while it changes.
    pub fn slider(&mut self, value: &mut f32, text: &str, x: f32, y: f32, w: f32, steps: Option<u32>) -> bool {
        let h = 20.0;
        let id = self.id(x, y, w, h);
        let hot = self.hot(id, x, y, w, h);
        let before = *value;
        if self.input.clicked && inside(self.input.mouse, x, y, w, h) {
            self.focus.dragging = Some(id);
            self.focus.index = Some(id);
            self.used = true;
        }
        if self.focus.dragging == Some(id) {
            if self.input.held {
                *value = ((self.input.mouse.0 - (x + 4.0)) / (w - 8.0)).clamp(0.0, 1.0);
            } else {
                self.focus.dragging = None;
                click_sound();
            }
        }
        if self.focus.index == Some(id) {
            let step = steps.map_or(0.01, |s| 1.0 / s.max(1) as f32);
            if self.input.pressed(KeyCode::ArrowLeft) || (self.input.pad && self.input.nav == Some(Nav::Left)) {
                *value = (*value - step).max(0.0);
            }
            if self.input.pressed(KeyCode::ArrowRight) || (self.input.pad && self.input.nav == Some(Nav::Right)) {
                *value = (*value + step).min(1.0);
            }
        }
        if let Some(s) = steps {
            *value = (*value * s as f32).round() / s as f32;
        }
        self.p.nine_slice(if hot { "slider_highlighted" } else { "slider" }, x, y, w, h, 1.0);
        let hx = x + *value * (w - 8.0);
        self.p.nine_slice(if hot || self.focus.dragging == Some(id) { "slider_handle_highlighted" } else { "slider_handle" }, hx, y, 8.0, h, 1.0);
        self.p.text_centered(text, x + w / 2.0, y + 6.0, hud::WHITE);
        *value != before
    }

    /// A text field; returns true when Enter was pressed in it.
    pub fn text_field(&mut self, text: &mut String, hint: &str, x: f32, y: f32, w: f32, h: f32, max: usize) -> bool {
        let id = self.id(x, y, w, h);
        if self.input.clicked {
            if inside(self.input.mouse, x, y, w, h) {
                self.focus.text = Some(id);
                self.focus.index = Some(id);
                self.used = true;
            } else if self.focus.text == Some(id) {
                self.focus.text = None;
            }
        }
        if self.input.pad && self.input.accept && self.focus.index == Some(id) {
            self.focus.text = Some(id);
        }
        if self.focus.index == Some(id) && self.input.pad && self.focus.text.is_none() {
            self.focus.text = Some(id);
        }
        let active = self.focus.text == Some(id);
        let mut entered = false;
        if active {
            for c in self.input.typed.chars() {
                if !c.is_control() && text.chars().count() < max {
                    text.push(c);
                }
            }
            if self.input.pressed(KeyCode::Backspace) {
                if self.input.ctrl {
                    let keep = text.trim_end().rfind(' ').map_or(0, |i| i + 1);
                    text.truncate(keep);
                } else {
                    text.pop();
                }
            }
            if self.input.ctrl && self.input.pressed(KeyCode::KeyV) {
                if let Some(clip) = clipboard() {
                    for c in clip.chars().filter(|c| !c.is_control()) {
                        if text.chars().count() < max {
                            text.push(c);
                        }
                    }
                }
            }
            entered = self.input.pressed(KeyCode::Enter) || self.input.pressed(KeyCode::NumpadEnter);
        }
        self.p.nine_slice(if active { "text_field_highlighted" } else { "text_field" }, x, y, w, h, 1.0);
        let ty = y + (h - 8.0) / 2.0;
        if text.is_empty() && !active {
            self.p.text(hint, x + 4.0, ty, [112, 112, 112, 255]);
        } else {
            // Show the end of long text.
            let mut shown: &str = text;
            while self.p.text_width(shown) > w - 10.0 && !shown.is_empty() {
                let mut it = shown.chars();
                it.next();
                shown = it.as_str();
            }
            self.p.text(shown, x + 4.0, ty, [224, 224, 224, 255]);
            let blink = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() / 300) % 2 == 0;
            if active && blink {
                let cx = x + 4.0 + self.p.text_width(shown) + 1.0;
                self.p.fill(cx, ty - 1.0, 1.0, 10.0, [208, 208, 208, 255]);
            }
        }
        entered
    }

    /// A clickable rectangle (list entries); returns (clicked, hovered).
    pub fn area(&mut self, x: f32, y: f32, w: f32, h: f32) -> (bool, bool) {
        let id = self.id(x, y, w, h);
        let hot = self.hot(id, x, y, w, h);
        (self.activated(id, x, y, w, h), hot)
    }

    pub fn label(&mut self, text: &str, cx: f32, y: f32, color: [u8; 4]) {
        self.p.text_centered(text, cx, y, color);
    }

    /// Ends the frame: remembers layout for focus moves.
    pub fn finish(self) {
        if let Some(i) = self.focus.index {
            if i >= self.rects.len() {
                self.focus.index = if self.rects.is_empty() { None } else { Some(self.rects.len() - 1) };
            }
        }
        self.focus.rects = self.rects;
    }
}

/// The widget whose center is nearest in direction `nav` from widget `from`.
fn nearest(rects: &[[f32; 4]], from: usize, nav: Nav) -> Option<usize> {
    let c = |r: &[f32; 4]| (r[0] + r[2] / 2.0, r[1] + r[3] / 2.0);
    let here = c(rects.get(from)?);
    rects
        .iter()
        .enumerate()
        .filter(|(i, _)| *i != from)
        .filter_map(|(i, r)| {
            let (x, y) = c(r);
            let (dx, dy) = (x - here.0, y - here.1);
            let (along, across) = match nav {
                Nav::Up => (-dy, dx),
                Nav::Down => (dy, dx),
                Nav::Left => (-dx, dy),
                Nav::Right => (dx, dy),
            };
            (along > 1.0).then_some((i, along + across.abs() * 2.0))
        })
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(i, _)| i)
}

thread_local! {
    /// Button clicks queue a UI sound for the app to play (`ui.button.click`).
    pub static CLICKS: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

fn click_sound() {
    CLICKS.with(|c| c.set(c.get() + 1));
}

/// Windows clipboard text (for pasting server addresses).
fn clipboard() -> Option<String> {
    use std::os::windows::process::CommandExt;
    let out = std::process::Command::new("powershell").args(["-NoProfile", "-Command", "Get-Clipboard"]).creation_flags(0x0800_0000).output().ok()?;
    Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Scroll state of a list.
#[derive(Default, Clone, Copy)]
pub struct Scroll {
    pub offset: f32,
}

impl Scroll {
    /// Applies the wheel and keeps the offset in range for `content` pixels in a `view`-pixel area.
    pub fn update(&mut self, input: &Input, inside_list: bool, content: f32, view: f32) {
        if inside_list {
            self.offset -= input.scroll * 18.0;
        }
        self.offset = self.offset.clamp(0.0, (content - view).max(0.0));
    }
}
