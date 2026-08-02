//! The namespace tree in the Explorer's first column.

use eframe::egui;
use eframe::egui::{Align, Layout, RichText, TextStyle};

use crate::app::{VmiScopeApp, ROOT_NAMESPACE};
use crate::theme::icons;
use crate::theme::tokens::muted;
use crate::widgets::button::btn_icon;
use crate::widgets::loading::spinner;

/// Horizontal step per tree level. The mock's 13px. Not on the density scale:
/// the indent is what makes the hierarchy legible, and a tighter density should
/// not flatten it.
const INDENT: f32 = 13.0;

impl VmiScopeApp {
    // ------------------------------------------------------------------
    // UI: namespace tree
    // ------------------------------------------------------------------

    pub(crate) fn ui_namespace_tree(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(icons::labelled(ui, icons::DATABASE, "Namespaces"));
            if self.ns_loading.contains(ROOT_NAMESPACE) {
                spinner(ui, "loading");
            }
        });

        egui::ScrollArea::vertical()
            .id_salt("ns-tree")
            .max_height(ui.available_height() * 0.55)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                self.ui_namespace_node(ui, ROOT_NAMESPACE.to_string(), 0);
            });

        // Footer: how many namespaces the tree knows, and how long the most
        // recent count took. Both are facts we already hold, so there is never a
        // placeholder here.
        let known = self.known_namespace_count();
        let footer = match self.last_ns_stats_ms {
            Some(ms) => format!("{known} namespaces \u{00b7} {ms} ms"),
            None => format!("{known} namespaces"),
        };
        ui.label(
            RichText::new(footer)
                .text_style(TextStyle::Name("caption".into()))
                .color(muted(42)),
        );
    }

    /// One namespace node: caret, folder, name, and -- once counted -- the
    /// number of classes it defines.
    pub(crate) fn ui_namespace_node(&mut self, ui: &mut egui::Ui, path: String, depth: usize) {
        // Class counts populate lazily: the row asks for its own namespace's
        // stats the first time it is drawn, and the guard inside dedupes the
        // request. Cheap next to an instance count -- `CreateClassEnum` counted
        // without reading a single object.
        self.request_namespace_stats(path.clone());

        let expanded = self.ns_expanded.contains(&path);
        let leaf = path.rsplit('\\').next().unwrap_or(&path).to_string();
        let selected = self.active_ns.eq_ignore_ascii_case(&path);

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
            let folder = if expanded {
                icons::FOLDER_OPEN
            } else {
                icons::FOLDER
            };
            if ui
                .selectable_label(selected, icons::labelled(ui, folder, &leaf))
                .clicked()
            {
                self.select_namespace(path.clone());
            }
            // The class count sits right-aligned, so counts line up down the
            // column however deep the node is indented.
            if let Some(stats) = self.ns_stats.get(&path) {
                let n = stats.classes;
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(
                        RichText::new(n.to_string())
                            .text_style(TextStyle::Name("code".into()))
                            .color(muted(45)),
                    );
                });
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

    /// Every namespace path the tree currently knows: the root, plus every child
    /// of every loaded parent. Each non-root namespace is a child of exactly one
    /// parent, so this does not double-count.
    fn known_namespace_count(&self) -> usize {
        1 + self.ns_children.values().map(Vec::len).sum::<usize>()
    }
}
