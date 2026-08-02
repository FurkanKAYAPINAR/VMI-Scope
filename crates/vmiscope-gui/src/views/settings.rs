//! The Settings view: preferences under accent-underlined headings, each row a
//! key over a muted note with the control on the right — and, at the bottom,
//! the About panel that discharges the font licences.
//!
//! # No control is decorative
//!
//! A setting is either wired to real behaviour, or it is rendered **disabled**
//! with a tooltip saying what would have to change for it to work. Shipping an
//! enabled control that silently does nothing is worse than not offering it.
//!
//! Tasks 7.1-7.3 closed that gap. Everything here is wired except two, and both
//! are disabled for a reason that names the code that would have to move:
//!
//! * **Authentication** — the authentication package is fixed per transport in
//!   the core, not chosen: alternate credentials are pinned to
//!   `RPC_C_AUTHN_WINNT` at `remote.rs:131`, and the SSO path goes through the
//!   `wmi` crate, which negotiates and exposes no hook. There is no second value
//!   to offer without a core change. The control still *reports* which package
//!   the live connection is using, so a disabled row is at least informative.
//! * **Monospace font** — exactly one monospace face is embedded, and nothing
//!   is loaded from disk at runtime by design (`theme::fonts`).
//!
//! One row changed meaning rather than being wired as planned. Task 7.3 asked
//! for **line width** to wrap generated scripts; it cannot, because all three
//! script languages carry the query inside a string literal and a newline in
//! one of those changes the query rather than the layout. It drives the Code
//! sub-tab's column guide instead, and the row says that.

use eframe::egui::{self, FontFamily, RichText, TextStyle, Ui};

use crate::app::{ConnStatus, ScriptLang, VmiScopeApp};
use crate::config::{ByteFormat, CodeLang, Impersonation};
use crate::theme::icons;
use crate::theme::tokens::{muted, S2, S3, S4, S6, TEXT};
use crate::theme::{Accent, Density, Theme};
use crate::widgets::button::{accent, btn_secondary, segmented};
use crate::widgets::codeview::{code_panel, Lang};
use crate::widgets::field::{combo, labelled_row, mono_input};
use crate::widgets::rule::hrule_colored;

/// Settings read best in a single narrow column rather than stretched across a
/// wide window, so the whole view is capped to this and left-aligned.
const CONTENT_W: f32 = 720.0;

/// The h6 group-heading size — a step under `Body` (13), medium weight, so a
/// group reads as a heading without shouting.
const HEADING_SIZE: f32 = 12.5;

/// Width of a text field in the value column. A namespace path is the longest
/// thing typed here; wider and the field starts competing with the key.
const FIELD_W: f32 = 260.0;

/// Row-cap presets.
///
/// A list rather than a free number: WQL has no `TOP`, so the cap is the only
/// thing between `SELECT * FROM CIM_DataFile` and every file on the machine,
/// and a text box invites a typo'd extra zero on the one setting where that
/// matters. There is no "unlimited" for the same reason.
const ROW_LIMITS: [(usize, &str); 5] = [
    (500, "500 rows"),
    (1_000, "1,000 rows"),
    (5_000, "5,000 rows"),
    (20_000, "20,000 rows"),
    (100_000, "100,000 rows"),
];

/// Operation-timeout presets, in seconds.
///
/// Needed on top of the row cap because a cap only bites once rows arrive, and
/// some providers deliver none for a long time — `CIM_DataFile` returned
/// nothing at all in twelve seconds when measured (`docs/FINDINGS.md`).
const TIMEOUTS: [(u64, &str); 5] = [
    (10, "10 s"),
    (30, "30 s"),
    (60, "1 min"),
    (120, "2 min"),
    (300, "5 min"),
];

/// Column-guide presets for the Code sub-tab.
const GUIDES: [(u32, &str); 4] = [
    (80, "80 columns"),
    (100, "100 columns"),
    (120, "120 columns"),
    (160, "160 columns"),
];

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

        self.settings_connection(ui);
        self.settings_results(ui);
        self.settings_codegen(ui);
        self.settings_interface(ui);
        self.settings_about(ui);

        ui.add_space(S6);
    }

    // ------------------------------------------------------------------
    // Connection
    // ------------------------------------------------------------------

    fn settings_connection(&mut self, ui: &mut Ui) {
        group_heading(ui, "Connection");

        labelled_row(
            ui,
            "Default namespace",
            "The namespace the Explorer opens to, and what a new connection is offered.",
            |ui| {
                // WIRED: read by `app::new` at boot and by the Machines view's
                // New-connection form. Persisted on every edit through the
                // debounce rather than on focus loss -- there is no Apply button
                // in this view, and a setting that needs one is a setting that
                // gets lost; `save_debounced` is what keeps a keystroke from
                // being a file write.
                ui.scope(|ui| {
                    ui.spacing_mut().text_edit_width = FIELD_W;
                    if mono_input(ui, &mut self.config.default_namespace, "root\\CIMV2").changed() {
                        self.config.save_debounced();
                    }
                });
            },
        );

        labelled_row(
            ui,
            "Impersonation level",
            "How much of your identity a remote WMI provider may use.",
            |ui| {
                // WIRED: goes into `Request::SetHost` and reaches the proxy
                // blanket on BOTH transports -- the core sets its own blanket on
                // each (`worker.rs`), so this is not the alt-cred-only setting
                // the previous tooltip here claimed it was.
                let mut level = self.config.impersonation;
                if segmented(
                    ui,
                    &mut level,
                    &[
                        (Impersonation::Identify, Impersonation::Identify.label()),
                        (
                            Impersonation::Impersonate,
                            Impersonation::Impersonate.label(),
                        ),
                        (Impersonation::Delegate, Impersonation::Delegate.label()),
                    ],
                ) {
                    self.config.impersonation = level;
                    self.config.save();
                }
            },
        );
        note(
            ui,
            match self.config.impersonation {
                Impersonation::Identify => {
                    "Identify lets a provider check your ACLs but not act as you. Most of WMI \
                     refuses it."
                }
                Impersonation::Impersonate => {
                    "Impersonate lets a provider act as you on its own machine. WMI's usual \
                     level."
                }
                Impersonation::Delegate => {
                    "Delegate lets a provider act as you on OTHER machines too. Needed for a \
                     double hop, and dangerous for exactly that reason."
                }
            },
        );

        labelled_row(
            ui,
            "Authentication",
            "The package the connection authenticates with.",
            |ui| {
                // DISABLED, and it reports rather than pretends: the value shown
                // is what the live connection is actually using.
                disabled_text(
                    ui,
                    match (&self.conn_status, self.conn_use_creds) {
                        (ConnStatus::Local, _) => "Current user (local)",
                        (_, true) => "NTLM (alternate credentials)",
                        (_, false) => "Negotiate (current user)",
                    },
                    "Not a choice this build offers. The alternate-credential path is pinned to \
                     RPC_C_AUTHN_WINNT where it sets its proxy blanket, and the current-user \
                     path goes through the wmi crate, which negotiates and exposes no hook. \
                     Offering a package here would need vmiscope-core to take one.",
                );
            },
        );

        labelled_row(
            ui,
            "Operation timeout",
            "How long one enumeration may run before it gives up and reports a partial result.",
            |ui| {
                // WIRED: `state::requests::run_query` puts this on
                // `Request::Query.timeout`.
                let mut secs = self.config.operation_timeout_secs;
                if combo(ui, "set-timeout", &mut secs, &TIMEOUTS) {
                    self.config.operation_timeout_secs = secs;
                    self.config.save();
                }
            },
        );
    }

    // ------------------------------------------------------------------
    // Results
    // ------------------------------------------------------------------

    fn settings_results(&mut self, ui: &mut Ui) {
        group_heading(ui, "Results");

        labelled_row(
            ui,
            "Row limit",
            "Rows a query returns before it stops and says it truncated.",
            |ui| {
                // WIRED: `Request::Query.max_rows`. The result header reports
                // the truncation, so a lowered cap is visible immediately on
                // the next run rather than silently shortening the table.
                let mut limit = self.config.row_limit;
                if combo(ui, "set-rows", &mut limit, &ROW_LIMITS) {
                    self.config.row_limit = limit;
                    self.config.save();
                }
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
                // WIRED: the Providers view's quota meters render every figure
                // through `fmt_bytes`, which takes this.
                let mut format = self.config.byte_format;
                if segmented(
                    ui,
                    &mut format,
                    &[
                        (ByteFormat::Binary, ByteFormat::Binary.label()),
                        (ByteFormat::Decimal, ByteFormat::Decimal.label()),
                    ],
                ) {
                    self.config.byte_format = format;
                    self.config.save();
                }
            },
        );

        labelled_row(
            ui,
            "Show system classes",
            "List WMI's own classes (names beginning '__') in the Explorer.",
            |ui| {
                // WIRED: the Explorer class list filters on it. Hiding is a list
                // filter and never a data one -- the System chip still reaches
                // every hidden class, and the list says how many it is holding
                // back.
                let mut show = self.config.show_system_classes;
                if segmented(ui, &mut show, &[(true, "Show"), (false, "Hide")]) {
                    self.config.show_system_classes = show;
                    self.config.save();
                }
            },
        );
    }

    // ------------------------------------------------------------------
    // Code generation
    // ------------------------------------------------------------------

    fn settings_codegen(&mut self, ui: &mut Ui) {
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
            "Generate scripts that authenticate as somebody else.",
            |ui| {
                // WIRED: `util::generate_script` and the Code sub-tab's C# arm
                // rewrite the line that *binds* WMI, not a comment above it --
                // a header declaring a credential the call below ignores would
                // read as working.
                let mut on = self.config.include_credentials;
                if segmented(ui, &mut on, &[(true, "On"), (false, "Off")]) {
                    self.config.include_credentials = on;
                    self.config.save();
                }
            },
        );
        note(
            ui,
            "PowerShell gains a New-CimSession, VBScript swaps the winmgmts: moniker for \
             SWbemLocator.ConnectServer, and C# gains ConnectionOptions. No password is ever \
             written into a generated script. WQL is unaffected: a query has no connection.",
        );

        labelled_row(
            ui,
            "Code column guide",
            "Mark this column in the Code sub-tab.",
            |ui| {
                // WIRED: `widgets::codeview::code_panel` paints a hairline there.
                let mut width = self.config.line_width;
                if combo(ui, "set-guide", &mut width, &GUIDES) {
                    self.config.line_width = width;
                    self.config.save();
                }
            },
        );
        note(
            ui,
            "This was planned as a wrap column and is a guide instead, because there is no \
             column at which a generated script can be wrapped: all three script languages \
             carry the query inside a string literal, so a newline added to fit a width would \
             change the query rather than the layout.",
        );
    }

    // ------------------------------------------------------------------
    // Interface
    // ------------------------------------------------------------------

    fn settings_interface(&mut self, ui: &mut Ui) {
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
            "Window chrome",
            "The custom title bar, or the operating system's.",
            |ui| {
                // WIRED: applied to the live viewport AND persisted. `decorated`
                // has been in the config since v2 (task 2.24) with nothing
                // reading it back -- the CLI flag was the only way to set it,
                // and the flag is not persistent.
                let mut decorated = self.config.decorated;
                if segmented(ui, &mut decorated, &[(false, "Custom"), (true, "System")]) {
                    self.config.decorated = decorated;
                    self.decorated = decorated;
                    ui.ctx()
                        .send_viewport_cmd(egui::ViewportCommand::Decorations(decorated));
                    self.config.save();
                }
            },
        );
        note(
            ui,
            "The custom bar costs Windows 11 Snap Layouts and screen-reader discovery of the \
             caption buttons: winit does not answer WM_NCHITTEST with HTMAXBUTTON, so there is \
             nothing for Snap to attach to. System chrome hands both back. --decorated forces \
             System at startup whatever is saved here.",
        );

        labelled_row(
            ui,
            "Monospace font",
            "Face used for paths, WQL and data.",
            |ui| {
                disabled_text(
                    ui,
                    "JetBrains Mono NL",
                    "One monospace face is embedded, and nothing is loaded from disk or the \
                     network at runtime -- a tool that inspects other machines has no business \
                     fetching a font. A second face would have to be bundled in theme::fonts.",
                );
            },
        );
    }

    // ------------------------------------------------------------------
    // About and licences (task 7.4)
    // ------------------------------------------------------------------

    fn settings_about(&mut self, ui: &mut Ui) {
        group_heading(ui, "About");

        labelled_row(ui, "VMI-Scope", env!("CARGO_PKG_DESCRIPTION"), |ui| {
            disabled_text(
                ui,
                concat!("v", env!("CARGO_PKG_VERSION")),
                "The version this binary was built from.",
            );
        });

        labelled_row(
            ui,
            "Licences",
            "VMI-Scope is MIT. The embedded fonts are not.",
            |ui| {
                let label = if self.licences_open { "Hide" } else { "Show" };
                if btn_secondary(ui, icons::labelled(ui, icons::FILE_TEXT, label)).clicked() {
                    self.licences_open = !self.licences_open;
                }
            },
        );

        if !self.licences_open {
            return;
        }

        note(
            ui,
            "Three font files are compiled into this binary. Two are under the SIL Open Font \
             License 1.1, whose clause 2 requires each copy to be distributed WITH its licence \
             text; one is MIT, which requires the same. This panel is how that text travels \
             with the software. All three are embedded byte-for-byte as released -- under the \
             OFL FAQ, subsetting counts as modification and would oblige a rename.",
        );
        ui.add_space(S2);

        for licence in LICENCES {
            ui.add_space(S3);
            ui.label(icons::labelled_styled(
                ui,
                icons::SEAL_CHECK,
                licence.title,
                TextStyle::Body,
                TEXT,
            ));
            ui.label(
                RichText::new(licence.subtitle)
                    .text_style(TextStyle::Name("caption".into()))
                    .color(muted(45)),
            );
            ui.add_space(S2);
            // Through the code panel: it is a monospace document with a fixed
            // width, which is exactly what these files are, and it scrolls
            // rather than making the whole view tall.
            code_panel(ui, licence.text, Lang::Plain, None);
        }
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

// ---------------------------------------------------------------------------
// Licence texts
// ---------------------------------------------------------------------------

/// One bundled licence.
struct Licence {
    title: &'static str,
    subtitle: &'static str,
    text: &'static str,
}

/// The three licence files, embedded from the same directory as the fonts they
/// cover.
///
/// `include_str!` rather than a runtime read, and that is the whole point: the
/// obligation is to distribute the licence *with* the software, and a panel
/// that read a file from beside the executable would show nothing at all once
/// the binary was copied somewhere on its own. It travels or it does not
/// discharge anything.
///
/// The paths are relative to this file, so a moved or renamed licence file is a
/// compile error rather than a missing notice.
const LICENCES: &[Licence] = &[
    Licence {
        title: "Inter",
        subtitle: "v4.1 \u{00b7} (c) 2016 The Inter Project Authors \u{00b7} SIL Open Font \
                   License 1.1 \u{00b7} embedded unmodified",
        text: include_str!("../../assets/fonts/LICENSE-Inter-OFL.txt"),
    },
    Licence {
        title: "JetBrains Mono NL",
        subtitle: "v2.304 \u{00b7} (c) 2020 The JetBrains Mono Project Authors \u{00b7} SIL \
                   Open Font License 1.1 \u{00b7} embedded unmodified",
        text: include_str!("../../assets/fonts/LICENSE-JetBrainsMono-OFL.txt"),
    },
    Licence {
        title: "Phosphor Icons",
        subtitle: "v2.1.2 regular \u{00b7} (c) 2020-2021 Phosphor Icons \u{00b7} MIT \
                   \u{00b7} embedded unmodified",
        text: include_str!("../../assets/fonts/LICENSE-Phosphor-MIT.txt"),
    },
];

// ---------------------------------------------------------------------------
// Row furniture
// ---------------------------------------------------------------------------

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

/// A full-width line under a row, for a consequence too long to be a note in
/// the key column and too important to be only a tooltip.
fn note(ui: &mut Ui, text: &str) {
    ui.add_space(S2);
    ui.label(
        RichText::new(text)
            .text_style(TextStyle::Name("caption".into()))
            .color(muted(40)),
    );
    ui.add_space(S2);
}

/// A greyed, inert control showing the value it carries, with a tooltip saying
/// why it is inert. The honest rendering for a setting this build cannot offer
/// a choice for.
fn disabled_text(ui: &mut Ui, value: &str, tip: &str) {
    ui.add_enabled(false, egui::Button::new(RichText::new(value)))
        .on_disabled_hover_text(tip);
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Task 7.4's whole obligation. The OFL's clause 2 and the MIT licence both
    /// require the text to be distributed with the software; a panel that
    /// rendered an empty string would satisfy nobody, and an empty
    /// `include_str!` is exactly what a truncated or emptied asset produces.
    #[test]
    fn every_bundled_licence_text_is_actually_present() {
        assert_eq!(LICENCES.len(), 3, "one per embedded font file");
        for licence in LICENCES {
            assert!(
                licence.text.len() > 400,
                "{} has no licence text ({} bytes)",
                licence.title,
                licence.text.len()
            );
            assert!(!licence.subtitle.is_empty());
        }
    }

    /// The two OFL faces must carry the OFL, and the icon font the MIT text.
    /// Swapping the two files would still compile and would still render three
    /// panels of plausible-looking legalese.
    #[test]
    fn each_licence_is_the_one_its_font_is_under() {
        let by_title = |t: &str| {
            LICENCES
                .iter()
                .find(|l| l.title == t)
                .unwrap_or_else(|| panic!("{t} is not listed"))
        };
        for title in ["Inter", "JetBrains Mono NL"] {
            let licence = by_title(title);
            assert!(
                licence.text.contains("SIL OPEN FONT LICENSE"),
                "{title} is not carrying the OFL"
            );
            assert!(licence.subtitle.contains("SIL Open Font License"));
        }
        let phosphor = by_title("Phosphor Icons");
        assert!(
            phosphor.text.contains("MIT License") || phosphor.text.contains("MIT license"),
            "Phosphor is not carrying the MIT text"
        );
    }

    /// The preset lists have to contain the value a fresh config starts at, or
    /// the combo opens showing an empty selection on a brand-new install.
    #[test]
    fn every_preset_list_contains_its_default() {
        let cfg = crate::config::Config::default();
        assert!(
            ROW_LIMITS.iter().any(|(v, _)| *v == cfg.row_limit),
            "the default row limit {} is not offered",
            cfg.row_limit
        );
        assert!(
            TIMEOUTS
                .iter()
                .any(|(v, _)| *v == cfg.operation_timeout_secs),
            "the default timeout {} s is not offered",
            cfg.operation_timeout_secs
        );
        assert!(
            GUIDES.iter().any(|(v, _)| *v == cfg.line_width),
            "the default guide column {} is not offered",
            cfg.line_width
        );
    }

    /// Presets have to be ordered and distinct: a combo that lists 5,000 twice,
    /// or 500 after 20,000, reads as a bug in the list rather than a choice.
    #[test]
    fn presets_ascend_without_repeating() {
        assert!(ROW_LIMITS.windows(2).all(|w| w[0].0 < w[1].0));
        assert!(TIMEOUTS.windows(2).all(|w| w[0].0 < w[1].0));
        assert!(GUIDES.windows(2).all(|w| w[0].0 < w[1].0));
    }
}
