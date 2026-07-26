//! The Persistence tab: the WMI event-subscription hunter and its baseline diff.

use eframe::egui;
use egui::Color32;
use egui_extras::{Column, TableBuilder};

use crate::app::VmiScopeApp;
use crate::theme::tokens::risk_color;
use crate::util::{save_file, smart_cmp, sub_col_value, toggle_sort};
use crate::widgets::table::sortable_header;

use vmiscope_core::{diff_subscriptions, Risk};

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
                ui.spinner();
            }
            if ui.button("\u{21bb} Refresh").clicked() {
                self.request_events();
            }
            if let Some(report) = self.events_report.as_ref() {
                if !report.subscriptions.is_empty() {
                    if ui.button("\u{2b73} CSV").clicked() {
                        save_file(
                            "wmi_persistence.csv",
                            &vmiscope_core::export::subscriptions_to_csv(report),
                        );
                    }
                    if ui.button("\u{2b73} JSON").clicked() {
                        save_file(
                            "wmi_persistence.json",
                            &vmiscope_core::export::subscriptions_to_json(report),
                        );
                    }
                    if ui.button("\u{2b73} HTML").clicked() {
                        save_file(
                            "wmi_persistence.html",
                            &vmiscope_core::export::subscriptions_to_html(report),
                        );
                    }
                    ui.separator();
                    if ui
                        .button("\u{1f4be} Snapshot")
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
            if ui
                .button("\u{1f4c2} Baseline")
                .on_hover_text("Load a snapshot to diff against")
                .clicked()
            {
                load_baseline = true;
            }
            if self.events_baseline.is_some() && ui.button("\u{2716} clear").clicked() {
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
                ui.colored_label(
                    risk_color(Risk::High),
                    format!("\u{25cf} {} high", report.count(Risk::High)),
                );
                ui.colored_label(
                    risk_color(Risk::Medium),
                    format!("\u{25cf} {} medium", report.count(Risk::Medium)),
                );
                ui.colored_label(
                    risk_color(Risk::Low),
                    format!("\u{25cf} {} low", report.count(Risk::Low)),
                );
            });
        }

        // Diff against a loaded baseline (snapshot hunting).
        if let (Some(base), Some(report)) =
            (self.events_baseline.as_ref(), self.events_report.as_ref())
        {
            let d = diff_subscriptions(base, &report.subscriptions);
            ui.horizontal(|ui| {
                ui.strong("vs baseline:");
                ui.colored_label(
                    Color32::from_rgb(240, 100, 100),
                    format!("+{} new", d.added.len()),
                );
                ui.colored_label(
                    Color32::from_rgb(225, 185, 90),
                    format!("~{} changed", d.changed.len()),
                );
                ui.weak(format!("-{} removed", d.removed.len()));
                ui.weak(format!("\u{00b7} {} unchanged", d.unchanged));
            });
            if !d.is_empty() {
                egui::CollapsingHeader::new("Diff details")
                    .id_salt("persist-diff")
                    .default_open(true)
                    .show(ui, |ui| {
                        let sections = [
                            ("New", &d.added, Color32::from_rgb(240, 100, 100)),
                            ("Changed", &d.changed, Color32::from_rgb(225, 185, 90)),
                            (
                                "Removed (was in baseline)",
                                &d.removed,
                                Color32::from_gray(150),
                            ),
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
        ui.separator();

        let sort = self.events_sort;
        let mut header_clicked: Option<usize> = None;

        if let Some(report) = self.events_report.as_ref() {
            if report.subscriptions.is_empty() {
                ui.weak("No permanent event subscriptions found.");
            } else {
                let mut order: Vec<usize> = (0..report.subscriptions.len()).collect();
                if let Some((ci, asc)) = sort {
                    order.sort_by(|&a, &b| {
                        let o = smart_cmp(
                            &sub_col_value(&report.subscriptions[a], ci),
                            &sub_col_value(&report.subscriptions[b], ci),
                        );
                        if asc {
                            o
                        } else {
                            o.reverse()
                        }
                    });
                }

                let headers = [
                    "Risk",
                    "Consumer type",
                    "Consumer",
                    "Filter",
                    "Action / query",
                    "Why",
                ];
                let widths = [64.0, 168.0, 150.0, 150.0, 260.0, 220.0];
                let row_h = ui.text_style_height(&egui::TextStyle::Body) + 6.0;

                let mut table = TableBuilder::new(ui)
                    .id_salt("events-table")
                    .striped(true)
                    .resizable(true)
                    .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
                    .min_scrolled_height(0.0);
                for w in widths {
                    table =
                        table.column(Column::initial(w).at_least(48.0).clip(true).resizable(true));
                }
                table
                    .header(22.0, |mut header| {
                        for (ci, h) in headers.iter().enumerate() {
                            header.col(|ui| {
                                if sortable_header(ui, h, ci, sort) {
                                    header_clicked = Some(ci);
                                }
                            });
                        }
                    })
                    .body(|body| {
                        body.rows(row_h, order.len(), |mut row| {
                            let s = &report.subscriptions[order[row.index()]];
                            let color = risk_color(s.risk);
                            row.col(|ui| {
                                ui.label(
                                    egui::RichText::new(s.risk.as_str()).color(color).strong(),
                                );
                            });
                            row.col(|ui| {
                                ui.label(s.consumer_type.as_str());
                            });
                            row.col(|ui| {
                                ui.label(s.consumer_name.as_str());
                            });
                            row.col(|ui| {
                                ui.label(s.filter_name.as_str());
                            });
                            row.col(|ui| {
                                let text = if s.action.is_empty() {
                                    s.filter_query.as_str()
                                } else {
                                    s.action.as_str()
                                };
                                ui.label(text).on_hover_text(format!(
                                    "filter query:\n{}\n\naction:\n{}",
                                    s.filter_query, s.action
                                ));
                            });
                            row.col(|ui| {
                                let why = s.reasons.join("; ");
                                ui.label(egui::RichText::new(why.as_str()).color(color))
                                    .on_hover_text(why);
                            });
                        });
                    });
            }
        } else if !self.events_loading {
            ui.weak("Click Refresh to scan for WMI persistence.");
        }

        if let Some(ci) = header_clicked {
            toggle_sort(&mut self.events_sort, ci);
        }
    }
}
