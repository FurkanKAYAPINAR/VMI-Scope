//! The method-invocation gate.
//!
//! This is one `egui::Modal` that merges what used to be two surfaces -- the
//! right-hand Actions panel (pick a method, fill its inputs) and a separate
//! confirm `Window` in front of the call. Merging them is deliberate: a
//! destructive action should be arranged, reviewed and fired without the target
//! ever leaving the screen, and a modal is the one surface that dims everything
//! else while it is up.
//!
//! It stays a *gate*. Three things are load-bearing and are called out where
//! they happen: the invocation is never fired on the first click (there is an
//! explicit confirm step), a destructive-looking method name earns an extra
//! warning, and every mutating call is audited -- the last via
//! `request_invoke`, which writes `config::append_audit` before it sends. The
//! modal also *declines* rather than guesses: if the class schema is not loaded
//! it cannot know whether a method is static or what it takes, and sending an
//! instance method at the class path with the typed arguments dropped is a
//! silently different operation from the one the operator confirmed.

use eframe::egui;
use eframe::egui::{RichText, TextStyle};

use crate::app::VmiScopeApp;
use crate::overlays::btn_danger;
use crate::theme::icons;
use crate::theme::tokens::{muted, BAD, OK, WARN};
use crate::util::is_dangerous_method;
use crate::widgets::button::btn_secondary;
use crate::widgets::card::card;
use crate::widgets::field::mono_input;
use crate::widgets::kv::kv_grid_sized;
use crate::widgets::loading::spinner;
use crate::widgets::rule::hrule;

use vmiscope_core::{param_kind, MethodArg, ParamKind};

/// Fixed modal width. The signature line and the command preview are the widest
/// content, and a floor keeps the whole dialog from resizing as the target path
/// changes under it.
const MODAL_W: f32 = 500.0;
/// How tall the scrollable middle (picker, target, params, preview, result) may
/// grow before it scrolls, so the review-and-fire buttons below it are always in
/// reach.
const BODY_H: f32 = 460.0;
/// How much of a method output is shown inline. The full value is still on the
/// returned object; this pane is a receipt, not a viewer.
const OUTPUT_CHARS: usize = 200;
/// Width for the full-bleed fields (the pickers and the text inputs). A concrete
/// number, not `f32::INFINITY` or `available_width()`: a modal auto-sizes, so an
/// infinite child width has no finite parent to clamp against and lands in the
/// layout as a NaN `max_rect`, which egui asserts on.
const FIELD_W: f32 = 452.0;

/// A method's shape, owned so the schema borrow is released before the widgets
/// (which need `&mut self`) run.
struct MInfo {
    name: String,
    is_static: bool,
    /// Return type, for the signature line.
    ret: String,
    /// `(name, cim_type, required)` per input parameter. `required` is the
    /// negation of the `Optional` qualifier and drives the required marker.
    inputs: Vec<(String, String, bool)>,
}

impl VmiScopeApp {
    /// The Actions-panel trampoline.
    ///
    /// `views::explorer` still opens the old right-hand panel by raising
    /// `actions_open` and calling this from inside a `Panel::right`. The panel
    /// body is gone -- the invoke UI is the modal below -- so this only hands the
    /// open state across (`actions_open` -> `invoke_open`) and asks for an
    /// immediate repaint, which keeps the now-empty panel from being visible for
    /// more than the single frame it takes the flag to flip. Deleting the panel
    /// and this trampoline, and repointing the triggers straight at
    /// `invoke_open`, is the explorer view's half of task 3.32.
    pub(crate) fn ui_actions(&mut self, ui: &mut egui::Ui) {
        if self.actions_open {
            self.actions_open = false;
            self.invoke_open = true;
            // A fresh open always starts disarmed, so an Escape-close that left
            // the confirm step armed cannot bring it back armed.
            self.act_armed = false;
            ui.ctx().request_repaint();
        }
    }

    /// The invoke modal. Dispatched once per frame from `app::ui`.
    pub(crate) fn ui_invoke_modal(&mut self, ctx: &egui::Context) {
        if !self.invoke_open {
            return;
        }
        let now = ctx.input(|i| i.time);

        let Some(class) = self.selected_class.clone() else {
            // Nothing is selected, so there is nothing to invoke. Close rather
            // than sit open over an empty detail pane.
            self.close_invoke();
            return;
        };

        // Capture the elapsed time the first frame a fresh outcome is in hand.
        // `MethodOutcome` carries no timing, so it is measured here between the
        // send (`act_invoke_started`) and the reply landing (`act_invoking`
        // clears in `handle_responses`). Guarded by `is_none` so it is written
        // once and then held steady rather than growing with the frame clock.
        if !self.act_invoking && self.act_outcome.is_some() && self.act_elapsed_ms.is_none() {
            if let Some(t0) = self.act_invoke_started {
                self.act_elapsed_ms = Some(((now - t0) * 1000.0).max(0.0) as u64);
            }
        }

        // Method metadata, but only if the schema belongs to THIS class. When it
        // does not we decline (below) instead of guessing a shape.
        let schema_ready = self.schema.is_some() && self.schema_class == class;
        let methods: Vec<MInfo> = if schema_ready {
            self.schema
                .as_ref()
                .unwrap()
                .methods
                .iter()
                .map(|m| MInfo {
                    name: m.name.clone(),
                    is_static: m.is_static,
                    ret: m
                        .out_params
                        .iter()
                        .find(|p| p.name.eq_ignore_ascii_case("ReturnValue"))
                        .map(|p| p.cim_type.clone())
                        .unwrap_or_else(|| "void".to_string()),
                    inputs: m
                        .in_params
                        .iter()
                        .map(|p| (p.name.clone(), p.cim_type.clone(), !p.optional))
                        .collect(),
                })
                .collect()
        } else {
            Vec::new()
        };

        let principal = self.runs_under();

        let id = egui::Id::new("vs_invoke_modal");
        let modal = egui::Modal::new(id).show(ctx, |ui| {
            self.invoke_modal_body(ui, &class, schema_ready, &methods, &principal)
        });

        if modal.inner || modal.should_close() {
            self.close_invoke();
        }
    }

    /// The modal contents. Returns `true` when it has asked to be closed.
    fn invoke_modal_body(
        &mut self,
        ui: &mut egui::Ui,
        class: &str,
        schema_ready: bool,
        methods: &[MInfo],
        principal: &str,
    ) -> bool {
        ui.set_width(MODAL_W);
        // Cap the max as well: the modal's content ui starts unbounded, and an
        // unbounded width turns any `available_width()` read below into infinity.
        ui.set_max_width(MODAL_W);

        let strong = ui.visuals().strong_text_color();
        ui.label(icons::labelled_styled(
            ui,
            icons::LIGHTNING,
            "Invoke method",
            TextStyle::Body,
            strong,
        ));
        ui.label(
            RichText::new(format!("on {class}"))
                .text_style(TextStyle::Small)
                .color(muted(55)),
        );
        hrule(ui);

        // Decline when the schema is not loaded. Guessing "static, no arguments"
        // here is precisely the mistake this branch exists to prevent.
        if !schema_ready {
            if self.schema_loading && self.schema_class == class {
                spinner(ui, "reflecting class methods\u{2026}");
            } else {
                ui.label(
                    RichText::new(
                        "The class schema is not loaded, so no method can be invoked. \
                         Reselect the class and try again.",
                    )
                    .color(muted(60)),
                );
            }
            hrule(ui);
            return btn_secondary(ui, "Close").clicked();
        }
        if methods.is_empty() {
            ui.label(RichText::new("This class declares no methods.").color(muted(55)));
            hrule(ui);
            return btn_secondary(ui, "Close").clicked();
        }

        // The scrollable middle. It returns everything the fixed footer needs so
        // the review/fire buttons stay reachable no matter how many parameters a
        // method takes.
        let body = egui::ScrollArea::vertical()
            .id_salt("vs-invoke-body")
            .max_height(BODY_H)
            .auto_shrink([false, false])
            .show(ui, |ui| self.invoke_form(ui, class, methods));

        let Some((method, is_static, args, edited)) = body.inner else {
            // No method picked yet.
            hrule(ui);
            return ui
                .horizontal(|ui| btn_secondary(ui, "Cancel").clicked())
                .inner;
        };

        // Any edit disarms the confirm step: the operator must be looking at the
        // command they are about to run when they confirm it, not one they typed
        // before changing their mind.
        if edited {
            self.act_armed = false;
        }

        hrule(ui);

        // The principal the call runs as. On the local/SSO path this is the
        // current interactive user; with alternate credentials it is the account
        // the worker binds with. Either way it answers "on whose authority".
        ui.label(
            RichText::new(format!("Runs under {principal}"))
                .text_style(TextStyle::Small)
                .color(muted(60)),
        );

        // Two levels, two tokens. The standing caution every invocation carries
        // is WARN; the name-based escalation is BAD. Painting both red would make
        // the second read as a restatement rather than a step up.
        ui.label(icons::labelled_styled(
            ui,
            icons::WARNING,
            "This invokes a WMI method and may change system state.",
            TextStyle::Small,
            WARN,
        ));
        if is_dangerous_method(&method) {
            ui.label(icons::labelled_styled(
                ui,
                icons::SHIELD_WARNING,
                &format!(
                    "\u{201c}{method}\u{201d} looks destructive \u{2014} double-check the target."
                ),
                TextStyle::Small,
                BAD,
            ));
        }

        let can_invoke = !self.act_invoking && (is_static || !self.act_target.trim().is_empty());

        let mut want_close = false;
        ui.horizontal(|ui| {
            if self.act_invoking {
                spinner(ui, "invoking");
                return;
            }
            if !self.act_armed {
                if btn_secondary(ui, "Cancel").clicked() {
                    want_close = true;
                }
                ui.add_enabled_ui(can_invoke, |ui| {
                    let label = icons::labelled_styled(
                        ui,
                        icons::WARNING,
                        &format!("Review \u{2014} invoke {class}.{method}"),
                        TextStyle::Button,
                        BAD,
                    );
                    if btn_danger(ui, label).clicked() {
                        self.act_armed = true;
                    }
                });
            } else {
                // The confirm step. This is the click that fires.
                if btn_secondary(ui, "Back").clicked() {
                    self.act_armed = false;
                }
                let label = icons::labelled_styled(
                    ui,
                    icons::LIGHTNING,
                    "Yes, invoke now",
                    TextStyle::Button,
                    BAD,
                );
                if btn_danger(ui, label).clicked() {
                    self.fire_invoke(class, &method, is_static, args, ui.ctx());
                }
            }
        });

        want_close
    }

    /// The picker, target, parameter fields, live preview and result -- the part
    /// that scrolls. Returns the selected method, its static-ness, the collected
    /// arguments and whether anything was edited this frame; `None` while no
    /// method is picked.
    fn invoke_form(
        &mut self,
        ui: &mut egui::Ui,
        class: &str,
        methods: &[MInfo],
    ) -> Option<(String, bool, Vec<MethodArg>, bool)> {
        // Method picker. Reflected per class, so it stays hand-rolled rather than
        // using the kit's fixed-list combo.
        let current = self.act_method.clone().unwrap_or_default();
        egui::ComboBox::from_id_salt("vs-invoke-method")
            .width(FIELD_W)
            .selected_text(if current.is_empty() {
                "\u{2014} pick a method \u{2014}".to_string()
            } else {
                current.clone()
            })
            .show_ui(ui, |ui| {
                for m in methods {
                    let tag = if m.is_static { "  [static]" } else { "" };
                    if ui
                        .selectable_label(
                            self.act_method.as_deref() == Some(m.name.as_str()),
                            format!("{}{tag}", m.name),
                        )
                        .clicked()
                        && self.act_method.as_deref() != Some(m.name.as_str())
                    {
                        // A new method is a fresh call: drop the old arguments,
                        // result and any armed confirmation.
                        self.act_method = Some(m.name.clone());
                        self.act_args.clear();
                        self.act_bools.clear();
                        self.act_outcome = None;
                        self.act_elapsed_ms = None;
                        self.act_armed = false;
                    }
                }
            });

        let mname = self.act_method.clone()?;
        let minfo = methods.iter().find(|m| m.name == mname)?;

        // Signature line: `<return> Class.Method(<in params>)`.
        let sig_ins: Vec<String> = minfo
            .inputs
            .iter()
            .map(|(n, t, _)| format!("{t} {n}"))
            .collect();
        ui.add(egui::Label::new(
            RichText::new(format!(
                "{} {class}.{}({})",
                minfo.ret,
                minfo.name,
                sig_ins.join(", ")
            ))
            .text_style(TextStyle::Monospace)
            .size(11.5)
            .color(muted(78)),
        ));

        let mut edited = false;

        // Target. A static method takes the class path; an instance method needs
        // an object path, loaded or pasted.
        hrule(ui);
        if minfo.is_static {
            ui.label(
                RichText::new("target: static (class-level)")
                    .text_style(TextStyle::Small)
                    .color(muted(55)),
            );
        } else {
            ui.horizontal(|ui| {
                ui.label(RichText::new("target:").text_style(TextStyle::Small));
                if btn_secondary(ui, "Load instances").clicked() {
                    self.request_instances(class.to_string());
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
                egui::ComboBox::from_id_salt("vs-invoke-target")
                    .width(FIELD_W)
                    .selected_text(text)
                    .show_ui(ui, |ui| {
                        for t in &insts {
                            if ui
                                .selectable_label(self.act_target == t.path, &t.label)
                                .clicked()
                                && self.act_target != t.path
                            {
                                self.act_target = t.path.clone();
                                edited = true;
                            }
                        }
                    });
            }
            ui.spacing_mut().text_edit_width = FIELD_W;
            if mono_input(ui, &mut self.act_target, "or paste an object path").changed() {
                edited = true;
            }
        }

        // Inputs, each with a required marker.
        if !minfo.inputs.is_empty() {
            hrule(ui);
            ui.label(
                RichText::new("arguments")
                    .text_style(TextStyle::Small)
                    .color(muted(55)),
            );
        }
        for (pname, ctype, required) in &minfo.inputs {
            let kind = param_kind(ctype);
            ui.horizontal(|ui| {
                ui.label(RichText::new(pname).text_style(TextStyle::Small));
                ui.label(
                    RichText::new(format!("({ctype})"))
                        .text_style(TextStyle::Small)
                        .color(muted(48)),
                );
                if *required {
                    ui.label(
                        RichText::new("required")
                            .text_style(TextStyle::Small)
                            .color(WARN),
                    );
                } else {
                    ui.label(
                        RichText::new("optional")
                            .text_style(TextStyle::Small)
                            .color(muted(40)),
                    );
                }
            });
            match kind {
                ParamKind::Bool => {
                    let b = self.act_bools.entry(pname.clone()).or_insert(false);
                    if ui.checkbox(b, "true").changed() {
                        edited = true;
                    }
                }
                ParamKind::Other => {
                    ui.label(
                        RichText::new("unsupported type \u{2014} cannot be supplied here")
                            .text_style(TextStyle::Small)
                            .color(muted(45)),
                    );
                }
                _ => {
                    ui.spacing_mut().text_edit_width = FIELD_W;
                    let v = self.act_args.entry(pname.clone()).or_default();
                    if mono_input(ui, v, "").changed() {
                        edited = true;
                    }
                }
            }
        }

        // Collect the arguments once, from this frame's field state, for both the
        // preview and the eventual send.
        let args: Vec<MethodArg> = minfo
            .inputs
            .iter()
            .map(|(pname, ctype, _)| {
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
                MethodArg {
                    name: pname.clone(),
                    kind,
                    value,
                }
            })
            .collect();

        // Live command preview.
        hrule(ui);
        ui.label(
            RichText::new("preview")
                .text_style(TextStyle::Small)
                .color(muted(55)),
        );
        let preview = command_preview(&self.active_ns, class, minfo, &self.act_target, &args);
        card(ui, |ui| {
            ui.add(
                egui::Label::new(
                    RichText::new(&preview)
                        .text_style(TextStyle::Monospace)
                        .size(11.0)
                        .color(muted(80)),
                )
                .selectable(true)
                .wrap(),
            );
        });

        // Result of the most recent call, when it belongs to this method.
        if let Some((done_method, outcome)) = self.act_outcome.clone() {
            if done_method == minfo.name {
                hrule(ui);
                ui.label(icons::labelled_styled(
                    ui,
                    icons::CHECK_CIRCLE,
                    &format!("Result of {done_method}"),
                    TextStyle::Small,
                    strong_or_muted(ui),
                ));
                if let Some(rv) = &outcome.return_value {
                    // WMI's convention: 0 is success; anything else is a provider
                    // status code the caller has to go and look up.
                    let color = if rv == "0" { OK } else { WARN };
                    ui.label(RichText::new(format!("ReturnValue = {rv}")).color(color));
                }
                if let Some(ms) = self.act_elapsed_ms {
                    ui.label(
                        RichText::new(format!("elapsed {ms} ms"))
                            .text_style(TextStyle::Small)
                            .color(muted(50)),
                    );
                }
                let outputs: Vec<(String, String)> = outcome
                    .outputs
                    .iter()
                    .map(|(k, v)| (k.clone(), v.chars().take(OUTPUT_CHARS).collect()))
                    .collect();
                if !outputs.is_empty() {
                    kv_grid_sized(
                        ui,
                        "vs-invoke-out",
                        110.0,
                        outputs.iter().map(|(k, v)| (k.as_str(), v.as_str())),
                    );
                }
            }
        }

        Some((minfo.name.clone(), minfo.is_static, args, edited))
    }

    /// Fire the confirmed call. The audit line is written inside
    /// `request_invoke` (`config::append_audit`), so every path that reaches WMI
    /// is recorded; the confirm gate is upstream of here.
    fn fire_invoke(
        &mut self,
        class: &str,
        method: &str,
        is_static: bool,
        args: Vec<MethodArg>,
        ctx: &egui::Context,
    ) {
        self.act_armed = false;
        self.act_elapsed_ms = None;
        self.act_invoke_started = Some(ctx.input(|i| i.time));
        let target = self.act_target.clone();
        self.request_invoke(
            class.to_string(),
            target,
            method.to_string(),
            is_static,
            args,
        );
    }

    fn close_invoke(&mut self) {
        self.invoke_open = false;
        self.act_armed = false;
    }

    /// The account the invocation runs as: the alternate credentials when the
    /// session is bound with them, otherwise the current interactive user.
    fn runs_under(&self) -> String {
        if self.conn_use_creds && !self.conn_user.trim().is_empty() {
            let domain = if self.conn_domain.trim().is_empty() {
                self.conn_host.trim()
            } else {
                self.conn_domain.trim()
            };
            if domain.is_empty() {
                self.conn_user.trim().to_string()
            } else {
                format!("{domain}\\{}", self.conn_user.trim())
            }
        } else {
            let domain = std::env::var("USERDOMAIN").unwrap_or_default();
            let user = std::env::var("USERNAME").unwrap_or_default();
            match (domain.is_empty(), user.is_empty()) {
                (false, _) => format!("{domain}\\{user}"),
                (true, false) => user,
                (true, true) => "the current user".to_string(),
            }
        }
    }
}

fn strong_or_muted(ui: &egui::Ui) -> egui::Color32 {
    ui.visuals().strong_text_color()
}

/// A PowerShell `Invoke-CimMethod` line that mirrors the call being arranged --
/// a preview, not the wire form. An instance target is rendered as a `-Query`
/// derived from its object path; a static method uses `-ClassName`.
fn command_preview(
    namespace: &str,
    class: &str,
    minfo: &MInfo,
    target: &str,
    args: &[MethodArg],
) -> String {
    let ns = namespace.trim();
    let mut s = format!("Invoke-CimMethod -Namespace '{ns}' ");
    if minfo.is_static {
        s.push_str(&format!("-ClassName {class} "));
    } else {
        let where_clause = path_to_where(target, class);
        if where_clause.is_empty() {
            s.push_str(&format!("-Query \"SELECT * FROM {class}\" "));
        } else {
            s.push_str(&format!(
                "-Query \"SELECT * FROM {class} WHERE {where_clause}\" "
            ));
        }
    }
    s.push_str(&format!("-MethodName {}", minfo.name));
    if !args.is_empty() {
        let pairs: Vec<String> = args
            .iter()
            .map(|a| format!("{} = {}", a.name, ps_value(a)))
            .collect();
        s.push_str(&format!(" -Arguments @{{ {} }}", pairs.join("; ")));
    }
    s
}

/// The WHERE clause of a preview query, recovered from an object's relative
/// path (`Class.Key="v",Key2="w"`). Best effort: a full path that does not carry
/// the `Class.` prefix is shown as-is, since the preview is illustrative.
fn path_to_where(path: &str, class: &str) -> String {
    let p = path.trim();
    if p.is_empty() {
        return String::new();
    }
    let rel = p.strip_prefix(&format!("{class}.")).unwrap_or(p);
    rel.replace(',', " AND ")
}

fn ps_value(a: &MethodArg) -> String {
    match a.kind {
        ParamKind::Bool => {
            if a.value.eq_ignore_ascii_case("true") || a.value.trim() == "1" {
                "$true".to_string()
            } else {
                "$false".to_string()
            }
        }
        ParamKind::Uint | ParamKind::Sint | ParamKind::Real => {
            let v = a.value.trim();
            if v.is_empty() {
                "''".to_string()
            } else if v.parse::<f64>().is_ok() {
                v.to_string()
            } else {
                ps_quote(v)
            }
        }
        _ => ps_quote(a.value.trim()),
    }
}

/// Single-quote a value for PowerShell, doubling any embedded quote.
fn ps_quote(v: &str) -> String {
    format!("'{}'", v.replace('\'', "''"))
}
