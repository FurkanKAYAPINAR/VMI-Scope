//! The error-log window behind the status bar's `Log (n)` button.

use eframe::egui;

use crate::app::VmiScopeApp;

impl VmiScopeApp {
    pub(crate) fn ui_error_log_window(&mut self, ctx: &egui::Context) {
        if !self.error_log_open {
            return;
        }
        let mut open = true;
        let mut clear = false;
        egui::Window::new(format!("Error log ({})", self.error_log.len()))
            .open(&mut open)
            .default_size([560.0, 300.0])
            .show(ctx, |ui| {
                if ui.button("Clear").clicked() {
                    clear = true;
                }
                ui.separator();
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        for e in &self.error_log {
                            ui.label(egui::RichText::new(e).monospace());
                            ui.separator();
                        }
                    });
            });
        if clear {
            self.error_log.clear();
        }
        if !open || clear {
            self.error_log_open = false;
        }
    }
}
