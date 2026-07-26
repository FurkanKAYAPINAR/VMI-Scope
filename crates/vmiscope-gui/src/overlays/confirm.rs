//! The confirmation gate in front of every method invocation.

use eframe::egui;
use egui::Color32;

use crate::app::VmiScopeApp;
use crate::util::is_dangerous_method;

use vmiscope_core::{param_kind, MethodArg, ParamKind};

impl VmiScopeApp {
    pub(crate) fn ui_confirm_window(&mut self, ctx: &egui::Context) {
        if !self.confirm_open {
            return;
        }
        let class = self.selected_class.clone().unwrap_or_default();
        let method = self.act_method.clone().unwrap_or_default();
        if class.is_empty() || method.is_empty() {
            self.confirm_open = false;
            return;
        }

        // Reconstruct the argument list from the current inputs + schema signature.
        //
        // If the schema is gone (a namespace switch landed between opening this
        // dialog and confirming it, say) we cannot know whether the method is
        // static or what it takes. Refusing is the only safe answer: guessing
        // "static, no arguments" would send an instance method at the class path
        // and drop every argument the user typed, which for something like
        // Terminate is a silently different operation from the one they
        // confirmed.
        let Some((is_static, inputs)) = self
            .schema
            .as_ref()
            .and_then(|s| s.methods.iter().find(|m| m.name == method))
            .map(|m| {
                (
                    m.is_static,
                    m.in_params
                        .iter()
                        .map(|p| (p.name.clone(), p.cim_type.clone()))
                        .collect::<Vec<(String, String)>>(),
                )
            })
        else {
            self.confirm_open = false;
            self.push_error(format!(
                "{class}.{method}: the class schema is no longer loaded, so the call was not made. \
                 Reselect the class and try again."
            ));
            return;
        };
        let target = self.act_target.clone();
        let ns = self.active_ns.clone();
        let mut args: Vec<MethodArg> = Vec::new();
        for (pname, ctype) in &inputs {
            let kind = param_kind(ctype);
            let value = match kind {
                ParamKind::Bool => self
                    .act_bools
                    .get(pname)
                    .copied()
                    .unwrap_or(false)
                    .to_string(),
                _ => self.act_args.get(pname).cloned().unwrap_or_default(),
            };
            args.push(MethodArg {
                name: pname.clone(),
                kind,
                value,
            });
        }

        let mut open = true;
        let mut do_invoke = false;
        let mut cancel = false;
        egui::Window::new("Confirm invocation")
            .collapsible(false)
            .resizable(false)
            .open(&mut open)
            .show(ctx, |ui| {
                ui.colored_label(
                    Color32::from_rgb(240, 120, 120),
                    "\u{26a0} This invokes a WMI method and may change system state.",
                );
                if is_dangerous_method(&method) {
                    ui.colored_label(
                        Color32::from_rgb(255, 80, 80),
                        format!("\u{201c}{method}\u{201d} looks destructive \u{2014} double-check the target."),
                    );
                }
                ui.separator();
                egui::Grid::new("confirm-grid").num_columns(2).show(ui, |ui| {
                    ui.strong("Namespace");
                    ui.label(&ns);
                    ui.end_row();
                    ui.strong("Class");
                    ui.label(&class);
                    ui.end_row();
                    ui.strong("Method");
                    ui.label(&method);
                    ui.end_row();
                    ui.strong("Target");
                    ui.label(if is_static {
                        "(static)"
                    } else if target.is_empty() {
                        "(none)"
                    } else {
                        target.as_str()
                    });
                    ui.end_row();
                });
                if !args.is_empty() {
                    ui.separator();
                    ui.strong("Arguments");
                    for a in &args {
                        let shown = if a.value.trim().is_empty() {
                            "(provider default)".to_string()
                        } else {
                            a.value.clone()
                        };
                        ui.label(format!("  {} = {shown}", a.name));
                    }
                }
                ui.separator();
                ui.horizontal(|ui| {
                    if ui.button("Cancel").clicked() {
                        cancel = true;
                    }
                    let go = egui::Button::new(
                        egui::RichText::new("Yes, invoke").color(Color32::WHITE),
                    )
                    .fill(Color32::from_rgb(150, 60, 60));
                    if ui.add(go).clicked() {
                        do_invoke = true;
                    }
                });
            });

        if cancel || !open {
            self.confirm_open = false;
        }
        if do_invoke {
            self.confirm_open = false;
            self.request_invoke(class, target, method, is_static, args);
        }
    }
}
