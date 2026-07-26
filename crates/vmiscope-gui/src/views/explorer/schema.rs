//! The reflective class-schema view: properties, flags and method signatures.

use eframe::egui;

use crate::app::VmiScopeApp;
use crate::theme::icons;

impl VmiScopeApp {
    // ------------------------------------------------------------------
    // UI: reflective class schema
    // ------------------------------------------------------------------

    pub(crate) fn ui_schema(&mut self, ui: &mut egui::Ui) {
        if self.selected_class.is_none() {
            ui.weak("Select a class to view its schema.");
            return;
        }
        if self.schema.is_none() {
            if self.schema_loading {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.weak("reflecting schema\u{2026}");
                });
            } else {
                ui.weak("No schema available for this class.");
            }
            return;
        }

        // Header (scoped immutable borrow so the filter box can borrow mutably next).
        {
            let s = self.schema.as_ref().unwrap();
            ui.horizontal(|ui| {
                ui.heading(s.class.as_str());
                if s.is_abstract {
                    ui.weak("[abstract]");
                }
                if let Some(sup) = &s.super_class {
                    ui.weak(format!(": {sup}"));
                }
                ui.weak(format!(
                    "\u{00b7} {} props \u{00b7} {} methods",
                    s.properties.len(),
                    s.methods.len()
                ));
            });
            if let Some(d) = &s.description {
                ui.label(d);
            }
        }
        ui.horizontal(|ui| {
            ui.label(icons::MAGNIFYING_GLASS);
            ui.add(
                egui::TextEdit::singleline(&mut self.schema_filter)
                    .hint_text("filter properties / methods")
                    .desired_width(240.0),
            );
        });
        ui.separator();

        let filter = self.schema_filter.to_lowercase();
        let schema = self.schema.as_ref().unwrap();
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.strong("Properties");
                egui::Grid::new("schema-props")
                    .num_columns(4)
                    .striped(true)
                    .spacing([14.0, 3.0])
                    .show(ui, |ui| {
                        ui.strong("Name");
                        ui.strong("Type");
                        ui.strong("Flags");
                        ui.strong("Description");
                        ui.end_row();
                        for p in schema.properties.iter().filter(|p| {
                            filter.is_empty()
                                || p.name.to_lowercase().contains(&filter)
                                || p.cim_type.to_lowercase().contains(&filter)
                        }) {
                            let label = if p.is_key {
                                format!("{} {}", icons::KEY, p.name)
                            } else {
                                p.name.clone()
                            };
                            let resp = ui.label(label);
                            if !p.value_map.is_empty() {
                                let vm = p
                                    .value_map
                                    .iter()
                                    .map(|(c, l)| format!("{c} = {l}"))
                                    .collect::<Vec<_>>()
                                    .join("\n");
                                resp.on_hover_text(format!("ValueMap:\n{vm}"));
                            }
                            ui.label(p.cim_type.as_str());
                            let flags = format!(
                                "{}{}{}",
                                if p.is_read { "R" } else { "" },
                                if p.is_write { "W" } else { "" },
                                if p.is_key { "K" } else { "" }
                            );
                            ui.label(flags);
                            ui.label(
                                p.units
                                    .as_deref()
                                    .map(|u| format!("[{u}] "))
                                    .unwrap_or_default()
                                    + p.description.as_deref().unwrap_or_default(),
                            );
                            ui.end_row();
                        }
                    });

                ui.add_space(10.0);
                ui.strong("Methods");
                let methods: Vec<_> = schema
                    .methods
                    .iter()
                    .filter(|m| filter.is_empty() || m.name.to_lowercase().contains(&filter))
                    .collect();
                if methods.is_empty() {
                    ui.weak("(none)");
                }
                for m in methods {
                    let tag = if m.is_static { "  [static]" } else { "" };
                    egui::CollapsingHeader::new(format!("{}(){tag}", m.name))
                        .id_salt(m.name.as_str())
                        .show(ui, |ui| {
                            if let Some(d) = &m.description {
                                ui.label(d);
                            }
                            if !m.in_params.is_empty() {
                                ui.weak("in:");
                                for p in &m.in_params {
                                    let opt = if p.optional { "  (optional)" } else { "" };
                                    ui.label(format!("    {} : {}{opt}", p.name, p.cim_type));
                                }
                            }
                            if !m.out_params.is_empty() {
                                ui.weak("out:");
                                for p in &m.out_params {
                                    ui.label(format!("    {} : {}", p.name, p.cim_type));
                                }
                            }
                        });
                }
            });
    }
}
