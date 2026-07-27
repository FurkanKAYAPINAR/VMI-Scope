//! The Persistence tab: the WMI event-subscription hunter and its baseline diff.

use eframe::egui;

use crate::app::VmiScopeApp;
use crate::theme::icons;
use crate::theme::tokens::{muted, risk_color, BAD, DIVIDER, WARN};
use crate::util::{save_file, sub_col_value};
use crate::widgets::button::{btn_ghost, btn_primary, btn_secondary};
use crate::widgets::chip::dot_chip;
use crate::widgets::loading::spinner;
use crate::widgets::rule::{hrule, vrule};
use crate::widgets::table::{DataTable, DataTableState, TableColumn};

use vmiscope_core::{diff_subscriptions, Risk};

/// Strength of the muted diff text -- removed rows are context, not a finding:
/// persistence that took itself away is not what anyone is hunting for.
const DIM: u8 = 55;

impl VmiScopeApp {
    // ------------------------------------------------------------------
    // UI: persistence (WMI event-subscription hunter)
    // ------------------------------------------------------------------

    pub(crate) fn load_baseline_dialog(&mut self) {
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("JSON", &["json"])
            .pick_file()
        {
            match std::fs::read_to_string(&path)
                .map_err(|e| e.to_string())
                .and_then(|t| {
                    vmiscope_core::export::subscriptions_from_json(&t).map_err(|e| e.to_string())
                }) {
                Ok(subs) => self.events_baseline = Some(subs),
                Err(e) => self.push_error(format!("Load baseline: {e}")),
            }
        }
    }

    pub(crate) fn ui_persistence(&mut self, ui: &mut egui::Ui) {
        let mut load_baseline = false;
        let mut clear_baseline = false;
        ui.horizontal(|ui| {
            ui.strong("WMI event subscriptions");
            if self.events_loading {
                spinner(ui, "scanning");
            }
            if btn_primary(ui, icons::labelled(ui, icons::ARROWS_CLOCKWISE, "Refresh")).clicked() {
                self.request_events();
            }
            if let Some(report) = self.events_report.as_ref() {
                if !report.subscriptions.is_empty() {
                    if btn_secondary(ui, icons::labelled(ui, icons::DOWNLOAD_SIMPLE, "CSV"))
                        .clicked()
                    {
                        save_file(
                            "wmi_persistence.csv",
                            &vmiscope_core::export::subscriptions_to_csv(report),
                        );
                    }
                    if btn_secondary(ui, icons::labelled(ui, icons::DOWNLOAD_SIMPLE, "JSON"))
                        .clicked()
                    {
                        save_file(
                            "wmi_persistence.json",
                            &vmiscope_core::export::subscriptions_to_json(report),
                        );
                    }
                    if btn_secondary(ui, icons::labelled(ui, icons::DOWNLOAD_SIMPLE, "HTML"))
                        .clicked()
                    {
                        save_file(
                            "wmi_persistence.html",
                            &vmiscope_core::export::subscriptions_to_html(report),
                        );
                    }
                    vrule(ui, DIVIDER);
                    if btn_secondary(ui, icons::labelled(ui, icons::FLOPPY_DISK, "Snapshot"))
                        .on_hover_text("Save a baseline")
                        .clicked()
                    {
                        save_file(
                            "wmi_persistence_snapshot.json",
                            &vmiscope_core::export::subscriptions_to_json(report),
                        );
                    }
                }
            }
            if btn_secondary(ui, icons::labelled(ui, icons::FOLDER_OPEN, "Baseline"))
                .on_hover_text("Load a snapshot to diff against")
                .clicked()
            {
                load_baseline = true;
            }
            if self.events_baseline.is_some()
                && btn_ghost(ui, icons::labelled(ui, icons::X, "clear")).clicked()
            {
                clear_baseline = true;
            }
        });
        if load_baseline {
            self.load_baseline_dialog();
        }
        if clear_baseline {
            self.events_baseline = None;
        }

        if let Some(report) = self.events_report.as_ref() {
            ui.horizontal(|ui| {
                let counts = [
                    (Risk::High, "high"),
                    (Risk::Medium, "medium"),
                    (Risk::Low, "low"),
                ];
                for (risk, label) in counts {
                    dot_chip(
                        ui,
                        risk_color(risk),
                        &format!("{} {label}", report.count(risk)),
                    );
                }
            });
        }

        // Diff against a loaded baseline (snapshot hunting).
        if let (Some(base), Some(report)) =
            (self.events_baseline.as_ref(), self.events_report.as_ref())
        {
            let d = diff_subscriptions(base, &report.subscriptions);
            ui.horizontal(|ui| {
                ui.strong("vs baseline:");
                dot_chip(ui, BAD, &format!("+{} new", d.added.len()));
                dot_chip(ui, WARN, &format!("~{} changed", d.changed.len()));
                dot_chip(ui, muted(DIM), &format!("-{} removed", d.removed.len()));
                ui.weak(format!("\u{00b7} {} unchanged", d.unchanged));
            });
            if !d.is_empty() {
                egui::CollapsingHeader::new("Diff details")
                    .id_salt("persist-diff")
                    .default_open(true)
                    .show(ui, |ui| {
                        let sections = [
                            ("New", &d.added, BAD),
                            ("Changed", &d.changed, WARN),
                            ("Removed (was in baseline)", &d.removed, muted(DIM)),
                        ];
                        for (title, list, color) in sections {
                            if list.is_empty() {
                                continue;
                            }
                            ui.colored_label(color, format!("{title} ({})", list.len()));
                            for s in list {
                                ui.label(format!(
                                    "    {} \u{2192} {} ({})   {}",
                                    s.filter_name, s.consumer_name, s.consumer_type, s.action
                                ));
                            }
                        }
                    });
            }
        }
        hrule(ui);

        // The sort lives on the app so it survives a tab switch; the table gets
        // it on loan for the frame.
        let mut table = DataTableState {
            sort: self.events_sort,
            selected: None,
        };

        if let Some(report) = self.events_report.as_ref() {
            let subs = &report.subscriptions;
            if subs.is_empty() {
                ui.weak("No permanent event subscriptions found.");
            } else {
                DataTable::new("events-table")
                    .columns([
                        TableColumn::initial("Risk", 64.0).at_least(48.0),
                        TableColumn::initial("Consumer type", 168.0).at_least(48.0),
                        TableColumn::initial("Consumer", 150.0).at_least(48.0),
                        TableColumn::initial("Filter", 150.0).at_least(48.0),
                        TableColumn::initial("Action / query", 260.0).at_least(48.0),
                        TableColumn::initial("Why", 220.0).at_least(48.0),
                    ])
                    .sort_key(|row, col| sub_col_value(&subs[row], col))
                    .show(ui, &mut table, subs.len(), |row| {
                        let s = &subs[row.data_index()];
                        let color = risk_color(s.risk);

                        row.colored(s.risk.as_str(), color);
                        row.text(s.consumer_type.as_str());
                        row.text(s.consumer_name.as_str());
                        row.text(s.filter_name.as_str());

                        let action = if s.action.is_empty() {
                            s.filter_query.as_str()
                        } else {
                            s.action.as_str()
                        };
                        // The tooltip carries both halves, because the cell only
                        // ever shows whichever one is populated.
                        row.text(action).on_hover_text(format!(
                            "filter query:\n{}\n\naction:\n{}",
                            s.filter_query, s.action
                        ));

                        let why = s.reasons.join("; ");
                        row.colored(why.as_str(), color).on_hover_text(why);
                    });
            }
        } else if !self.events_loading {
            ui.weak("Click Refresh to scan for WMI persistence.");
        }

        self.events_sort = table.sort;
    }
}
