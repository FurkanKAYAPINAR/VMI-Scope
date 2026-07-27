//! The "save query" dialog.

use eframe::egui;

use crate::app::VmiScopeApp;
use crate::widgets::button::{btn_primary, btn_secondary};
use crate::widgets::field::mono_input;

/// Width of the name field. A saved-query name is a label, not a path, so the
/// dialog stays the size of what gets typed into it.
const NAME_W: f32 = 240.0;

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
                // No hint: the label above already says what goes in here, and
                // two of them would just read as a repetition.
                let resp = ui
                    .scope(|ui| {
                        ui.set_max_width(NAME_W);
                        mono_input(ui, &mut self.save_query_name, "")
                    })
                    .inner;
                let can = !self.save_query_name.trim().is_empty();
                ui.horizontal(|ui| {
                    let save = ui.add_enabled_ui(can, |ui| btn_primary(ui, "Save")).inner;
                    if save.clicked()
                        || (can
                            && resp.lost_focus()
                            && ui.input(|i| i.key_pressed(egui::Key::Enter)))
                    {
                        do_save = true;
                    }
                    if btn_secondary(ui, "Cancel").clicked() {
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
