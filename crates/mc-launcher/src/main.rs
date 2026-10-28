//! Native launcher: version manifest, downloads, Java runtime, offline accounts, game launch.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod install;
mod launch;
mod meta;
mod settings;
mod theme;
mod ui;

use eframe::egui;

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("mcrs")
            .with_inner_size([1080.0, 660.0])
            .with_min_inner_size([860.0, 560.0])
            .with_maximized(false),
        ..Default::default()
    };
    eframe::run_native("mcrs", options, Box::new(|cc| Ok(Box::new(ui::App::new(cc)))))
}
