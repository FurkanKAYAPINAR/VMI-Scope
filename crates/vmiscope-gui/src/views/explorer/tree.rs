//! The namespace tree in the Explorer's left panel.

use eframe::egui;

use crate::app::{VmiScopeApp, ROOT_NAMESPACE};
use crate::theme::icons;
use crate::widgets::button::btn_icon;
use crate::widgets::loading::spinner;
use crate::widgets::rule::hrule;

/// Horizontal step per tree level. Not on the density scale: the indent is what
/// makes the hierarchy readable, and a tighter density should not flatten it.
const INDENT: f32 = 14.0;

impl VmiScopeApp {
    // ------------------------------------------------------------------
    // UI: namespace tree
    // ------------------------------------------------------------------

    pub(crate) fn ui_namespace_tree(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.strong("Namespaces");
            if self.ns_loading.contains(ROOT_NAMESPACE) {
                spinner(ui, "loading");
            }
        });
        hrule(ui);
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
            ui.add_space(depth as f32 * INDENT);
            let arrow = if expanded {
                icons::CARET_DOWN
            } else {
                icons::CARET_RIGHT
            };
            if btn_icon(ui, arrow)
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
                        ui.add_space((depth as f32 + 1.0) * INDENT);
                        spinner(ui, "loading\u{2026}");
                    });
                }
            }
        }
    }
}
