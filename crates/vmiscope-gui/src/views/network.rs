//! The Network tab: a live, self-refreshing view of the machine's sockets.

use eframe::egui;

use crate::app::{TrackedConn, VmiScopeApp};
use crate::theme::icons;
use crate::theme::tokens::{state_color, DIVIDER, NEUTRAL, OK, WARN};
use crate::util::net_col_value;
use crate::widgets::button::{btn_primary, btn_secondary};
use crate::widgets::chip::{dot_chip, dot_chip_icon};
use crate::widgets::field::filter_box;
use crate::widgets::loading::spinner;
use crate::widgets::rule::{hrule, vrule};
use crate::widgets::table::{DataTable, DataTableState, TableColumn};

use vmiscope_core::Protocol;

/// How often the Network tab re-snapshots the connection table.
pub(crate) const NET_REFRESH_SECS: f64 = 1.5;
/// How long a closed connection lingers (fading) before it disappears.
pub(crate) const NET_FADE_SECS: f64 = 6.0;

/// `filter_box` fills whatever width it is handed, and a filter that runs the
/// width of a 4K window is harder to read than one the size of what you type
/// into it.
const FILTER_W: f32 = 240.0;

impl VmiScopeApp {
    // ------------------------------------------------------------------
    // UI: live network monitor
    // ------------------------------------------------------------------

    pub(crate) fn ui_network(&mut self, ui: &mut egui::Ui, now: f64) {
        // Controls.
        ui.horizontal(|ui| {
            ui.strong("Live connections");
            if self.net_inflight {
                spinner(ui, "refreshing");
            }
            let pause = if self.net_paused {
                icons::labelled(ui, icons::PLAY, "Resume")
            } else {
                icons::labelled(ui, icons::PAUSE, "Pause")
            };
            // Pause is the primary, not Refresh: this tab re-snapshots itself
            // every NET_REFRESH_SECS, so stopping it is the only thing the user
            // actually has to decide.
            if btn_primary(ui, pause).clicked() {
                self.net_paused = !self.net_paused;
            }
            if btn_secondary(ui, icons::labelled(ui, icons::ARROWS_CLOCKWISE, "Refresh")).clicked()
            {
                self.request_network(now);
            }
            vrule(ui, DIVIDER);
            ui.scope(|ui| {
                ui.set_max_width(FILTER_W);
                filter_box(
                    ui,
                    &mut self.net_filter,
                    "filter process / ip / port / state",
                );
            });
        });

        let active = self.net_conns.values().filter(|t| t.alive).count();
        let fading = self.net_conns.len().saturating_sub(active);
        let external = self
            .net_conns
            .values()
            .filter(|t| t.conn.is_external())
            .count();
        ui.horizontal(|ui| {
            dot_chip(ui, OK, &format!("{active} active"));
            // The same neutral `state_color` gives a closed socket, so the chip
            // and the rows it counts dim to the same colour.
            dot_chip(ui, NEUTRAL[5], &format!("{fading} closing"));
            dot_chip_icon(
                ui,
                WARN,
                icons::GLOBE_SIMPLE,
                &format!("{external} external"),
            );
            ui.checkbox(&mut self.net_external_only, "external only")
                .on_hover_text("Established TCP connections to public IPs (possible C2 / exfil)");
        });
        hrule(ui);

        // Filter. Sorting is the table's, keyed by `net_col_value`.
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
        if self.net_sort.is_none() {
            // The unsorted order: grouped by process, then local port. It has to
            // be an order of its own rather than the map's, because a fading row
            // must keep its place for the whole of NET_FADE_SECS -- a row that
            // jumps while it dims is worse than one that just vanishes.
            rows.sort_by(|a, b| {
                a.conn
                    .process
                    .to_lowercase()
                    .cmp(&b.conn.process.to_lowercase())
                    .then(a.conn.proto.as_str().cmp(b.conn.proto.as_str()))
                    .then(a.conn.local_port.cmp(&b.conn.local_port))
                    .then(a.conn.remote_addr.cmp(&b.conn.remote_addr))
                    .then(a.conn.remote_port.cmp(&b.conn.remote_port))
            });
        }

        // The sort lives on the app so it survives a tab switch; the table gets
        // it on loan for the frame.
        let mut table = DataTableState {
            sort: self.net_sort,
            selected: None,
        };
        DataTable::new("net-table")
            .columns([
                TableColumn::initial("Proto", 52.0),
                TableColumn::initial("State", 96.0),
                TableColumn::initial("Local address", 190.0),
                TableColumn::initial("L.Port", 60.0).numeric(true),
                TableColumn::initial("Remote address", 190.0),
                TableColumn::initial("R.Port", 60.0).numeric(true),
                TableColumn::initial("PID", 64.0).numeric(true),
                TableColumn::initial("Process", 180.0),
            ])
            .sort_key(|row, col| net_col_value(&rows[row].conn, col))
            .show(ui, &mut table, rows.len(), |row| {
                let t = rows[row.data_index()];
                let c = &t.conn;
                // Full while alive, fading toward transparent after close. The
                // floor keeps the last second of the fade legible.
                let alpha = if t.alive {
                    1.0
                } else {
                    (1.0 - (now - t.last_seen) / NET_FADE_SECS).clamp(0.06, 1.0) as f32
                };
                row.set_alpha(alpha);
                row.set_color(state_color(&c.state, c.proto));

                row.text(c.proto.as_str());
                row.text(if c.state.is_empty() {
                    "\u{2014}".to_string()
                } else {
                    c.state.clone()
                });
                row.text(c.local_addr.as_str());
                row.text(c.local_port.to_string());
                // One cell either way -- the external address is led by an icon,
                // which needs its own section in the icon family.
                if c.remote_addr.is_empty() {
                    row.text("*");
                } else if c.is_external() {
                    row.icon_text(icons::GLOBE_SIMPLE, &c.remote_addr);
                } else {
                    row.text(c.remote_addr.as_str());
                }
                row.text(if c.proto == Protocol::Udp {
                    "*".to_string()
                } else {
                    c.remote_port.to_string()
                });
                row.text(c.pid.to_string());
                row.text(c.process.as_str());
            });
        self.net_sort = table.sort;
    }
}
