// No console window on Windows in release builds
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;
mod exif;
mod geo;
mod jpeg;
mod loader;
mod model;
mod settings;

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("Rollstamp")
            .with_app_id("rollstamp")
            .with_inner_size([1480.0, 940.0])
            .with_min_inner_size([1000.0, 660.0]),
        ..Default::default()
    };
    eframe::run_native("Rollstamp", options, Box::new(|cc| Ok(Box::new(app::App::new(cc)))))
}
