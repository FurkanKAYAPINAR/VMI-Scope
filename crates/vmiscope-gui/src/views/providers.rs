//! The Providers tab: WMI providers mapped to their host processes.

use eframe::egui;
use egui::Color32;
use egui_extras::{Column, TableBuilder};

use crate::app::VmiScopeApp;
use crate::theme::icons;
use crate::util::{prov_col_value, save_file, smart_cmp, toggle_sort};
use crate::widgets::table::sortable_header;

use vmiscope_core::diff_providers;

impl VmiScopeApp {
    // ------------------------------------------------------------------
    // UI: WMI providers → host processes
    // ------------------------------------------------------------------

    pub(crate) fn ui_providers(&mut self, ui: &mut egui::Ui) {
        let mut load_baseline = false;
        let mut clear_baseline = false;
        ui.horizontal(|ui| {
            ui.strong("WMI providers");
            if self.providers_loading {
                ui.spinner();
            }
            if ui
                .button(format!("{} Refresh", icons::ARROWS_CLOCKWISE))
                .clicked()
            {
                self.request_providers();
            }
            if let Some(p) = self.providers.as_ref() {
                ui.weak(format!("({})", p.len()));
                if !p.is_empty()
                    && ui
                        .button(format!("{} Snapshot", icons::FLOPPY_DISK))
                        .on_hover_text("Save a baseline")
                        .clicked()
                {
                    save_file(
                        "wmi_providers_snapshot.json",
                        &vmiscope_core::export::providers_to_json(p),
                    );
                }
            }
            if ui
                .button(format!("{} Baseline", icons::FOLDER_OPEN))
                .clicked()
            {
                load_baseline = true;
            }
            if self.providers_baseline.is_some()
                && ui.button(format!("{} clear", icons::X)).clicked()
            {
                clear_baseline = true;
            }
        });
        if load_baseline {
            if let Some(path) = rfd::FileDialog::new()
                .add_filter("JSON", &["json"])
                .pick_file()
            {
                match std::fs::read_to_string(&path)
                    .map_err(|e| e.to_string())
                    .and_then(|t| {
                        vmiscope_core::export::providers_from_json(&t).map_err(|e| e.to_string())
                    }) {
                    Ok(p) => self.providers_baseline = Some(p),
                    Err(e) => self.push_error(format!("Load provider baseline: {e}")),
                }
            }
        }
        if clear_baseline {
            self.providers_baseline = None;
        }

        // Diff against baseline.
        if let (Some(base), Some(cur)) = (self.providers_baseline.as_ref(), self.providers.as_ref())
        {
            let d = diff_providers(base, cur);
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
            });
            if !d.is_empty() {
                egui::CollapsingHeader::new("Diff details")
                    .id_salt("prov-diff")
                    .show(ui, |ui| {
                        for (title, list) in [
                            ("New", &d.added),
                            ("Changed", &d.changed),
                            ("Removed", &d.removed),
                        ] {
                            if list.is_empty() {
                                continue;
                            }
                            ui.strong(format!("{title} ({})", list.len()));
                            for p in list {
                                ui.label(format!(
                                    "    {} [{}]  pid {} {}",
                                    p.provider, p.namespace, p.host_pid, p.host_process
                                ));
                            }
                        }
                    });
            }
        }
        ui.separator();

        let sort = self.providers_sort;
        let mut header_clicked: Option<usize> = None;

        if let Some(providers) = self.providers.as_ref() {
            if providers.is_empty() {
                ui.weak("No providers returned.");
            } else {
                let mut order: Vec<usize> = (0..providers.len()).collect();
                if let Some((ci, asc)) = sort {
                    order.sort_by(|&a, &b| {
                        let o = smart_cmp(
                            &prov_col_value(&providers[a], ci),
                            &prov_col_value(&providers[b], ci),
                        );
                        if asc {
                            o
                        } else {
                            o.reverse()
                        }
                    });
                }

                let headers = [
                    "Provider",
                    "Namespace",
                    "Host PID",
                    "Host process",
                    "Hosting group",
                ];
                let widths = [220.0, 160.0, 74.0, 170.0, 200.0];
                let row_h = ui.text_style_height(&egui::TextStyle::Body) + 6.0;

                let mut table = TableBuilder::new(ui)
                    .id_salt("providers-table")
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
                            let p = &providers[order[row.index()]];
                            row.col(|ui| {
                                ui.label(p.provider.as_str());
                            });
                            row.col(|ui| {
                                ui.label(p.namespace.as_str());
                            });
                            row.col(|ui| {
                                ui.label(p.host_pid.to_string());
                            });
                            row.col(|ui| {
                                ui.label(p.host_process.as_str());
                            });
                            row.col(|ui| {
                                ui.label(p.hosting_group.as_str());
                            });
                        });
                    });
            }
        } else if !self.providers_loading {
            ui.weak("Click Refresh to list WMI providers.");
        }

        if let Some(ci) = header_clicked {
            toggle_sort(&mut self.providers_sort, ci);
        }
    }
}
