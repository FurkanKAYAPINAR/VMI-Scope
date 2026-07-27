//! The selected-row detail panel.

use eframe::egui;

use crate::app::VmiScopeApp;
use crate::theme::icons;
use crate::widgets::button::btn_icon;
use crate::widgets::kv::kv_grid_sized;
use crate::widgets::rule::hrule;

impl VmiScopeApp {
    // ------------------------------------------------------------------
    // UI: selected-row detail
    // ------------------------------------------------------------------

    pub(crate) fn ui_detail(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.strong("Row detail");
            if btn_icon(ui, icons::X).on_hover_text("Close").clicked() {
                self.selected_row = None;
            }
        });
        hrule(ui);
        let (Some(result), Some(ri)) = (self.result.as_ref(), self.selected_row) else {
            return;
        };
        let Some(row) = result.rows.get(ri) else {
            return;
        };
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                // `kv_grid` owns the em-dash-for-empty convention, so the branch
                // that used to spell it out here is gone with it.
                kv_grid_sized(
                    ui,
                    "detail-grid",
                    140.0,
                    result
                        .columns
                        .iter()
                        .zip(row.iter())
                        .map(|(col, val)| (col.as_str(), val.as_str())),
                );
            });
    }
}
