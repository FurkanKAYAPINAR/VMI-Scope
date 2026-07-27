//! The Actions side panel: pick a WMI method, fill its inputs, and invoke it.

use eframe::egui;

use crate::app::VmiScopeApp;
use crate::overlays::btn_danger;
use crate::theme::icons;
use crate::theme::tokens::{BAD, OK, WARN};
use crate::widgets::button::btn_secondary;
use crate::widgets::field::mono_input;
use crate::widgets::kv::kv_grid_sized;
use crate::widgets::loading::spinner;
use crate::widgets::rule::hrule;

use vmiscope_core::{param_kind, ParamKind};

/// How much of a method output is shown inline. The full value is still in the
/// object; this pane is a receipt, not a viewer.
const OUTPUT_CHARS: usize = 200;

impl VmiScopeApp {
    // ------------------------------------------------------------------
    // UI: Actions (method execution)
    // ------------------------------------------------------------------

    pub(crate) fn ui_actions(&mut self, ui: &mut egui::Ui) {
        // Object paths and method arguments are long, and both fields used to
        // be full-bleed. `spacing.text_edit_width` is where a kit input takes
        // its width from, and egui defaults it to 280.
        ui.spacing_mut().text_edit_width = ui.available_width();
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
                spinner(ui, "invoking");
            }
        });
        ui.weak("invoke WMI methods \u{2014} may change system state");
        hrule(ui);

        let Some(class) = self.selected_class.clone() else {
            ui.weak("Select a class first.");
            return;
        };

        // Method signatures come from the reflected schema.
        if self.schema_class != class || self.schema.is_none() {
            spinner(ui, "loading class methods\u{2026}");
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

        // Method picker. `widgets::field::combo` takes a fixed `&[(T, &str)]`;
        // this list is reflected per class, so it stays hand-rolled.
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
        hrule(ui);

        // Target (non-static methods need an instance).
        if minfo.is_static {
            ui.weak("target: static (class-level)");
        } else {
            ui.horizontal(|ui| {
                ui.label("target:");
                if btn_secondary(ui, "Load instances").clicked() {
                    self.request_instances(class.clone());
                }
                if self.act_instances_loading {
                    spinner(ui, "listing");
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
            mono_input(ui, &mut self.act_target, "or paste an object path");
        }

        // Inputs.
        if !minfo.inputs.is_empty() {
            hrule(ui);
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
                    mono_input(ui, v, "");
                }
            }
        }

        hrule(ui);
        let can_invoke =
            !self.act_invoking && (minfo.is_static || !self.act_target.trim().is_empty());
        ui.add_enabled_ui(can_invoke, |ui| {
            // The one filled button in the app; see `overlays::btn_danger` for
            // why this gate is not allowed to look like every other action.
            let label = icons::labelled_styled(
                ui,
                icons::WARNING,
                &format!("Invoke {class}.{mname}"),
                egui::TextStyle::Button,
                BAD,
            );
            if btn_danger(ui, label).clicked() {
                self.confirm_open = true;
            }
        });

        // Result.
        if let Some((m, outcome)) = self.act_outcome.clone() {
            hrule(ui);
            ui.strong(format!("Result of {m}"));
            if let Some(rv) = &outcome.return_value {
                // WMI's convention: 0 is success, anything else is a provider
                // status code the caller has to go and look up.
                let color = if rv == "0" { OK } else { WARN };
                ui.colored_label(color, format!("ReturnValue = {rv}"));
            }
            let outputs: Vec<(String, String)> = outcome
                .outputs
                .iter()
                .map(|(k, v)| (k.clone(), v.chars().take(OUTPUT_CHARS).collect()))
                .collect();
            egui::ScrollArea::vertical()
                .max_height(200.0)
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    kv_grid_sized(
                        ui,
                        "act-out",
                        110.0,
                        outputs.iter().map(|(k, v)| (k.as_str(), v.as_str())),
                    );
                });
        }
    }
}
