//! Colors, fonts and egui style for the launcher.

use eframe::egui::{self, Color32, FontData, FontDefinitions, FontFamily, FontId, RichText, TextStyle};
use std::sync::Arc;

pub const BG: Color32 = Color32::from_rgb(11, 13, 18);
pub const SIDEBAR: Color32 = Color32::from_rgb(15, 18, 24);
pub const CARD: Color32 = Color32::from_rgb(21, 25, 33);
pub const CARD_HOVER: Color32 = Color32::from_rgb(27, 32, 42);
pub const STROKE: Color32 = Color32::from_rgb(35, 41, 53);
pub const TEXT: Color32 = Color32::from_rgb(232, 235, 242);
pub const MUTED: Color32 = Color32::from_rgb(138, 147, 166);
pub const ACCENT: Color32 = Color32::from_rgb(62, 230, 168);
pub const ACCENT_DEEP: Color32 = Color32::from_rgb(18, 140, 112);
pub const DANGER: Color32 = Color32::from_rgb(255, 107, 107);

pub fn bold_family() -> FontFamily {
    FontFamily::Name("bold".into())
}

pub fn text(s: impl Into<String>, size: f32, color: Color32) -> RichText {
    RichText::new(s).size(size).color(color)
}

pub fn bold(s: impl Into<String>, size: f32, color: Color32) -> RichText {
    RichText::new(s).size(size).color(color).family(bold_family())
}

fn load(name: &str) -> Option<Vec<u8>> {
    std::fs::read(format!(r"C:\Windows\Fonts\{name}")).ok()
}

/// Uses the system's Segoe UI so the app looks native without bundling font files.
pub fn install(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    if let Some(bytes) = load("segoeui.ttf") {
        fonts.font_data.insert("segoe".into(), Arc::new(FontData::from_owned(bytes)));
        fonts.families.entry(FontFamily::Proportional).or_default().insert(0, "segoe".into());
    }
    if let Some(bytes) = load("seguisym.ttf") {
        fonts.font_data.insert("segoe-sym".into(), Arc::new(FontData::from_owned(bytes)));
        fonts.families.entry(FontFamily::Proportional).or_default().push("segoe-sym".into());
    }
    let bold_name = if let Some(bytes) = load("seguisb.ttf").or_else(|| load("segoeuib.ttf")) {
        fonts.font_data.insert("segoe-bold".into(), Arc::new(FontData::from_owned(bytes)));
        "segoe-bold"
    } else {
        "segoe"
    };
    let mut bold_stack = vec![bold_name.to_string()];
    bold_stack.extend(fonts.families[&FontFamily::Proportional].iter().cloned());
    fonts.families.insert(bold_family(), bold_stack);
    ctx.set_fonts(fonts);

    ctx.global_style_mut(|style| {
        style.text_styles = [
            (TextStyle::Heading, FontId::new(28.0, FontFamily::Proportional)),
            (TextStyle::Body, FontId::new(15.0, FontFamily::Proportional)),
            (TextStyle::Button, FontId::new(15.0, FontFamily::Proportional)),
            (TextStyle::Small, FontId::new(12.5, FontFamily::Proportional)),
            (TextStyle::Monospace, FontId::new(13.5, FontFamily::Monospace)),
        ]
        .into();
        let v = &mut style.visuals;
        v.dark_mode = true;
        v.panel_fill = BG;
        v.window_fill = CARD;
        v.extreme_bg_color = Color32::from_rgb(9, 11, 15);
        v.override_text_color = Some(TEXT);
        v.selection.bg_fill = ACCENT_DEEP;
        v.selection.stroke = egui::Stroke::new(1.0, ACCENT);
        v.widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0, STROKE);
        for w in [&mut v.widgets.inactive, &mut v.widgets.hovered, &mut v.widgets.active] {
            w.corner_radius = 10.into();
        }
        v.widgets.inactive.bg_fill = CARD;
        v.widgets.inactive.weak_bg_fill = CARD;
        v.widgets.hovered.bg_fill = CARD_HOVER;
        v.widgets.hovered.weak_bg_fill = CARD_HOVER;
        v.widgets.active.bg_fill = CARD_HOVER;
        style.spacing.item_spacing = egui::vec2(10.0, 10.0);
        style.spacing.button_padding = egui::vec2(16.0, 9.0);
        style.spacing.slider_width = 260.0;
    });
}
