//! The Schema sub-tab: derivation and associations on the left, class
//! qualifiers and inline MOF on the right.

use eframe::egui;
use eframe::egui::{RichText, TextStyle};

use vmiscope_core::AssocInfo;

use crate::app::VmiScopeApp;
use crate::theme::icons;
use crate::theme::tokens::{muted, S1, S4};
use crate::widgets::button::{accent, btn_icon};
use crate::widgets::codeview::{code_panel, Lang};
use crate::widgets::kv::kv_grid;
use crate::widgets::loading::{partial_note, spinner};

/// Indent per derivation step. Not on the density scale -- the indent is the
/// hierarchy.
const INDENT: f32 = 14.0;

impl VmiScopeApp {
    pub(crate) fn ui_schema_tab(&mut self, ui: &mut egui::Ui) {
        let Some(class) = self.selected_class.clone() else {
            return;
        };

        // Both lazy and both guarded: the associations lookup and the MOF fetch
        // fire the first time the Schema tab is shown for a class, not on every
        // frame.
        self.request_associations(class.clone());
        self.request_class_mof_inline(class.clone());

        // The derivation chain, the association list and the MOF can each run
        // long, so the whole two-column body scrolls rather than being clipped
        // at the pane's bottom edge (task 3.33: no view shows content it cannot
        // reach).
        egui::ScrollArea::vertical()
            .id_salt("schema-scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.columns(2, |cols| {
                    self.ui_schema_relationships(&mut cols[0], &class);
                    self.ui_schema_definition(&mut cols[1], &class);
                });
            });
    }

    /// Left column: the derivation chain and the association list.
    fn ui_schema_relationships(&self, ui: &mut egui::Ui, class: &str) {
        section(ui, "Derivation");
        // The current class leads, in the accent; each ancestor is indented
        // under it, nearest first.
        ui.add(egui::Label::new(RichText::new(class).color(accent(ui))).selectable(false));
        match self.schema_for_selected() {
            Some(schema) => {
                if schema.derivation.is_empty() {
                    ui.label(RichText::new("(root class)").color(muted(45)));
                }
                for (i, ancestor) in schema.derivation.iter().enumerate() {
                    ui.horizontal(|ui| {
                        ui.add_space((i as f32 + 1.0) * INDENT);
                        ui.add(egui::Label::new(
                            icons::glyph(icons::ARROW_ELBOW_DOWN_RIGHT)
                                .size(11.0)
                                .color(muted(35)),
                        ));
                        ui.label(RichText::new(ancestor).color(muted(72)));
                    });
                }
            }
            None if self.schema_loading && self.schema_class == class => {
                spinner(ui, "reflecting derivation\u{2026}");
            }
            None => {
                ui.label(RichText::new("derivation unavailable").color(muted(45)));
            }
        }

        ui.add_space(S4);
        section(ui, "Associations");
        if self.assoc_class != class {
            // Request just fired; the reply will land within a frame or two.
            spinner(ui, "finding associations\u{2026}");
            return;
        }
        if self.assoc_loading {
            spinner(ui, "finding associations\u{2026}");
        }
        if let Some(assocs) = &self.associations {
            partial_note(ui, self.assoc_completion.note());
            if assocs.is_empty() {
                ui.label(RichText::new("No associations.").color(muted(50)));
            }
            for a in assocs {
                ui.add(egui::Label::new(icons::labelled_styled(
                    ui,
                    icons::LINK_SIMPLE,
                    &a.target_class,
                    TextStyle::Body,
                    muted(85),
                )));
                let sub = assoc_subtext(a);
                if !sub.is_empty() {
                    // Wraps within the column: an inherited, self-referencing note
                    // is long and would otherwise bleed into the qualifiers column.
                    ui.add(
                        egui::Label::new(
                            RichText::new(sub)
                                .text_style(TextStyle::Small)
                                .color(muted(42)),
                        )
                        .wrap(),
                    );
                }
            }
        }
    }

    /// Right column: the class qualifiers table and the inline MOF panel.
    fn ui_schema_definition(&self, ui: &mut egui::Ui, class: &str) {
        section(ui, "Qualifiers");
        match self.schema_for_selected() {
            Some(schema) if schema.qualifiers.is_empty() => {
                ui.label(RichText::new("No class qualifiers.").color(muted(50)));
            }
            Some(schema) => {
                kv_grid(
                    ui,
                    "schema-qualifiers",
                    schema
                        .qualifiers
                        .iter()
                        .map(|(n, v)| (n.as_str(), v.as_str())),
                );
            }
            None if self.schema_loading && self.schema_class == class => {
                spinner(ui, "reading qualifiers\u{2026}");
            }
            None => {
                ui.label(RichText::new("qualifiers unavailable").color(muted(45)));
            }
        }

        ui.add_space(S4);
        section(ui, "MOF");
        // The MOF loads inline here; the floating MOF window is superseded (task
        // 3.28) -- `request_class_mof_inline` deliberately never raises `mof_open`.
        if self.mof_object_path != class {
            spinner(ui, "loading MOF\u{2026}");
            return;
        }
        if self.mof_loading {
            spinner(ui, "loading MOF\u{2026}");
        }
        if let Some(mof) = &self.mof_text {
            ui.horizontal(|ui| {
                if btn_icon(ui, icons::COPY)
                    .on_hover_text("Copy MOF")
                    .clicked()
                {
                    ui.ctx().copy_text(mof.clone());
                }
            });
            // No guide -- the MOF is the provider's text, not ours.
            code_panel(ui, mof, Lang::Mof, None);
        } else if !self.mof_loading {
            ui.label(RichText::new("MOF unavailable.").color(muted(45)));
        }
    }
}

/// A section heading: small, uppercase, letter-spaced, muted.
fn section(ui: &mut egui::Ui, text: &str) {
    ui.add(
        egui::Label::new(
            RichText::new(text.to_uppercase())
                .text_style(TextStyle::Small)
                .extra_letter_spacing(0.5)
                .color(muted(55)),
        )
        .selectable(false),
    );
    ui.add_space(S1);
}

/// The one-line note under an association: how it is reached and any caveat.
fn assoc_subtext(a: &AssocInfo) -> String {
    let mut parts: Vec<String> = Vec::new();
    if !a.assoc_class.is_empty() {
        parts.push(format!("via {}", a.assoc_class));
    }
    if !a.role.is_empty() {
        parts.push(format!("role {}", a.role));
    }
    if !a.note.is_empty() {
        parts.push(a.note.clone());
    }
    parts.join(" \u{00b7} ")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A plain association names its class and role; a self-referencing or
    /// inherited one carries its note; an `ASSOCIATORS OF`-only endpoint has
    /// neither class nor role, so only its note shows.
    #[test]
    fn assoc_subtext_joins_present_parts_only() {
        assert_eq!(
            assoc_subtext(&AssocInfo {
                assoc_class: "Win32_SessionProcess".into(),
                role: "Dependent".into(),
                target_class: "Win32_LogonSession".into(),
                note: String::new(),
            }),
            "via Win32_SessionProcess \u{00b7} role Dependent"
        );
        assert_eq!(
            assoc_subtext(&AssocInfo {
                assoc_class: String::new(),
                role: String::new(),
                target_class: "Win32_Foo".into(),
                note: "reported by ASSOCIATORS OF only".into(),
            }),
            "reported by ASSOCIATORS OF only"
        );
    }
}
