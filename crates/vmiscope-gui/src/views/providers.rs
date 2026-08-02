//! The Providers tab: WMI providers mapped to their host processes, and the load
//! those host processes carry against the quota that will kill them for leaking.

use eframe::egui::{self, Align, Layout, Rect, RichText, Sense, TextStyle, Vec2};

use crate::app::VmiScopeApp;
use crate::config::ByteFormat;
use crate::theme::icons;
use crate::theme::tokens::{muted, BAD, NEUTRAL, R_SM, S2, TEXT, WARN};
use crate::util::{prov_col_value, save_file};
use crate::widgets::button::{accent, btn_ghost, btn_primary, btn_secondary};
use crate::widgets::chip::{count_pill, dot_chip};
use crate::widgets::loading::spinner;
use crate::widgets::rule::hrule;
use crate::widgets::table::{numeric_threshold_color, DataTable, DataTableState, TableColumn};

use vmiscope_core::{diff_providers, HostQuota, HostStats};

/// Bar height for the quota meters.
const BAR_H: f32 = 6.0;
/// The width of a host row's name/PID column.
const NAME_W: f32 = 170.0;
/// The width of each quota meter.
const METER_W: f32 = 176.0;
/// Fraction of a per-host quota at which a meter turns amber, then red. A host
/// is terminated near 1.0, so amber has to lead it by enough to be a warning
/// rather than an obituary.
const WARN_FRAC: f64 = 0.75;
const BAD_FRAC: f64 = 0.90;

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
            // Returns at once; the parse happens in `drain_io` when the file
            // arrives. See `crate::io`.
            crate::io::pick(crate::io::PickFor::ProvidersBaseline, "JSON", &["json"]);
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

        // The host processes and their load against the quota (task 5.15). This
        // is the point of the view: 58 MB is unremarkable, 58 MB of a 512 MB
        // ceiling is a provider about to be terminated, and only the second form
        // says which.
        self.ui_provider_hosts(ui);

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

    /// A picked provider baseline file, parsed. Called from `drain_io`.
    pub(crate) fn apply_provider_baseline_file(&mut self, text: &str) {
        match vmiscope_core::export::providers_from_json(text) {
            Ok(p) => self.providers_baseline = Some(p),
            Err(e) => self.push_error(format!("Load provider baseline: {e}")),
        }
    }

    // ------------------------------------------------------------------
    // UI: provider host processes, load against quota (task 5.15)
    // ------------------------------------------------------------------

    fn ui_provider_hosts(&mut self, ui: &mut egui::Ui) {
        let Some(hosts) = self.provider_hosts.as_ref() else {
            return;
        };
        if hosts.stats.is_empty() {
            return;
        }
        let byte_format = self.config.byte_format;

        ui.horizontal(|ui| {
            ui.label(icons::labelled_styled(
                ui,
                icons::CPU,
                "Provider host processes",
                TextStyle::Body,
                accent(ui),
            ));
            let cpu_note = if hosts.logical_cpus > 0 {
                format!("load vs quota · {} logical CPUs", hosts.logical_cpus)
            } else {
                "load vs quota · CPU count unknown".to_string()
            };
            ui.label(
                RichText::new(cpu_note)
                    .text_style(TextStyle::Small)
                    .color(muted(45)),
            );
            // `None` quota and a zero quota are different: the first could not be
            // read, the second is WMI's "unlimited". Only the first is a warning.
            if hosts.quota.is_none() {
                dot_chip(ui, WARN, "quota unreadable");
            }
        });
        // A scan that could not read a host says so rather than dropping the row.
        for reason in &hosts.unreadable {
            ui.label(
                RichText::new(format!("· {reason}"))
                    .text_style(TextStyle::Small)
                    .color(WARN),
            );
        }
        ui.add_space(S2);

        // Worst pressure first, so a leaking host is the one you see. Hosts with
        // no quota to measure against sink to the bottom rather than jumping.
        let quota = hosts.quota;
        let mut order: Vec<usize> = (0..hosts.stats.len()).collect();
        order.sort_by(|&a, &b| {
            pressure_of(quota.as_ref(), &hosts.stats[b])
                .total_cmp(&pressure_of(quota.as_ref(), &hosts.stats[a]))
        });

        for &i in &order {
            host_row(
                ui,
                &hosts.stats[i],
                quota.as_ref(),
                hosts.logical_cpus,
                byte_format,
            );
        }
        hrule(ui);
    }
}

// ---------------------------------------------------------------------------
// Host-process rendering
// ---------------------------------------------------------------------------

/// The worst quota fraction a host is at, or 0 when there is nothing to measure
/// -- used only for ordering, where "unmeasurable" belongs at the bottom.
fn pressure_of(quota: Option<&HostQuota>, s: &HostStats) -> f32 {
    quota
        .and_then(|q| q.pressure(s))
        .map(|(_, f)| f)
        .unwrap_or(0.0)
}

/// One host process: its identity and CPU on the left, then a meter each for the
/// three quotas WMI enforces (memory, handles, threads).
fn host_row(
    ui: &mut egui::Ui,
    s: &HostStats,
    quota: Option<&HostQuota>,
    logical_cpus: u32,
    byte_format: ByteFormat,
) {
    ui.horizontal_top(|ui| {
        let cpu = match s.cpu_of_machine(logical_cpus) {
            Some(p) => format!("{p:.1}% CPU"),
            // Withheld rather than shown 24x too large; see `HostStats`.
            None => "CPU n/a".to_string(),
        };
        ui.allocate_ui(Vec2::new(NAME_W, 0.0), |ui| {
            ui.set_width(NAME_W);
            ui.vertical(|ui| {
                ui.add(
                    egui::Label::new(icons::labelled_styled(
                        ui,
                        icons::CPU,
                        &s.instance,
                        TextStyle::Body,
                        TEXT,
                    ))
                    .truncate(),
                );
                ui.label(
                    RichText::new(format!("pid {} · {cpu}", s.pid))
                        .text_style(TextStyle::Small)
                        .color(muted(45)),
                );
            });
        });

        meter(
            ui,
            quota.and_then(|q| q.memory_fraction(s)),
            "Memory",
            &format!(
                "{} / {}",
                fmt_bytes(s.private_bytes, byte_format),
                quota
                    .map(|q| fmt_bytes(q.memory_per_host, byte_format))
                    .unwrap_or_else(|| "—".to_string())
            ),
        );
        meter(
            ui,
            quota.and_then(|q| q.handle_fraction(s)),
            "Handles",
            &format!(
                "{} / {}",
                s.handle_count,
                quota
                    .map(|q| q.handles_per_host.to_string())
                    .unwrap_or_else(|| "—".to_string())
            ),
        );
        meter(
            ui,
            quota.and_then(|q| q.thread_fraction(s)),
            "Threads",
            &format!(
                "{} / {}",
                s.thread_count,
                quota
                    .map(|q| q.threads_per_host.to_string())
                    .unwrap_or_else(|| "—".to_string())
            ),
        );
    });
    ui.add_space(S2);
}

/// A labelled usage bar. `frac` is `Some` only when there is a quota to divide
/// by; when it is `None` the track stays empty and the tooltip says why, so an
/// unconfigured quota never reads as 0% used.
fn meter(ui: &mut egui::Ui, frac: Option<f32>, label: &str, detail: &str) {
    ui.allocate_ui(Vec2::new(METER_W, 0.0), |ui| {
        ui.set_width(METER_W);
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(label)
                    .text_style(TextStyle::Small)
                    .color(muted(55)),
            );
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.label(
                    RichText::new(detail)
                        .text_style(TextStyle::Small)
                        .color(muted(48)),
                );
            });
        });
        let (rect, resp) = ui.allocate_exact_size(Vec2::new(METER_W, BAR_H), Sense::hover());
        if ui.is_rect_visible(rect) {
            ui.painter().rect_filled(rect, R_SM, NEUTRAL[8]);
            if let Some(f) = frac {
                let f = f.clamp(0.0, 1.0);
                if f > 0.0 {
                    let fill =
                        Rect::from_min_size(rect.min, Vec2::new(rect.width() * f, rect.height()));
                    ui.painter().rect_filled(
                        fill,
                        R_SM,
                        numeric_threshold_color(f64::from(f), WARN_FRAC, BAD_FRAC),
                    );
                }
            }
        }
        let tip = match frac {
            Some(f) => format!(
                "{:.0}% of the per-host {} quota",
                (f * 100.0).min(999.0),
                label.to_lowercase()
            ),
            None => "no quota configured for this metric".to_string(),
        };
        resp.on_hover_text(tip);
    });
}

/// Bytes rendered in the user's chosen base (task 2.24's `byte_format`).
fn fmt_bytes(bytes: u64, byte_format: ByteFormat) -> String {
    let (base, units): (f64, &[&str]) = match byte_format {
        ByteFormat::Binary => (1024.0, &["B", "KiB", "MiB", "GiB", "TiB"]),
        ByteFormat::Decimal => (1000.0, &["B", "KB", "MB", "GB", "TB"]),
    };
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= base && unit + 1 < units.len() {
        value /= base;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} {}", units[0])
    } else {
        format!("{value:.1} {}", units[unit])
    }
}
