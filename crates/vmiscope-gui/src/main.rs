//! VMI-Scope — egui desktop front-end.
//!
//! `main` only boots the window and hands off to [`app::VmiScopeApp`].

// Hide the console window on Windows release builds; keep it in debug for logs.
#![cfg_attr(all(not(debug_assertions), windows), windows_subsystem = "windows")]

mod app;
mod config;

use app::VmiScopeApp;

fn main() -> eframe::Result<()> {
    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1240.0, 780.0])
            .with_min_inner_size([840.0, 520.0])
            .with_title("VMI-Scope"),
        ..Default::default()
    };

    eframe::run_native(
        "VMI-Scope",
        native_options,
        Box::new(|cc| Ok(Box::new(VmiScopeApp::new(cc)))),
    )
}
