//! The Network tab: a live, self-refreshing view of the machine's sockets.

use eframe::egui;

use crate::app::{TrackedConn, VmiScopeApp};
use crate::theme::icons;
use crate::theme::tokens::{state_color, DIVIDER, NEUTRAL, OK, WARN};
use crate::util::net_col_value;
use crate::widgets::button::{btn_primary, btn_secondary};
use crate::widgets::chip::{dot_chip, dot_chip_icon};
use crate::widgets::field::filter_box;
use crate::widgets::loading::{empty_state, spinner};
use crate::widgets::rule::{hrule, vrule};
use crate::widgets::table::{DataTable, DataTableState, TableColumn};

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
        // The base order: grouped by process, then protocol, then local port. It
        // has to be an order of its own rather than the map's, because a fading
        // row must keep its place for the whole of NET_FADE_SECS -- a row that
        // jumps while it dims is worse than one that just vanishes.
        //
        // Applied UNCONDITIONALLY, which is the fix for a real defect: it used
        // to run only when no column sort was active, so under a column sort the
        // rows reached `DataTable` in `HashMap` iteration order. `sort_order` is
        // stable, so ties kept that order -- and `HashMap`'s order is free to
        // change on any insert. Every row sharing a value in the sorted column
        // (all 40 `Listen` rows, every row of one process) could therefore
        // reshuffle between one 1.5 s snapshot and the next, which is the one
        // thing a fading row must not do. The column sort now rides on top of a
        // deterministic base instead.
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

        // Task 7.6: this view had no empty state at all, and drew a header rule
        // over an empty rectangle in three different situations that mean three
        // different things -- before the first snapshot lands, when a machine
        // genuinely has no sockets, and when the filters exclude everything.
        // The third is the one that matters: a security tool showing nothing
        // has to say whether that is the answer or the question.
        if rows.is_empty() {
            // From the values the filter above actually used, not from the
            // fields they came from. Caught by capture: with a stale read the
            // view reported "every socket has closed" over a chip row saying
            // "297 active", because it asked a different question from the one
            // the rows had been filtered by.
            let (title, note) = net_empty_note(
                self.net_conns.is_empty(),
                self.net_inflight,
                filter.is_empty() && !external_only,
            );
            empty_state(ui, icons::WIFI_SLASH, title, note);
            // No table this frame, so `net_sort` is simply left as it is: it
            // lives on the app precisely so it survives a frame that has
            // nothing to sort.
            return;
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

                // Every cell goes through `net_col_value`, which is also the
                // sort key -- so a placeholder can never again be visible in
                // one and absent from the other. See `util::net_col_value`.
                row.text(net_col_value(c, 0));
                row.text(net_col_value(c, 1));
                row.text(net_col_value(c, 2));
                row.text(net_col_value(c, 3));
                // One cell either way -- the external address is led by an icon,
                // which needs its own section in the icon family. Same string.
                let remote = net_col_value(c, 4);
                if c.is_external() {
                    row.icon_text(icons::GLOBE_SIMPLE, &remote);
                } else {
                    row.text(remote);
                }
                row.text(net_col_value(c, 5));
                row.text(net_col_value(c, 6));
                row.text(net_col_value(c, 7));
            });
        self.net_sort = table.sort;
    }
}

/// Which "nothing here" this is. Three states, three sentences.
///
/// `unfiltered` is whether the view is showing everything it has -- a filtered
/// empty table is a statement about the filter, and reporting it as "no
/// connections" would be the view lying about the machine.
fn net_empty_note(
    no_snapshot: bool,
    inflight: bool,
    unfiltered: bool,
) -> (&'static str, &'static str) {
    if no_snapshot {
        if inflight {
            (
                "Reading the connection table",
                "The first snapshot is on its way.",
            )
        } else {
            (
                "No connections",
                "The snapshot came back empty. On a running machine that is unusual \u{2014} it \
                 normally means MSFT_NetTCPConnection returned nothing, not that the machine has \
                 no sockets.",
            )
        }
    } else if unfiltered {
        (
            "No connections",
            "Every socket in the last snapshot has closed and finished fading.",
        )
    } else {
        (
            "No rows match the filters",
            "The machine has connections; none of them match. Clear the text filter or turn off \
             'external only'.",
        )
    }
}
