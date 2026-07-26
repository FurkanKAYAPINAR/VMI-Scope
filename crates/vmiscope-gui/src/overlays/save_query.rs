//! The "save query" dialog.

use eframe::egui;

use crate::app::VmiScopeApp;

impl VmiScopeApp {
    pub(crate) fn ui_save_query_window(&mut self, ctx: &egui::Context) {
        if !self.save_query_open {
            return;
        }
        let mut open = true;
        let mut do_save = false;
        let mut cancel = false;
        egui::Window::new("Save query")
            .collapsible(false)
            .resizable(false)
            .open(&mut open)
            .show(ctx, |ui| {
                ui.label("Name:");
                let resp = ui.add(
                    egui::TextEdit::singleline(&mut self.save_query_name).desired_width(240.0),
                );
                let can = !self.save_query_name.trim().is_empty();
                ui.horizontal(|ui| {
                    if ui.add_enabled(can, egui::Button::new("Save")).clicked()
                        || (can
                            && resp.lost_focus()
                            && ui.input(|i| i.key_pressed(egui::Key::Enter)))
                    {
                        do_save = true;
                    }
                    if ui.button("Cancel").clicked() {
                        cancel = true;
                    }
                });
            });
        if do_save {
            self.config.save_query(
                self.save_query_name.trim().to_string(),
                self.active_ns.clone(),
                self.query_text.clone(),
            );
            self.save_query_open = false;
        } else if cancel || !open {
            self.save_query_open = false;
        }
    }
}
