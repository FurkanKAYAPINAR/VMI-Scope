//! The Network tab: a live, self-refreshing view of the machine's sockets.

use eframe::egui;
use egui::Color32;
use egui_extras::{Column, TableBuilder};

use crate::app::{TrackedConn, VmiScopeApp};
use crate::theme::tokens::state_color;
use crate::util::{net_col_value, smart_cmp, toggle_sort};
use crate::widgets::table::sortable_header;

use vmiscope_core::Protocol;

/// How often the Network tab re-snapshots the connection table.
pub(crate) const NET_REFRESH_SECS: f64 = 1.5;
/// How long a closed connection lingers (fading) before it disappears.
pub(crate) const NET_FADE_SECS: f64 = 6.0;

impl VmiScopeApp {
    // ------------------------------------------------------------------
    // UI: live network monitor
    // ------------------------------------------------------------------

    pub(crate) fn ui_network(&mut self, ui: &mut egui::Ui, now: f64) {
        // Controls.
        ui.horizontal(|ui| {
            ui.strong("Live connections");
            if self.net_inflight {
                ui.spinner();
            }
            let pause = if self.net_paused {
                "\u{25b6} Resume"
            } else {
                "\u{23f8} Pause"
            };
            if ui.button(pause).clicked() {
                self.net_paused = !self.net_paused;
            }
            if ui.button("\u{21bb} Refresh").clicked() {
                self.request_network(now);
            }
            ui.separator();
            ui.label("\u{1f50d}");
            ui.add(
                egui::TextEdit::singleline(&mut self.net_filter)
                    .hint_text("filter process / ip / port / state")
                    .desired_width(240.0),
            );
        });

        let active = self.net_conns.values().filter(|t| t.alive).count();
        let fading = self.net_conns.len().saturating_sub(active);
        let external = self
            .net_conns
            .values()
            .filter(|t| t.conn.is_external())
            .count();
        ui.horizontal(|ui| {
            ui.colored_label(
                Color32::from_rgb(120, 210, 140),
                format!("\u{25cf} {active} active"),
            );
            ui.weak(format!("\u{25cb} {fading} closing"));
            ui.colored_label(
                Color32::from_rgb(240, 150, 90),
                format!("\u{1f310} {external} external"),
            );
            ui.checkbox(&mut self.net_external_only, "external only")
                .on_hover_text("Established TCP connections to public IPs (possible C2 / exfil)");
        });
        ui.separator();

        // Filter + sort (stable order so fading rows stay put while they dim).
        let filter = self.net_filter.to_lowercase();
        let external_only = self.net_external_only;
        let mut rows: Vec<&TrackedConn> = self
            .net_conns
            .values()
            .filter(|t| {
                if external_only && !t.conn.is_external() {
                    return false;
                }
                if filter.is_empty() {
                    return true;
                }
                let c = &t.conn;
                c.process.to_lowercase().contains(&filter)
                    || c.local_addr.contains(&filter)
                    || c.remote_addr.contains(&filter)
                    || c.local_port.to_string().contains(&filter)
                    || c.remote_port.to_string().contains(&filter)
                    || c.state.to_lowercase().contains(&filter)
            })
            .collect();
        let sort = self.net_sort;
        match sort {
            Some((ci, asc)) => rows.sort_by(|a, b| {
                let o = smart_cmp(&net_col_value(&a.conn, ci), &net_col_value(&b.conn, ci));
                if asc {
                    o
                } else {
                    o.reverse()
                }
            }),
            // Default: group by process, then local port (stable while fading).
            None => rows.sort_by(|a, b| {
                a.conn
                    .process
                    .to_lowercase()
                    .cmp(&b.conn.process.to_lowercase())
                    .then(a.conn.proto.as_str().cmp(b.conn.proto.as_str()))
                    .then(a.conn.local_port.cmp(&b.conn.local_port))
                    .then(a.conn.remote_addr.cmp(&b.conn.remote_addr))
                    .then(a.conn.remote_port.cmp(&b.conn.remote_port))
            }),
        }

        let headers = [
            "Proto",
            "State",
            "Local address",
            "L.Port",
            "Remote address",
            "R.Port",
            "PID",
            "Process",
        ];
        let widths = [52.0, 96.0, 190.0, 60.0, 190.0, 60.0, 64.0, 180.0];
        let row_h = ui.text_style_height(&egui::TextStyle::Body) + 6.0;

        let mut header_clicked: Option<usize> = None;
        let mut table = TableBuilder::new(ui)
            .id_salt("net-table")
            .striped(true)
            .resizable(true)
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .min_scrolled_height(0.0);
        for w in widths {
            table = table.column(Column::initial(w).at_least(40.0).clip(true).resizable(true));
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
                body.rows(row_h, rows.len(), |mut row| {
                    let t = rows[row.index()];
                    let c = &t.conn;
                    // Alpha: full while alive, fading toward transparent after close.
                    let alpha = if t.alive {
                        1.0
                    } else {
                        (1.0 - (now - t.last_seen) / NET_FADE_SECS).clamp(0.06, 1.0) as f32
                    };
                    let color = state_color(&c.state, c.proto).gamma_multiply(alpha);

                    let is_udp = c.proto == Protocol::Udp;
                    let cells = [
                        c.proto.as_str().to_string(),
                        if c.state.is_empty() {
                            "\u{2014}".into()
                        } else {
                            c.state.clone()
                        },
                        c.local_addr.clone(),
                        c.local_port.to_string(),
                        if c.remote_addr.is_empty() {
                            "*".into()
                        } else if c.is_external() {
                            format!("\u{1f310} {}", c.remote_addr)
                        } else {
                            c.remote_addr.clone()
                        },
                        if is_udp {
                            "*".into()
                        } else {
                            c.remote_port.to_string()
                        },
                        c.pid.to_string(),
                        c.process.clone(),
                    ];
                    for cell in cells {
                        row.col(|ui| {
                            ui.label(egui::RichText::new(cell).color(color));
                        });
                    }
                });
            });

        if let Some(ci) = header_clicked {
            toggle_sort(&mut self.net_sort, ci);
        }
    }
}
