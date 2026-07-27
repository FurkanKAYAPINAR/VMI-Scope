//! The Actions side panel: pick a WMI method, fill its inputs, and invoke it.

use eframe::egui;
use egui::Color32;

use crate::app::VmiScopeApp;
use crate::theme::icons;

use vmiscope_core::{param_kind, ParamKind};

impl VmiScopeApp {
    // ------------------------------------------------------------------
    // UI: Actions (method execution)
    // ------------------------------------------------------------------

    pub(crate) fn ui_actions(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            // `ui.strong` takes a `RichText`, which carries one font family and
            // so cannot hold the icon; the colour it would have applied is
            // spelled out instead.
            let strong = ui.visuals().strong_text_color();
            ui.label(icons::labelled_styled(
                ui,
                icons::GEAR_SIX,
                "Actions",
                egui::TextStyle::Body,
                strong,
            ));
            if self.act_invoking {
                ui.spinner();
            }
        });
        ui.weak("invoke WMI methods \u{2014} may change system state");
        ui.separator();

        let Some(class) = self.selected_class.clone() else {
            ui.weak("Select a class first.");
            return;
        };

        // Method signatures come from the reflected schema.
        if self.schema_class != class || self.schema.is_none() {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.weak("loading class methods\u{2026}");
            });
            return;
        }

        // Owned method metadata so the schema borrow is released for the widgets.
        struct MInfo {
            name: String,
            is_static: bool,
            inputs: Vec<(String, String)>,
        }
        let methods: Vec<MInfo> = self
            .schema
            .as_ref()
            .unwrap()
            .methods
            .iter()
            .map(|m| MInfo {
                name: m.name.clone(),
                is_static: m.is_static,
                inputs: m
                    .in_params
                    .iter()
                    .map(|p| (p.name.clone(), p.cim_type.clone()))
                    .collect(),
            })
            .collect();
        if methods.is_empty() {
            ui.weak("This class has no methods.");
            return;
        }

        // Method picker.
        let current = self.act_method.clone().unwrap_or_default();
        egui::ComboBox::from_id_salt("act-method")
            .width(f32::INFINITY)
            .selected_text(if current.is_empty() {
                "\u{2014} pick a method \u{2014}".to_string()
            } else {
                current.clone()
            })
            .show_ui(ui, |ui| {
                for m in &methods {
                    let tag = if m.is_static { "  [static]" } else { "" };
                    if ui
                        .selectable_label(
                            self.act_method.as_deref() == Some(m.name.as_str()),
                            format!("{}{tag}", m.name),
                        )
                        .clicked()
                    {
                        self.act_method = Some(m.name.clone());
                        self.act_args.clear();
                        self.act_bools.clear();
                        self.act_outcome = None;
                    }
                }
            });

        let Some(mname) = self.act_method.clone() else {
            return;
        };
        let Some(minfo) = methods.iter().find(|m| m.name == mname) else {
            return;
        };
        ui.separator();

        // Target (non-static methods need an instance).
        if minfo.is_static {
            ui.weak("target: static (class-level)");
        } else {
            ui.horizontal(|ui| {
                ui.label("target:");
                if ui.button("Load instances").clicked() {
                    self.request_instances(class.clone());
                }
                if self.act_instances_loading {
                    ui.spinner();
                }
            });
            if let Some(insts) = self.act_instances.clone() {
                let sel = self.act_target.clone();
                let text = if sel.is_empty() {
                    "\u{2014} pick instance \u{2014}".to_string()
                } else {
                    insts
                        .iter()
                        .find(|t| t.path == sel)
                        .map(|t| t.label.clone())
                        .unwrap_or(sel)
                };
                egui::ComboBox::from_id_salt("act-target")
                    .width(f32::INFINITY)
                    .selected_text(text)
                    .show_ui(ui, |ui| {
                        for t in &insts {
                            if ui
                                .selectable_label(self.act_target == t.path, &t.label)
                                .clicked()
                            {
                                self.act_target = t.path.clone();
                            }
                        }
                    });
            }
            ui.add(
                egui::TextEdit::singleline(&mut self.act_target)
                    .hint_text("or paste an object path")
                    .desired_width(f32::INFINITY),
            );
        }

        // Inputs.
        if !minfo.inputs.is_empty() {
            ui.separator();
            ui.weak("inputs:");
        }
        for (pname, ctype) in &minfo.inputs {
            let kind = param_kind(ctype);
            match kind {
                ParamKind::Bool => {
                    let b = self.act_bools.entry(pname.clone()).or_insert(false);
                    ui.checkbox(b, format!("{pname}  ({ctype})"));
                }
                ParamKind::Other => {
                    ui.weak(format!("{pname} ({ctype}) \u{2014} unsupported"));
                }
                _ => {
                    ui.label(format!("{pname}  ({ctype})"));
                    let v = self.act_args.entry(pname.clone()).or_default();
                    ui.add(egui::TextEdit::singleline(v).desired_width(f32::INFINITY));
                }
            }
        }

        ui.separator();
        let can_invoke =
            !self.act_invoking && (minfo.is_static || !self.act_target.trim().is_empty());
        ui.add_enabled_ui(can_invoke, |ui| {
            let btn = egui::Button::new(icons::labelled_styled(
                ui,
                icons::WARNING,
                &format!("Invoke {class}.{mname}"),
                egui::TextStyle::Button,
                Color32::WHITE,
            ))
            .fill(Color32::from_rgb(150, 60, 60));
            if ui.add(btn).clicked() {
                self.confirm_open = true;
            }
        });

        // Result.
        if let Some((m, outcome)) = self.act_outcome.clone() {
            ui.separator();
            ui.strong(format!("Result of {m}"));
            if let Some(rv) = &outcome.return_value {
                let color = if rv == "0" {
                    Color32::from_rgb(120, 210, 140)
                } else {
                    Color32::from_rgb(230, 180, 90)
                };
                ui.colored_label(color, format!("ReturnValue = {rv}"));
            }
            egui::ScrollArea::vertical()
                .max_height(200.0)
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    egui::Grid::new("act-out")
                        .num_columns(2)
                        .striped(true)
                        .show(ui, |ui| {
                            for (k, v) in &outcome.outputs {
                                ui.strong(k);
                                let short: String = v.chars().take(200).collect();
                                ui.label(short);
                                ui.end_row();
                            }
                        });
                });
        }
    }
}
