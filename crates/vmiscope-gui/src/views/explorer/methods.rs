//! The Methods sub-tab: one card per method, with its signature and an Invoke.

use eframe::egui;
use eframe::egui::{Align, Layout, RichText, TextStyle};

use vmiscope_core::MethodSchema;

use crate::app::VmiScopeApp;
use crate::theme::icons;
use crate::theme::tokens::muted;
use crate::widgets::button::btn_secondary;
use crate::widgets::card::{card, card_grid};
use crate::widgets::chip::tag_neutral;
use crate::widgets::loading::spinner;

/// Minimum card width, from the mock's `minmax(330px, 1fr)` grid.
const CARD_MIN_W: f32 = 330.0;

impl VmiScopeApp {
    pub(crate) fn ui_methods_tab(&mut self, ui: &mut egui::Ui) {
        let Some(class) = self.selected_class.clone() else {
            return;
        };

        let methods: Vec<MethodSchema> = match self.schema_for_selected() {
            Some(schema) => schema.methods.clone(),
            None => {
                if self.schema_loading && self.schema_class == class {
                    spinner(ui, "reflecting methods\u{2026}");
                } else {
                    ui.label(egui::RichText::new("No schema for this class.").color(muted(50)));
                }
                return;
            }
        };

        if methods.is_empty() {
            ui.label(egui::RichText::new("This class declares no methods.").color(muted(50)));
            return;
        }

        let mut invoke: Option<String> = None;
        egui::ScrollArea::vertical()
            .id_salt("methods-scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                card_grid(ui, CARD_MIN_W, &methods, |ui, m| {
                    let mut hit = false;
                    card(ui, |ui| {
                        ui.horizontal(|ui| {
                            let strong = ui.visuals().strong_text_color();
                            ui.label(icons::labelled_styled(
                                ui,
                                icons::FUNCTION,
                                &format!("{}()", m.name),
                                TextStyle::Body,
                                strong,
                            ));
                            // The static/instance scope tag matches `is_static` (which the
                            // core widens past the often-absent `Static` qualifier).
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                tag_neutral(ui, if m.is_static { "static" } else { "instance" });
                            });
                        });
                        ui.add(egui::Label::new(
                            RichText::new(method_signature(m))
                                .text_style(TextStyle::Monospace)
                                .size(11.5)
                                .color(muted(72)),
                        ));
                        if let Some(desc) = &m.description {
                            if !desc.is_empty() {
                                ui.label(
                                    RichText::new(desc)
                                        .text_style(TextStyle::Small)
                                        .color(muted(48)),
                                );
                            }
                        }
                        if btn_secondary(ui, icons::labelled(ui, icons::LIGHTNING, "Invoke"))
                            .clicked()
                        {
                            hit = true;
                        }
                    });
                    if hit {
                        invoke = Some(m.name.clone());
                    }
                });
            });

        if let Some(method) = invoke {
            // Open the Actions panel with this method preselected. Its own gate
            // (and the confirm dialog) still stand -- this only reveals it.
            self.actions_open = true;
            self.act_method = Some(method);
            self.act_args.clear();
            self.act_bools.clear();
            self.act_outcome = None;
            self.request_schema(class);
        }
    }
}

/// A method's one-line signature: `<return> Name(<in params>)`.
///
/// `ReturnValue` lives among the out-params; every other out-param is a value
/// the provider writes back and is not part of the call's surface here.
fn method_signature(m: &MethodSchema) -> String {
    let ins: Vec<String> = m
        .in_params
        .iter()
        .map(|p| format!("{} {}", p.cim_type, p.name))
        .collect();
    let ret = m
        .out_params
        .iter()
        .find(|p| p.name.eq_ignore_ascii_case("ReturnValue"))
        .map(|p| p.cim_type.clone())
        .unwrap_or_else(|| "void".to_string());
    format!("{ret} {}({})", m.name, ins.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use vmiscope_core::ParamSchema;

    fn param(name: &str, cim: &str) -> ParamSchema {
        ParamSchema {
            name: name.to_string(),
            cim_type: cim.to_string(),
            ..Default::default()
        }
    }

    /// `Win32_Process.Create(CommandLine, CurrentDirectory, ProcessStartup)`
    /// returns a `uint32` -- the signature has to read as that call, with the
    /// return type in front and `ReturnValue` not repeated as a parameter.
    #[test]
    fn signature_puts_the_return_first_and_omits_returnvalue() {
        let m = MethodSchema {
            name: "Create".into(),
            in_params: vec![
                param("CommandLine", "string"),
                param("CurrentDirectory", "string"),
            ],
            out_params: vec![param("ProcessId", "uint32"), param("ReturnValue", "uint32")],
            ..Default::default()
        };
        assert_eq!(
            method_signature(&m),
            "uint32 Create(string CommandLine, string CurrentDirectory)"
        );
    }

    /// A method with no `ReturnValue` out-param reads as `void`, not as empty.
    #[test]
    fn a_method_without_a_return_value_is_void() {
        let m = MethodSchema {
            name: "Reset".into(),
            ..Default::default()
        };
        assert_eq!(method_signature(&m), "void Reset()");
    }
}
