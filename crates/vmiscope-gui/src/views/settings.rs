//! The Settings view: four groups of preferences under accent-underlined
//! headings, each row a key over a muted note with the control on the right.
//!
//! The governing rule here is that no control is decorative. A setting is either
//! wired to real behaviour, or it is rendered disabled with a tooltip that says
//! what it will control once its plumbing lands. Shipping an enabled control
//! that silently does nothing is worse than not offering it.
//!
//! Wired today: **accent** and **density** (re-install the style live and
//! persist), **live polling** (gates the live-view auto-refresh), and the code
//! generator's **default language** (seeds the generator and persists). Every
//! other control is disabled, because the point where it would take effect lives
//! in a module this view does not own yet (`state::requests` for the row cap and
//! timeout, the Explorer for the class filter, `vmiscope-core` for the
//! impersonation blanket) or has no consumer at all (byte formatting). Each of
//! those carries a tooltip naming the task that wires it.

use eframe::egui::{self, FontFamily, RichText, TextStyle, Ui};

use crate::app::{ScriptLang, VmiScopeApp};
use crate::config::CodeLang;
use crate::theme::icons;
use crate::theme::tokens::{muted, S2, S4, S6, TEXT};
use crate::theme::{Accent, Density, Theme};
use crate::widgets::button::{accent, segmented};
use crate::widgets::field::labelled_row;
use crate::widgets::rule::hrule_colored;

/// Settings read best in a single narrow column rather than stretched across a
/// wide window, so the whole view is capped to this and left-aligned.
const CONTENT_W: f32 = 720.0;

/// The h6 group-heading size — a step under `Body` (13), medium weight, so a
/// group reads as a heading without shouting.
const HEADING_SIZE: f32 = 12.5;

impl VmiScopeApp {
    pub(crate) fn ui_settings(&mut self, ui: &mut egui::Ui) {
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                // Cap the column here rather than in a centering closure: it
                // keeps the whole body a single flat borrow of `self`, which the
                // per-row control closures below all need.
                ui.set_max_width(CONTENT_W.min(ui.available_width()));
                self.settings_body(ui);
            });
    }

    fn settings_body(&mut self, ui: &mut Ui) {
        ui.add_space(S4);
        ui.label(icons::labelled_styled(
            ui,
            icons::GEAR_SIX,
            "Settings",
            TextStyle::Heading,
            TEXT,
        ));
        ui.label(
            RichText::new("Preferences for this machine, saved to config.json.")
                .text_style(TextStyle::Name("caption".into()))
                .color(muted(50)),
        );

        // ---- Connection ----
        group_heading(ui, "Connection");

        labelled_row(
            ui,
            "Default namespace",
            "The namespace the Explorer opens to on the next connection.",
            |ui| {
                disabled_mono(
                    ui,
                    &self.config.default_namespace,
                    "Read at startup and on a new connection. Wiring the reset path \
                     lands with task 7.1 (state::requests).",
                );
            },
        );

        labelled_row(
            ui,
            "Impersonation level",
            "DCOM impersonation for alternate-credential connections.",
            |ui| {
                disabled_text(
                    ui,
                    self.config.impersonation.label(),
                    "Honoured only on the alternate-credentials path, and only once core \
                     task 5.10 parameterises CoSetProxyBlanket. The SSO path goes through \
                     the wmi crate, which cannot set it.",
                );
            },
        );

        labelled_row(
            ui,
            "Authentication",
            "How a remote host is authenticated.",
            |ui| {
                disabled_text(
                    ui,
                    "Current user (Kerberos)",
                    "Chosen per connection on the Machines view; a persisted default \
                     lands with task 7.1.",
                );
            },
        );

        labelled_row(
            ui,
            "Operation timeout",
            "How long one enumeration may run before it gives up.",
            |ui| {
                disabled_mono(
                    ui,
                    &format!("{} s", self.config.operation_timeout_secs),
                    "Feeds Request::Query. Currently a constant in state::requests; \
                     wiring lands with task 7.1.",
                );
            },
        );

        // ---- Results ----
        group_heading(ui, "Results");

        labelled_row(
            ui,
            "Row limit",
            "Rows a query returns before it stops and reports truncation.",
            |ui| {
                disabled_mono(
                    ui,
                    &format!("{} rows", self.config.row_limit),
                    "Feeds Request::Query.max_rows. Currently a constant in \
                     state::requests; wiring lands with task 7.2.",
                );
            },
        );

        labelled_row(
            ui,
            "Live polling",
            "Auto-refresh the live views (Network, Events).",
            |ui| {
                // WIRED: the Network view's auto-refresh gate in `app::ui` reads
                // this. Off means the live views only update on a manual refresh.
                let mut live = self.config.live_polling;
                if segmented(ui, &mut live, &[(true, "On"), (false, "Off")]) {
                    self.config.live_polling = live;
                    self.config.save();
                }
            },
        );

        labelled_row(
            ui,
            "Byte formatting",
            "How sizes are shown where the UI prints one.",
            |ui| {
                disabled_text(
                    ui,
                    self.config.byte_format.label(),
                    "No size display consumes this yet; it lands with the status-bar \
                     provider host stats (task 5.14).",
                );
            },
        );

        labelled_row(
            ui,
            "Show system classes",
            "List WMI system classes (names beginning '__').",
            |ui| {
                disabled_text(
                    ui,
                    on_off(self.config.show_system_classes),
                    "Filters the Explorer class list; wiring lands with the class-list \
                     filter (views::explorer).",
                );
            },
        );

        // ---- Code generation ----
        group_heading(ui, "Code generation");

        labelled_row(
            ui,
            "Default language",
            "The language the script generator starts in.",
            |ui| {
                // WIRED: writes the persisted default and updates the live
                // generator selection in the same frame.
                let mut lang = self.config.default_lang;
                if segmented(
                    ui,
                    &mut lang,
                    &[
                        (CodeLang::PowerShell, "PowerShell"),
                        (CodeLang::VbScript, "VBScript"),
                    ],
                ) {
                    self.config.default_lang = lang;
                    self.script_lang = script_lang_of(lang);
                    self.config.save();
                }
            },
        );

        labelled_row(
            ui,
            "Include credentials block",
            "Emit an alternate-credentials header in generated scripts.",
            |ui| {
                disabled_text(
                    ui,
                    "Off",
                    "The generator does not emit a credentials header yet (task 7.3).",
                );
            },
        );

        labelled_row(
            ui,
            "Line width",
            "Wrap generated scripts at this column.",
            |ui| {
                disabled_mono(
                    ui,
                    &self.config.line_width.to_string(),
                    "The generator does not wrap its output yet (task 7.3).",
                );
            },
        );

        // ---- Interface ----
        group_heading(ui, "Interface");

        labelled_row(ui, "Density", "Row height and spacing throughout.", |ui| {
            // WIRED: re-installs the style with the new metrics this frame,
            // then persists. Survives a restart because `app::new` installs
            // the theme from config at boot.
            let mut density = self.config.density;
            if segmented(ui, &mut density, &Density::ALL.map(|d| (d, d.label()))) {
                self.config.density = density;
                crate::theme::install(ui.ctx(), self.theme());
                self.config.save();
            }
        });

        labelled_row(ui, "Accent", "The interface accent colour.", |ui| {
            // WIRED: same one-frame re-install as density.
            let mut chosen = self.config.accent;
            if segmented(ui, &mut chosen, &Accent::ALL.map(|a| (a, a.label()))) {
                self.config.accent = chosen;
                crate::theme::install(ui.ctx(), self.theme());
                self.config.save();
            }
        });

        labelled_row(
            ui,
            "Monospace font",
            "Face used for paths, WQL and data.",
            |ui| {
                disabled_text(
                    ui,
                    "JetBrains Mono",
                    "Only JetBrains Mono is bundled; alternate faces need theme::fonts.",
                );
            },
        );

        ui.add_space(S6);
    }

    /// The live theme, rebuilt from the persisted preferences. Used by the two
    /// controls that re-install the style so both read from one place.
    fn theme(&self) -> Theme {
        Theme {
            accent: self.config.accent,
            density: self.config.density,
        }
    }
}

/// An accent-underlined h6 heading: medium-weight label over the design's faded
/// accent rule (the sanctioned "section mark under an accent heading").
fn group_heading(ui: &mut Ui, title: &str) {
    ui.add_space(S6);
    ui.label(
        RichText::new(title)
            .family(FontFamily::Name(crate::theme::fonts::UI_MEDIUM.into()))
            .size(HEADING_SIZE)
            .color(TEXT),
    );
    ui.add_space(S2);
    hrule_colored(ui, accent(ui));
}

/// A greyed, inert control showing the value it *will* carry, with a tooltip
/// naming what it controls. The honest rendering for a setting whose plumbing is
/// not in this view's reach yet.
fn disabled_text(ui: &mut Ui, value: &str, tip: &str) {
    ui.add_enabled(false, egui::Button::new(RichText::new(value)))
        .on_disabled_hover_text(tip);
}

/// [`disabled_text`] for a data-shaped value (a path, a count, a number), shown
/// in the monospace face the rest of the tool uses for such values.
fn disabled_mono(ui: &mut Ui, value: &str, tip: &str) {
    ui.add_enabled(false, egui::Button::new(RichText::new(value).monospace()))
        .on_disabled_hover_text(tip);
}

/// A boolean rendered as its eventual on/off word rather than a dead checkbox.
fn on_off(v: bool) -> &'static str {
    if v {
        "On"
    } else {
        "Off"
    }
}

/// Map the persisted code language onto the app's live generator enum. Kept next
/// to the view that needs both so neither `config` nor `app` has to know the
/// other's enum.
fn script_lang_of(lang: CodeLang) -> ScriptLang {
    match lang {
        CodeLang::PowerShell => ScriptLang::PowerShell,
        CodeLang::VbScript => ScriptLang::VbScript,
    }
}
