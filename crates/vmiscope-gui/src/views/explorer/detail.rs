//! The selected-row detail panel.

use eframe::egui;

use crate::app::VmiScopeApp;
use crate::theme::icons;

impl VmiScopeApp {
    // ------------------------------------------------------------------
    // UI: selected-row detail
    // ------------------------------------------------------------------

    pub(crate) fn ui_detail(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.strong("Row detail");
            if ui
                .small_button(icons::glyph(icons::X))
                .on_hover_text("Close")
                .clicked()
            {
                self.selected_row = None;
            }
        });
        ui.separator();
        let (Some(result), Some(ri)) = (self.result.as_ref(), self.selected_row) else {
            return;
        };
        let Some(row) = result.rows.get(ri) else {
            return;
        };
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                egui::Grid::new("detail-grid")
                    .num_columns(2)
                    .striped(true)
                    .spacing([12.0, 4.0])
                    .show(ui, |ui| {
                        for (col, val) in result.columns.iter().zip(row.iter()) {
                            ui.strong(col);
                            if val.is_empty() {
                                ui.weak("\u{2014}"); // em dash for empty
                            } else {
                                ui.label(val);
                            }
                            ui.end_row();
                        }
                    });
            });
    }
}
