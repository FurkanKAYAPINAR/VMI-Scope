//! The MOF viewer window.

use eframe::egui;

use crate::app::VmiScopeApp;
use crate::theme::icons;
use crate::widgets::button::btn_secondary;
use crate::widgets::codeview::{code_panel, Lang};
use crate::widgets::loading::spinner;

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
                    spinner(ui, "loading MOF\u{2026}");
                }
                if let Some(text) = self.mof_text.clone() {
                    if btn_secondary(ui, icons::labelled(ui, icons::COPY, "Copy")).clicked() {
                        ui.ctx().copy_text(text.clone());
                    }
                    // `code_panel` brings its own scrolling and gutter, so the
                    // window only has to decide how much room to hand it.
                    // No guide: MOF is read, not authored here, so a column
                    // mark would be a rule about somebody else's formatting.
                    code_panel(ui, &text, Lang::Mof, None);
                }
            });
        self.mof_open = open;
    }
}
