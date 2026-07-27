//! The namespace tree in the Explorer's left panel.

use eframe::egui;

use crate::app::{VmiScopeApp, ROOT_NAMESPACE};
use crate::theme::icons;

impl VmiScopeApp {
    // ------------------------------------------------------------------
    // UI: namespace tree
    // ------------------------------------------------------------------

    pub(crate) fn ui_namespace_tree(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.strong("Namespaces");
            if self.ns_loading.contains(ROOT_NAMESPACE) {
                ui.spinner();
            }
        });
        ui.separator();
        egui::ScrollArea::vertical()
            .id_salt("ns-tree")
            .max_height(ui.available_height() * 0.45)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                self.ui_namespace_node(ui, ROOT_NAMESPACE.to_string(), 0);
            });
    }

    pub(crate) fn ui_namespace_node(&mut self, ui: &mut egui::Ui, path: String, depth: usize) {
        let expanded = self.ns_expanded.contains(&path);
        let leaf = path.rsplit('\\').next().unwrap_or(&path).to_string();

        ui.horizontal(|ui| {
            ui.add_space(depth as f32 * 14.0);
            let arrow = if expanded {
                icons::CARET_DOWN
            } else {
                icons::CARET_RIGHT
            };
            if ui
                .add(egui::Button::new(icons::glyph(arrow)).frame(false))
                .on_hover_text("Expand / collapse")
                .clicked()
            {
                self.toggle_namespace(&path);
            }
            if ui
                .selectable_label(self.active_ns.eq_ignore_ascii_case(&path), &leaf)
                .clicked()
            {
                self.select_namespace(path.clone());
            }
        });

        if expanded {
            match self.ns_children.get(&path).cloned() {
                Some(children) => {
                    for child in children {
                        self.ui_namespace_node(ui, child, depth + 1);
                    }
                }
                None => {
                    ui.horizontal(|ui| {
                        ui.add_space((depth as f32 + 1.0) * 14.0);
                        ui.spinner();
                        ui.weak("loading\u{2026}");
                    });
                }
            }
        }
    }
}
