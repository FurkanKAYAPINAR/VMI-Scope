//! The detail pane's head: breadcrumb, class header, tags, meta line, actions.

use eframe::egui;
use eframe::egui::{RichText, TextStyle};

use crate::app::{ConnStatus, VmiScopeApp};
use crate::theme::icons;
use crate::theme::tokens::muted;
use crate::util::save_file;
use crate::views::nav::View;
use crate::widgets::button::{btn_icon, btn_secondary};
use crate::widgets::chip::{tag_accent, tag_neutral};
use crate::widgets::loading::spinner;

use vmiscope_core::export::{query_to_csv, query_to_json};

impl VmiScopeApp {
    // ------------------------------------------------------------------
    // UI: detail head (breadcrumb + header + tags + meta + actions)
    // ------------------------------------------------------------------

    pub(crate) fn ui_detail(&mut self, ui: &mut egui::Ui) {
        let host = self.breadcrumb_host();
        let ns = self.active_ns.clone();
        let class = self.selected_class.clone();

        // Breadcrumb: \\host > namespace [> class] with a copy button that puts
        // the full object path on the clipboard.
        ui.horizontal(|ui| {
            crumb(ui, &format!("\\\\{host}"), muted(50));
            caret(ui);
            crumb(ui, &ns, muted(70));
            if let Some(class) = &class {
                caret(ui);
                crumb(ui, class, muted(90));
                let path = format!("\\\\{host}\\{ns}:{class}");
                if btn_icon(ui, icons::COPY)
                    .on_hover_text("Copy object path")
                    .clicked()
                {
                    ui.ctx().copy_text(path);
                }
            }
        });

        let Some(class) = class else {
            return;
        };

        // H4 class name.
        ui.add(
            egui::Label::new(RichText::new(&class).text_style(TextStyle::Heading))
                .selectable(false),
        );

        // Tags: the class's kinds, then its provider. Read off the `ClassBrief`
        // the list already holds, so they show before the schema arrives.
        if let Some(brief) = self.classes.iter().find(|c| c.name == class).cloned() {
            ui.horizontal_wrapped(|ui| {
                for label in brief.kind.labels() {
                    tag_accent(ui, label);
                }
                if let Some(provider) = &brief.provider {
                    tag_neutral(ui, &format!("provider: {provider}"));
                }
            });
        }

        // Meta line: property/method counts and the immediate parent, from the
        // reflected schema. Says "reflecting" while it loads and "unavailable"
        // if it never comes -- never a blank line (task 3.33).
        match self.schema_for_selected() {
            Some(schema) => {
                let parent = schema.derivation.first().map_or("(root)", String::as_str);
                ui.label(
                    RichText::new(format!(
                        "{} properties \u{00b7} {} methods \u{00b7} derives from {parent}",
                        schema.properties.len(),
                        schema.methods.len(),
                    ))
                    .color(muted(55)),
                );
            }
            None if self.schema_loading && self.schema_class == class => {
                spinner(ui, "reflecting schema\u{2026}");
            }
            None => {
                ui.label(RichText::new("schema unavailable").color(muted(45)));
            }
        }

        self.ui_detail_actions(ui, &class);
    }

    /// The action row: Query · Watch · Invoke · Export. Collected into locals so
    /// the menu closure never has to borrow `self` alongside the buttons.
    fn ui_detail_actions(&mut self, ui: &mut egui::Ui, class: &str) {
        let mut do_query = false;
        let mut do_watch = false;
        let mut do_invoke = false;
        let mut export_csv = false;
        let mut export_json = false;
        let has_result = self.result.as_ref().is_some_and(|r| !r.rows.is_empty());

        ui.horizontal(|ui| {
            if btn_secondary(ui, icons::labelled(ui, icons::TERMINAL_WINDOW, "Query"))
                .on_hover_text("Open this class in the Query view")
                .clicked()
            {
                do_query = true;
            }
            if btn_secondary(ui, icons::labelled(ui, icons::BROADCAST, "Watch"))
                .on_hover_text("Watch instance-creation events for this class")
                .clicked()
            {
                do_watch = true;
            }
            if btn_secondary(ui, icons::labelled(ui, icons::GEAR_SIX, "Invoke"))
                .on_hover_text("Invoke a method (may change system state)")
                .clicked()
            {
                do_invoke = true;
            }
            ui.menu_button(icons::labelled(ui, icons::EXPORT, "Export"), |ui| {
                if has_result {
                    if ui
                        .button(icons::labelled(ui, icons::FILE_CSV, "Instances as CSV"))
                        .clicked()
                    {
                        export_csv = true;
                        ui.close();
                    }
                    if ui
                        .button(icons::labelled(
                            ui,
                            icons::BRACKETS_CURLY,
                            "Instances as JSON",
                        ))
                        .clicked()
                    {
                        export_json = true;
                        ui.close();
                    }
                } else {
                    ui.label(RichText::new("Load instances first").color(muted(40)));
                }
            });
        });

        if do_query {
            // Hand the class to the Query view with a prefilled WQL (task 3.22).
            self.query_text = format!("SELECT * FROM {class}");
            self.view = View::Query;
        }
        if do_watch {
            // Prime the Events view with an intrinsic creation subscription. The
            // Events view starts the monitor; this only sets its query.
            self.monitor_wql = format!(
                "SELECT * FROM __InstanceCreationEvent WITHIN 2 \
                 WHERE TargetInstance ISA '{class}'"
            );
            self.view = View::Events;
        }
        if do_invoke {
            self.actions_open = true;
            self.act_method = None;
            self.act_outcome = None;
            self.act_instances = None;
            self.request_schema(class.to_string());
        }
        if let Some(result) = self.result.as_ref() {
            if export_csv {
                save_file("instances.csv", &query_to_csv(result));
            }
            if export_json {
                save_file("instances.json", &query_to_json(result));
            }
        }
    }

    /// The host segment of the breadcrumb and of a copied object path: the
    /// remote host when connected to one, `.` for the local machine (which is
    /// the host part of a local WMI object path).
    fn breadcrumb_host(&self) -> String {
        match &self.conn_status {
            ConnStatus::Remote(h) => h.clone(),
            _ => ".".to_string(),
        }
    }
}

/// One breadcrumb segment.
fn crumb(ui: &mut egui::Ui, text: &str, color: egui::Color32) {
    ui.add(
        egui::Label::new(
            RichText::new(text)
                .text_style(TextStyle::Name("caption".into()))
                .color(color),
        )
        .selectable(false),
    );
}

/// The `>` between breadcrumb segments, in the icon font.
fn caret(ui: &mut egui::Ui) {
    ui.add(
        egui::Label::new(icons::glyph(icons::CARET_RIGHT).size(10.0).color(muted(28)))
            .selectable(false),
    );
}
