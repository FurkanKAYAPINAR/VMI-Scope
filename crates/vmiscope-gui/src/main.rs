//! VMI-Scope — egui desktop front-end.
//!
//! `main` only boots the window and hands off to [`app::VmiScopeApp`].

// Hide the console window on Windows release builds; keep it in debug for logs.
#![cfg_attr(all(not(debug_assertions), windows), windows_subsystem = "windows")]

mod app;
mod config;
mod overlays;
mod shell;
mod state;
mod theme;
mod util;
mod views;
mod widgets;

use app::VmiScopeApp;

/// The escape hatch out of the custom window chrome.
///
/// Custom chrome costs three things that cannot be recovered under egui 0.35:
/// Windows 11 Snap Layouts (which need `WM_NCHITTEST` to answer `HTMAXBUTTON`,
/// and winit does not handle that message at all), screen-reader discovery of
/// the caption buttons as *system* caption buttons, and a handful of tiling and
/// remote-desktop edge cases that key off a real caption.
///
/// So `--decorated` hands all of that back to the OS. It is a supported,
/// tested path rather than a debug toggle: the title bar drops its own window
/// buttons and `shell::chrome` skips both the drag region and the resize
/// strips, or the window ends up with two of everything.
const DECORATED_FLAG: &str = "--decorated";

fn main() -> eframe::Result<()> {
    let decorated = std::env::args().any(|arg| arg == DECORATED_FLAG);

    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("VMI-Scope")
            .with_decorations(decorated)
            .with_resizable(true)
            .with_inner_size([1240.0, 780.0])
            // 980 rather than 840: the rebuilt Explorer is three columns, and
            // the narrowest arrangement that keeps all three usable alongside a
            // 64px rail is 980.
            .with_min_inner_size([980.0, 560.0]),
        ..Default::default()
    };

    eframe::run_native(
        "VMI-Scope",
        native_options,
        Box::new(move |cc| Ok(Box::new(VmiScopeApp::new(cc, decorated)))),
    )
}
