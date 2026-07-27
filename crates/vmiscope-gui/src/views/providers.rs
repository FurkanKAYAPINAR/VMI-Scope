//! The Providers tab: WMI providers mapped to their host processes.

use eframe::egui;

use crate::app::VmiScopeApp;
use crate::theme::icons;
use crate::theme::tokens::{muted, BAD, WARN};
use crate::util::{prov_col_value, save_file};
use crate::widgets::button::{btn_ghost, btn_primary, btn_secondary};
use crate::widgets::chip::{count_pill, dot_chip};
use crate::widgets::loading::spinner;
use crate::widgets::rule::hrule;
use crate::widgets::table::{DataTable, DataTableState, TableColumn};

use vmiscope_core::diff_providers;

/// Strength of the muted diff text -- removed rows are context, not a finding:
/// a provider that went away is the one thing here nobody hunts for.
const DIM: u8 = 55;

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
                spinner(ui, "listing");
            }
            if btn_primary(ui, icons::labelled(ui, icons::ARROWS_CLOCKWISE, "Refresh")).clicked() {
                self.request_providers();
            }
            if let Some(p) = self.providers.as_ref() {
                count_pill(ui, p.len());
                if !p.is_empty()
                    && btn_secondary(ui, icons::labelled(ui, icons::FLOPPY_DISK, "Snapshot"))
                        .on_hover_text("Save a baseline")
                        .clicked()
                {
                    save_file(
                        "wmi_providers_snapshot.json",
                        &vmiscope_core::export::providers_to_json(p),
                    );
                }
            }
            if btn_secondary(ui, icons::labelled(ui, icons::FOLDER_OPEN, "Baseline")).clicked() {
                load_baseline = true;
            }
            if self.providers_baseline.is_some()
                && btn_ghost(ui, icons::labelled(ui, icons::X, "clear")).clicked()
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
                dot_chip(ui, BAD, &format!("+{} new", d.added.len()));
                dot_chip(ui, WARN, &format!("~{} changed", d.changed.len()));
                dot_chip(ui, muted(DIM), &format!("-{} removed", d.removed.len()));
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
        hrule(ui);

        // The sort lives on the app so it survives a tab switch; the table gets
        // it on loan for the frame.
        let mut table = DataTableState {
            sort: self.providers_sort,
            selected: None,
        };

        if let Some(providers) = self.providers.as_ref() {
            if providers.is_empty() {
                ui.weak("No providers returned.");
            } else {
                DataTable::new("providers-table")
                    .columns([
                        TableColumn::initial("Provider", 220.0).at_least(48.0),
                        TableColumn::initial("Namespace", 160.0).at_least(48.0),
                        TableColumn::initial("Host PID", 74.0)
                            .at_least(48.0)
                            .numeric(true),
                        TableColumn::initial("Host process", 170.0).at_least(48.0),
                        TableColumn::initial("Hosting group", 200.0).at_least(48.0),
                    ])
                    .sort_key(|row, col| prov_col_value(&providers[row], col))
                    .show(ui, &mut table, providers.len(), |row| {
                        let p = &providers[row.data_index()];
                        row.text(p.provider.as_str());
                        row.text(p.namespace.as_str());
                        row.text(p.host_pid.to_string());
                        row.text(p.host_process.as_str());
                        row.text(p.hosting_group.as_str());
                    });
            }
        } else if !self.providers_loading {
            ui.weak("Click Refresh to list WMI providers.");
        }

        self.providers_sort = table.sort;
    }
}
