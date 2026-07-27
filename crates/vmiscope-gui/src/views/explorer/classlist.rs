//! The filtered class list under the namespace tree.

use eframe::egui;

use crate::app::VmiScopeApp;
use crate::theme::icons;

impl VmiScopeApp {
    // ------------------------------------------------------------------
    // UI: class list
    // ------------------------------------------------------------------

    pub(crate) fn ui_class_list(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.strong("Classes");
            if self.classes_loading {
                ui.spinner();
            }
            ui.weak(format!("({})", self.classes.len()));
        });
        ui.horizontal(|ui| {
            ui.label(icons::glyph(icons::MAGNIFYING_GLASS));
            ui.add(
                egui::TextEdit::singleline(&mut self.class_filter)
                    .hint_text("filter classes")
                    .desired_width(f32::INFINITY),
            );
        });
        ui.separator();

        let mut clicked: Option<String> = None;
        {
            let filter = self.class_filter.to_lowercase();
            let filtered: Vec<&String> = self
                .classes
                .iter()
                .filter(|c| filter.is_empty() || c.to_lowercase().contains(&filter))
                .collect();
            let row_h = ui.text_style_height(&egui::TextStyle::Body) + 4.0;
            egui::ScrollArea::vertical()
                .id_salt("class-list")
                .auto_shrink([false, false])
                .show_rows(ui, row_h, filtered.len(), |ui, range| {
                    for i in range {
                        let class = filtered[i];
                        let selected = self.selected_class.as_deref() == Some(class.as_str());
                        if ui.selectable_label(selected, class).clicked() {
                            clicked = Some(class.clone());
                        }
                    }
                });
        }
        if let Some(class) = clicked {
            self.select_class(class);
        }
    }
}
