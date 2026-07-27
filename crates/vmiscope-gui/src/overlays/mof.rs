//! The MOF viewer window.

use eframe::egui;

use crate::app::VmiScopeApp;
use crate::theme::icons;

impl VmiScopeApp {
    // ------------------------------------------------------------------
    // UI: MOF viewer (floating window)
    // ------------------------------------------------------------------

    pub(crate) fn ui_mof_window(&mut self, ctx: &egui::Context) {
        if !self.mof_open {
            return;
        }
        let mut open = self.mof_open;
        egui::Window::new(format!("MOF \u{2014} {}", self.mof_title))
            .open(&mut open)
            .default_size([560.0, 460.0])
            .show(ctx, |ui| {
                if self.mof_loading {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.weak("loading MOF\u{2026}");
                    });
                }
                if let Some(text) = self.mof_text.clone() {
                    if ui
                        .button(icons::labelled(ui, icons::COPY, "Copy"))
                        .clicked()
                    {
                        ui.ctx().copy_text(text.clone());
                    }
                    egui::ScrollArea::both()
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            ui.add(
                                egui::Label::new(egui::RichText::new(text.as_str()).monospace())
                                    .selectable(true),
                            );
                        });
                }
            });
        self.mof_open = open;
    }
}
