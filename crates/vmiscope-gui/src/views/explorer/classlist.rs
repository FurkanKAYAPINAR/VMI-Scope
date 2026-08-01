//! The filtered class list under the namespace tree.

use eframe::egui;

use vmiscope_core::ClassBrief;

use crate::app::VmiScopeApp;
use crate::widgets::field::filter_box;
use crate::widgets::loading::spinner;
use crate::widgets::rule::hrule;

impl VmiScopeApp {
    // ------------------------------------------------------------------
    // UI: class list
    // ------------------------------------------------------------------

    pub(crate) fn ui_class_list(&mut self, ui: &mut egui::Ui) {
        // The filter spans the panel, whatever the user has dragged it to.
        // `spacing.text_edit_width` is where a kit input takes its width from,
        // and egui's default of 280 would leave a widened panel half empty.
        ui.spacing_mut().text_edit_width = ui.available_width();
        ui.horizontal(|ui| {
            ui.strong("Classes");
            if self.classes_loading {
                spinner(ui, "listing");
            }
            ui.weak(format!("({})", self.classes.len()));
        });
        // The magnifier is a prefix atom inside the field now, so the label that
        // used to sit beside it is gone with it.
        filter_box(ui, &mut self.class_filter, "filter classes");
        hrule(ui);

        let mut clicked: Option<String> = None;
        {
            let filter = self.class_filter.to_lowercase();
            let filtered: Vec<&ClassBrief> = self
                .classes
                .iter()
                .filter(|c| filter.is_empty() || c.name.to_lowercase().contains(&filter))
                .collect();
            let row_h = ui.text_style_height(&egui::TextStyle::Body) + 4.0;
            egui::ScrollArea::vertical()
                .id_salt("class-list")
                .auto_shrink([false, false])
                .show_rows(ui, row_h, filtered.len(), |ui, range| {
                    for i in range {
                        let class = filtered[i];
                        let selected = self.selected_class.as_deref() == Some(class.name.as_str());
                        // The kind badge and the instance count that
                        // `ClassBrief` now carries arrive with the Phase 3
                        // rebuild; this row still shows only the name.
                        if ui.selectable_label(selected, &class.name).clicked() {
                            clicked = Some(class.name.clone());
                        }
                    }
                });
        }
        if let Some(class) = clicked {
            self.select_class(class);
        }
    }
}
