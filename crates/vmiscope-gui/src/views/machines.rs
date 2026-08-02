//! The Machines view: saved connection targets, and the panel that connects to
//! a new one.
//!
//! This replaces the connection bar that lived inside the old Machines
//! placeholder. The table on the left is every target this tool knows about --
//! the machine it runs on, plus whatever the user has connected to -- and the
//! 290px panel on the right is where a new connection is arranged.
//!
//! Three honesty rules run through the whole file, and every one of them is a
//! guard against the tool claiming a capability it does not have:
//!
//! * **The transport is DCOM.** The core speaks DCOM only, so the segmented
//!   control ships "WinRM" *disabled* with a tooltip saying so, and the
//!   Transport column reads "DCOM". Certificate-thumbprint authentication --
//!   which exists only for the WSMan transport this project does not implement
//!   -- is not offered at all, not merely disabled.
//! * **RTT is the DCOM bind time**, labelled "RTT (bind)". It is the wall clock
//!   around `IWbemLocator::ConnectServer`, not an ICMP round trip and not
//!   `Win32_PingStatus` (which would measure the *connected host* to the target,
//!   not you to the target).
//! * **Status is `Unknown` until you connect or test.** Nothing here
//!   background-polls a target, because each poll is a full DCOM bind. A probe's
//!   result is cached with a timestamp, and the Status column shows that
//!   timestamp's age so a stale "Online" can never read as a live one.

use std::collections::HashMap;

use eframe::egui;
use eframe::egui::{
    Color32, Frame, Margin, Pos2, Rect, RichText, Sense, Stroke, StrokeKind, TextEdit, TextStyle,
    Ui, Vec2,
};

use crate::app::{ConnStatus, VmiScopeApp, DEFAULT_NAMESPACE};
use crate::config::{CredRef, Impersonation, Target, Transport};
use crate::theme::icons;
use crate::theme::tokens::{
    muted, BAD, BG, DIVIDER, NEUTRAL, OK, R_MD, R_SM, S2, S3, SURFACE, WARN,
};
use crate::widgets::button::{accent, btn_ghost, btn_primary, btn_secondary, focus_ring};
use crate::widgets::chip::dot_chip;
use crate::widgets::field::{combo, mono_input, radio_group};
use crate::widgets::loading::format_ms;
use crate::widgets::rule::{hrule, solid_vline, HAIRLINE};
use crate::widgets::table::{DataTable, DataTableState, Sort, TableColumn};

use vmiscope_core::{Credential, HostInfo, Impersonation as CoreImp};

/// The New-connection panel's exact width (task 5.17).
const NEW_CONN_W: f32 = 290.0;

/// A probe younger than this reads as "just now" rather than "0m ago".
const JUST_NOW_SECS: u64 = 45;

/// The result of the last connect/test for one target, held for this session
/// only. The persisted half of a probe (RTT, OS, timestamp) lives on
/// [`crate::config::Target`]; this is the transient status that a saved field
/// cannot carry -- "in flight" and "failed" are facts about a running attempt,
/// not about the machine.
enum Probe {
    /// A bind is in flight.
    Connecting,
    /// The bind succeeded. `rtt_ms` is the bind, `probe_ms` the identity queries
    /// on top of it; both are shown because they fail for different reasons.
    Online {
        rtt_ms: u64,
        probe_ms: u64,
        os: String,
        at: u64,
    },
    /// The bind failed. The message is the connection error's first line.
    Failed { message: String, at: u64 },
}

/// The Machines view's own state.
///
/// The host, credentials and "use alternate credentials" flag are deliberately
/// *not* here: they are the app's `conn_*` fields, which the shell's machine
/// chip and the method-invocation overlay also read, so the connection form
/// edits those directly rather than keeping a second copy to fall out of sync.
#[derive(Default)]
pub(crate) struct MachinesView {
    /// Per-target runtime probe state, keyed by [`Target::key`].
    probes: HashMap<String, Probe>,
    /// The target key a connect/test is in flight for, so the reply lands on the
    /// right row. One at a time: the worker is single-threaded.
    connecting: Option<String>,
    /// The namespace to seed the Explorer with once the in-flight connect lands.
    pending_namespace: Option<String>,
    /// Form: the namespace the target opens to.
    namespace: String,
    /// Form: the impersonation level to request on the proxy blanket.
    imp: Impersonation,
    /// The form has been seeded from config once this session.
    initialized: bool,
    /// The targets table's sort, kept across tab switches.
    sort: Sort,
}

/// The authentication choice, as the radio group sees it. Maps onto the app's
/// `conn_use_creds` bool, which is what everything else reads.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Auth {
    Current,
    Alternate,
}

/// A fully resolved status for one row, ready to paint.
struct StatusView {
    color: Color32,
    label: String,
    /// The probe's age, or a cached "last seen" age.
    note: Option<String>,
    /// A hover explanation -- the failure message, or the bind/probe split.
    tip: Option<String>,
}

/// One row of the targets table, with everything it shows already resolved so
/// the sort keys and the row closure need no further lookups.
struct RowView {
    /// The host name, `""` for the local machine. Loaded back into the form.
    name: String,
    display_name: String,
    namespace: String,
    cred: CredRef,
    is_local: bool,
    key: String,
    rtt_ms: Option<u64>,
    os: String,
    status: StatusView,
}

/// Seconds since the Unix epoch, or 0 on a clock that predates it. A local copy
/// of `config::unix_now`, which is private to that module.
fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// A coarse "how long ago" for a probe timestamp. Coarse on purpose: the point
/// is to say whether a reading is fresh or stale, not to the second.
fn age(at: u64) -> String {
    let secs = unix_now().saturating_sub(at);
    if secs < JUST_NOW_SECS {
        "just now".to_string()
    } else if secs < 3600 {
        format!("{}m ago", (secs / 60).max(1))
    } else if secs < 86_400 {
        format!("{}h ago", secs / 3600)
    } else {
        format!("{}d ago", secs / 86_400)
    }
}

/// The core impersonation level the config choice maps to. Kept here rather than
/// on `config::Impersonation` so the config layer does not depend on the core's
/// enum shape.
fn to_core_imp(imp: Impersonation) -> CoreImp {
    match imp {
        Impersonation::Identify => CoreImp::Identify,
        Impersonation::Impersonate => CoreImp::Impersonate,
        Impersonation::Delegate => CoreImp::Delegate,
    }
}

impl VmiScopeApp {
    // ------------------------------------------------------------------
    // Layout
    // ------------------------------------------------------------------

    pub(crate) fn ui_machines(&mut self, ui: &mut egui::Ui) {
        if !self.machines.initialized {
            // Seed the form from persisted preferences once, so the namespace and
            // impersonation start where Settings left them rather than at the
            // struct's zero value.
            self.machines.namespace = self.config.default_namespace.clone();
            self.machines.imp = self.config.impersonation;
            self.machines.initialized = true;
        }

        egui::Panel::right("vs_new_conn")
            .exact_size(NEW_CONN_W)
            .resizable(false)
            .show_separator_line(false)
            .frame(Frame::NONE.fill(BG))
            .show(ui, |ui| self.ui_new_connection(ui));

        egui::CentralPanel::default()
            .frame(Frame::NONE.fill(BG))
            .show(ui, |ui| {
                let rows = self.resolve_rows();
                self.ui_machines_header(ui, &rows);
                self.ui_altcred_banner(ui);
                hrule(ui);
                self.ui_targets_table(ui, rows);
            });
    }

    // ------------------------------------------------------------------
    // The targets table
    // ------------------------------------------------------------------

    /// Build the display list: the local machine first, then every saved target,
    /// then the active remote connection if it is not among the saved ones.
    fn resolve_rows(&self) -> Vec<RowView> {
        let mut rows = Vec::new();

        // The machine we run on is always present and never persisted.
        rows.push(self.resolve_row(
            String::new(),
            self.config.default_namespace.clone(),
            CredRef::CurrentUser,
            true,
            None,
        ));

        for t in &self.config.targets {
            rows.push(self.resolve_row(
                t.name.clone(),
                t.namespace.clone(),
                t.cred_ref.clone(),
                false,
                Some(t),
            ));
        }

        // A connection made but never saved (or in flight) still deserves a row.
        let active = self.active_target_key();
        if !matches!(self.conn_status, ConnStatus::Local) && !rows.iter().any(|r| r.key == active) {
            let cred = self.active_cred_ref();
            rows.push(self.resolve_row(
                self.conn_host.trim().trim_start_matches('\\').to_string(),
                self.machines.namespace.clone(),
                cred,
                false,
                None,
            ));
        }

        rows
    }

    fn resolve_row(
        &self,
        name: String,
        namespace: String,
        cred: CredRef,
        is_local: bool,
        saved: Option<&Target>,
    ) -> RowView {
        let key = Target {
            name: name.clone(),
            cred_ref: cred.clone(),
            ..Default::default()
        }
        .key();
        let display_name = if is_local {
            "This machine".to_string()
        } else {
            format!("\\\\{name}")
        };

        let probe = self.machines.probes.get(&key);
        let rtt_ms = match probe {
            Some(Probe::Online { rtt_ms, .. }) => Some(*rtt_ms),
            _ => saved.and_then(|t| t.last_rtt_ms),
        };
        let os = match probe {
            Some(Probe::Online { os, .. }) => os.clone(),
            _ => saved.map(|t| t.last_os.clone()).unwrap_or_default(),
        };
        let status = self.resolve_status(&key, is_local, probe, saved.and_then(|t| t.last_seen));

        RowView {
            name,
            display_name,
            namespace,
            cred,
            is_local,
            key,
            rtt_ms,
            os,
            status,
        }
    }

    /// Turn a target's runtime probe and cached facts into a paintable status.
    ///
    /// The order is deliberate: an in-flight bind wins, then this session's
    /// probe result, then the active-connection fact, then a cached "last seen",
    /// then the honest defaults. The local machine short-circuits at the top --
    /// its reachability is not in question, so a probe only fills in its RTT and
    /// OS columns rather than turning it green.
    fn resolve_status(
        &self,
        key: &str,
        is_local: bool,
        probe: Option<&Probe>,
        saved_last_seen: Option<u64>,
    ) -> StatusView {
        let neutral = NEUTRAL[4];
        if is_local {
            return StatusView {
                color: neutral,
                label: "This machine".to_string(),
                note: None,
                tip: None,
            };
        }

        let is_active = key == self.active_target_key();
        if is_active && matches!(self.conn_status, ConnStatus::Connecting) {
            return StatusView {
                color: WARN,
                label: "Connecting…".to_string(),
                note: None,
                tip: None,
            };
        }

        match probe {
            Some(Probe::Connecting) => StatusView {
                color: WARN,
                label: "Connecting…".to_string(),
                note: None,
                tip: None,
            },
            Some(Probe::Failed { message, at }) => StatusView {
                color: BAD,
                label: "Failed".to_string(),
                note: Some(age(*at)),
                tip: Some(message.clone()),
            },
            Some(Probe::Online {
                rtt_ms,
                probe_ms,
                at,
                ..
            }) => StatusView {
                color: OK,
                label: if is_active { "Connected" } else { "Online" }.to_string(),
                note: Some(format!("checked {}", age(*at))),
                tip: Some(format!(
                    "bind {} · probe {}",
                    format_ms(*rtt_ms),
                    format_ms(*probe_ms)
                )),
            },
            None => {
                if is_active {
                    StatusView {
                        color: OK,
                        label: "Connected".to_string(),
                        note: None,
                        tip: None,
                    }
                } else if let Some(seen) = saved_last_seen {
                    // Cached from an earlier session: neutral, not green, with its
                    // age spelled out so it never reads as a live check.
                    StatusView {
                        color: neutral,
                        label: "Last seen".to_string(),
                        note: Some(age(seen)),
                        tip: Some(
                            "Cached from an earlier probe — connect or test to refresh".to_string(),
                        ),
                    }
                } else {
                    StatusView {
                        color: neutral,
                        label: "Unknown".to_string(),
                        note: Some("not tested".to_string()),
                        tip: None,
                    }
                }
            }
        }
    }

    fn ui_machines_header(&self, ui: &mut Ui, rows: &[RowView]) {
        let online = rows
            .iter()
            .filter(|r| matches!(r.status.label.as_str(), "Connected" | "Online"))
            .count();
        ui.horizontal(|ui| {
            ui.label(icons::labelled_styled(
                ui,
                icons::HARD_DRIVES,
                "Connection targets",
                TextStyle::Body,
                accent(ui),
            ));
            ui.label(
                RichText::new(format!(
                    "{} target{} · {online} reachable",
                    rows.len(),
                    if rows.len() == 1 { "" } else { "s" }
                ))
                .text_style(TextStyle::Small)
                .color(muted(50)),
            );
        });
    }

    /// The banner shown while alternate credentials are active (task 5.19).
    ///
    /// The mock, and an earlier draft of the plan, had this list which operations
    /// still ran as the current user. The core's Phase 5 work closed that hole at
    /// the root -- there is now one way to bind a namespace, and it always carries
    /// the credential -- so there is nothing left to list. What remains true, and
    /// what this says instead, is that the alternate-credential path has not been
    /// exercised against a live remote host.
    fn ui_altcred_banner(&self, ui: &mut Ui) {
        if !self.conn_use_creds || matches!(self.conn_status, ConnStatus::Local) {
            return;
        }
        ui.add_space(S2);
        Frame::NONE
            .fill(WARN.gamma_multiply(0.10))
            .stroke(Stroke::new(HAIRLINE, WARN.gamma_multiply(0.45)))
            .corner_radius(R_MD)
            .inner_margin(Margin::symmetric(S3 as i8, S2 as i8))
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing.x = S2;
                    ui.label(icons::labelled_styled(
                        ui,
                        icons::SHIELD_CHECK,
                        "Alternate credentials active",
                        TextStyle::Body,
                        WARN,
                    ));
                    ui.label(
                        RichText::new(
                            "Every operation binds with these credentials; none fall back to \
                             the current user. This path is unverified against a live remote host.",
                        )
                        .text_style(TextStyle::Small)
                        .color(muted(70)),
                    );
                });
            });
    }

    fn ui_targets_table(&mut self, ui: &mut Ui, rows: Vec<RowView>) {
        if rows.is_empty() {
            ui.weak("No connection targets.");
            return;
        }

        let mut table = DataTableState {
            sort: self.machines.sort,
            selected: None,
        };
        let out = DataTable::new("machines-table")
            .selectable(true)
            .columns([
                TableColumn::initial("Target", 190.0).at_least(120.0),
                TableColumn::initial("Transport", 84.0).at_least(60.0),
                TableColumn::initial("Credential", 150.0).at_least(80.0),
                TableColumn::initial("RTT (bind)", 92.0)
                    .at_least(60.0)
                    .numeric(true),
                TableColumn::initial("OS build", 110.0).at_least(60.0),
                TableColumn::remainder("Status").at_least(150.0),
            ])
            .sort_key(|row, col| target_col_value(&rows[row], col))
            .show(ui, &mut table, rows.len(), |row| {
                let r = &rows[row.data_index()];
                let icon = if r.is_local {
                    icons::HARD_DRIVES
                } else {
                    icons::DESKTOP_TOWER
                };
                row.icon_text(icon, &r.display_name);
                row.text(Transport::Dcom.label());
                row.text(r.cred.label());
                row.text(match r.rtt_ms {
                    Some(ms) => format_ms(ms),
                    None => "—".to_string(),
                });
                row.text(if r.os.is_empty() {
                    "—".to_string()
                } else {
                    r.os.clone()
                });
                status_cell(row, &r.status);
            });

        self.machines.sort = table.sort;

        // Clicking a row loads it into the form, so a saved target can be tested
        // or reconnected without retyping it. The password is not stored, so an
        // alternate-credential target loads its user and domain but leaves the
        // password field for the user to fill.
        if let Some(i) = out.clicked {
            let r = &rows[i];
            self.conn_host = r.name.clone();
            self.machines.namespace = r.namespace.clone();
            match &r.cred {
                CredRef::CurrentUser => self.conn_use_creds = false,
                CredRef::Alt { user, domain } => {
                    self.conn_use_creds = true;
                    self.conn_user = user.clone();
                    self.conn_domain = domain.clone().unwrap_or_default();
                }
            }
        }
    }

    // ------------------------------------------------------------------
    // The New-connection panel
    // ------------------------------------------------------------------

    fn ui_new_connection(&mut self, ui: &mut Ui) {
        Frame::NONE
            .inner_margin(Margin::same(S3 as i8))
            .show(ui, |ui| {
                ui.label(icons::labelled_styled(
                    ui,
                    icons::PLUGS_CONNECTED,
                    "New connection",
                    TextStyle::Body,
                    accent(ui),
                ));
                hrule(ui);

                form_field(ui, "Computer name", |ui| {
                    mono_input(ui, &mut self.conn_host, "local (blank) or host / IP");
                });
                form_field(ui, "Namespace", |ui| {
                    mono_input(ui, &mut self.machines.namespace, DEFAULT_NAMESPACE);
                });

                form_field(ui, "Transport", transport_segmented);

                form_field(ui, "Authentication", |ui| {
                    let mut auth = if self.conn_use_creds {
                        Auth::Alternate
                    } else {
                        Auth::Current
                    };
                    if radio_group(
                        ui,
                        &mut auth,
                        &[
                            (Auth::Current, "Current user (Kerberos)"),
                            (Auth::Alternate, "Alternate credentials"),
                        ],
                    ) {
                        self.conn_use_creds = matches!(auth, Auth::Alternate);
                    }
                });

                if self.conn_use_creds {
                    form_field(ui, "User", |ui| {
                        mono_input(ui, &mut self.conn_user, "user");
                    });
                    form_field(ui, "Password", |ui| {
                        // Hand-rolled rather than `mono_input`: the kit's field has
                        // no password mode, and a masked field is the one place the
                        // difference matters. The password is never persisted; it
                        // lives here and, once connected, in the worker thread.
                        let resp = ui.add(
                            TextEdit::singleline(&mut self.conn_pass)
                                .password(true)
                                .font(TextStyle::Monospace)
                                .desired_width(f32::INFINITY)
                                .hint_text(RichText::new("password").color(muted(38)))
                                .background_color(SURFACE),
                        );
                        focus_ring(ui, &resp);
                    });
                    form_field(ui, "Domain", |ui| {
                        mono_input(ui, &mut self.conn_domain, "domain (optional)");
                    });
                }

                form_field(ui, "Impersonation", |ui| {
                    if combo(
                        ui,
                        "vs_imp",
                        &mut self.machines.imp,
                        &[
                            (Impersonation::Identify, "Identify"),
                            (Impersonation::Impersonate, "Impersonate"),
                            (Impersonation::Delegate, "Delegate"),
                        ],
                    ) {
                        // Persist the choice as the connection default, the same
                        // field Settings edits.
                        self.config.impersonation = self.machines.imp;
                        self.config.save_debounced();
                    }
                });
                // What the level means, stated rather than hidden in a tooltip:
                // this is a real security choice and Identify breaks most of WMI.
                ui.add_space(2.0);
                ui.label(
                    RichText::new(
                        "Identify refuses nearly everything a provider serves; Delegate lets \
                         the target reuse your credentials on a third machine.",
                    )
                    .text_style(TextStyle::Small)
                    .color(muted(38)),
                );

                ui.add_space(S3);
                let mut connect = false;
                let mut test = false;
                ui.horizontal(|ui| {
                    if btn_primary(ui, icons::labelled(ui, icons::PLUGS_CONNECTED, "Connect"))
                        .on_hover_text("Bind, probe, and browse this target")
                        .clicked()
                    {
                        connect = true;
                    }
                    if btn_secondary(ui, icons::labelled(ui, icons::CROSSHAIR_SIMPLE, "Test"))
                        .on_hover_text(
                            "Bind and probe to measure the RTT and read the OS. On the \
                             single-worker core this also re-points the active connection.",
                        )
                        .clicked()
                    {
                        test = true;
                    }
                });

                // Forget, only when the form matches a saved target.
                let form_key = Target {
                    name: self.conn_host.trim().trim_start_matches('\\').to_string(),
                    cred_ref: self.active_cred_ref(),
                    ..Default::default()
                }
                .key();
                if self.config.target(&form_key).is_some() {
                    ui.add_space(S2);
                    if btn_ghost(ui, icons::labelled(ui, icons::TRASH, "Forget this target"))
                        .clicked()
                    {
                        self.config.forget_target(&form_key);
                        self.machines.probes.remove(&form_key);
                    }
                }

                if connect || test {
                    self.machines_connect(test);
                }
            });
    }

    // ------------------------------------------------------------------
    // Dispatch and reply plumbing (called from state::responses)
    // ------------------------------------------------------------------

    /// Bind and probe the target the form describes.
    ///
    /// Connect and Test are the same operation on the current single-worker core
    /// -- both send one `SetHost`, a real DCOM bind plus the identity probe --
    /// so `test` is recorded for intent but does not (yet) change behaviour. A
    /// per-host worker registry would let Test probe without re-pointing the
    /// active connection; the GUI has not adopted it.
    fn machines_connect(&mut self, test: bool) {
        let _ = test;
        let host_raw = self.conn_host.trim().trim_start_matches('\\').to_string();
        let host = if host_raw.is_empty() {
            None
        } else {
            Some(host_raw.clone())
        };
        let alt = self.conn_use_creds && !self.conn_user.trim().is_empty();
        let cred = if alt {
            let d = self.conn_domain.trim();
            Some(Credential {
                user: self.conn_user.trim().to_string(),
                password: self.conn_pass.clone(),
                domain: (!d.is_empty()).then(|| d.to_string()),
            })
        } else {
            None
        };
        let cred_ref = self.active_cred_ref();
        let namespace = {
            let n = self.machines.namespace.trim();
            if n.is_empty() {
                DEFAULT_NAMESPACE.to_string()
            } else {
                n.to_string()
            }
        };

        let key = Target {
            name: host_raw.clone(),
            cred_ref: cred_ref.clone(),
            ..Default::default()
        }
        .key();

        // Keep any earlier measurements until this probe replaces them, so the
        // row does not flicker to em dashes while the bind is in flight.
        let prior = self.config.target(&key).cloned();
        self.config.upsert_target(Target {
            name: host_raw.clone(),
            namespace: namespace.clone(),
            transport: Transport::Dcom,
            cred_ref,
            last_rtt_ms: prior.as_ref().and_then(|t| t.last_rtt_ms),
            last_os: prior
                .as_ref()
                .map(|t| t.last_os.clone())
                .unwrap_or_default(),
            last_seen: prior.as_ref().and_then(|t| t.last_seen),
        });

        self.machines.probes.insert(key.clone(), Probe::Connecting);
        self.machines.connecting = Some(key);
        self.machines.pending_namespace = Some(namespace);
        self.conn_host = host_raw;

        self.apply_host(host, cred, to_core_imp(self.machines.imp));
    }

    /// Record a successful connect on the target it was for, and persist its
    /// measurements. Called from the `HostConnected` reply.
    pub(crate) fn machines_note_connected(
        &mut self,
        connect_ms: u64,
        probe_ms: u64,
        info: &HostInfo,
    ) {
        let Some(key) = self.machines.connecting.take() else {
            return;
        };
        // The "OS build" column wants the build; fall back to the summary when a
        // locked-down namespace answered the connect but not the OS query.
        let os = if info.build_number.is_empty() {
            info.summary()
        } else {
            info.build_number.clone()
        };
        let at = unix_now();
        self.machines.probes.insert(
            key.clone(),
            Probe::Online {
                rtt_ms: connect_ms,
                probe_ms,
                os: os.clone(),
                at,
            },
        );
        // The bind time is the persisted RTT; a saved target picks it up, the
        // synthetic local row does not (and does not need to).
        self.config.note_target_probe(&key, connect_ms, &os, at);
    }

    /// Record a failed connect on the target it was for. Called from the
    /// `Connect` error path.
    pub(crate) fn machines_note_connect_failed(&mut self, message: &str) {
        if let Some(key) = self.machines.connecting.take() {
            self.machines.probes.insert(
                key,
                Probe::Failed {
                    message: message
                        .lines()
                        .next()
                        .unwrap_or("connect failed")
                        .to_string(),
                    at: unix_now(),
                },
            );
        }
        self.machines.pending_namespace = None;
    }

    /// The namespace an in-flight connect asked to open, consumed by the reseed.
    pub(crate) fn machines_take_pending_namespace(&mut self) -> Option<String> {
        self.machines.pending_namespace.take()
    }

    // ------------------------------------------------------------------
    // Shared identity helpers
    // ------------------------------------------------------------------

    /// The credential the form currently describes, as a persistable reference.
    fn active_cred_ref(&self) -> CredRef {
        if self.conn_use_creds && !self.conn_user.trim().is_empty() {
            let d = self.conn_domain.trim();
            CredRef::Alt {
                user: self.conn_user.trim().to_string(),
                domain: (!d.is_empty()).then(|| d.to_string()),
            }
        } else {
            CredRef::CurrentUser
        }
    }

    /// The key of the target the worker is currently bound to.
    fn active_target_key(&self) -> String {
        match self.conn_status {
            ConnStatus::Local => Target::default().key(),
            _ => Target {
                name: self.conn_host.trim().trim_start_matches('\\').to_string(),
                cred_ref: self.active_cred_ref(),
                ..Default::default()
            }
            .key(),
        }
    }
}

// ---------------------------------------------------------------------------
// Free helpers
// ---------------------------------------------------------------------------

/// The display/sort string for a targets-table column, kept in step with the
/// header order so a header click sorts by what the column shows.
fn target_col_value(r: &RowView, col: usize) -> String {
    match col {
        0 => r.display_name.clone(),
        1 => Transport::Dcom.label().to_string(),
        2 => r.cred.label(),
        // Sort by the number, not by the formatted string.
        3 => r.rtt_ms.map(|m| m.to_string()).unwrap_or_default(),
        4 => r.os.clone(),
        5 => r.status.label.clone(),
        _ => String::new(),
    }
}

/// The Status cell: a coloured dot and label, its age beside it, and the failure
/// message or bind/probe split on hover.
fn status_cell(row: &mut crate::widgets::table::RowCtx<'_, '_, '_>, status: &StatusView) {
    let color = status.color;
    let label = status.label.clone();
    let note = status.note.clone();
    let resp = row.cell(move |ui| {
        ui.spacing_mut().item_spacing.x = S2;
        dot_chip(ui, color, &label);
        if let Some(note) = &note {
            ui.label(
                RichText::new(note)
                    .text_style(TextStyle::Small)
                    .color(muted(40)),
            );
        }
    });
    if let Some(tip) = &status.tip {
        resp.on_hover_text(tip.clone());
    }
}

/// A labelled form field: a caption over the control.
fn form_field<R>(ui: &mut Ui, label: &str, add: impl FnOnce(&mut Ui) -> R) -> R {
    ui.add_space(S3);
    ui.label(
        RichText::new(label)
            .text_style(TextStyle::Small)
            .color(muted(60)),
    );
    ui.add_space(2.0);
    add(ui)
}

/// The Transport control: DCOM selected, WinRM shown but disabled.
///
/// Hand-rolled rather than `widgets::button::segmented`, which makes every
/// option interactive. The point here is the opposite -- WinRM must be visibly
/// present *and* unusable, with a tooltip that says why, so the control tells
/// the truth about a transport this tool does not implement.
fn transport_segmented(ui: &mut Ui) {
    let a = accent(ui);
    Frame::NONE
        .stroke(Stroke::new(HAIRLINE, DIVIDER))
        .corner_radius(R_MD)
        .inner_margin(Margin::same(0))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                let dcom = transport_segment(ui, "DCOM", a, true);
                let seam = Rect::from_min_max(
                    dcom.right_top(),
                    dcom.right_bottom() + Vec2::new(HAIRLINE, 0.0),
                );
                solid_vline(ui.painter(), seam, DIVIDER);
                // WinRM is present but disabled, with the reason on hover -- the
                // control must show what this tool cannot do, not hide it.
                transport_segment(ui, "WinRM", muted(28), false);
            });
        });
}

/// One segment of the transport control. Returns its rect so the caller can draw
/// the seam. `selected` draws the accent ring; a non-selected segment is inert.
fn transport_segment(ui: &mut Ui, text: &str, color: Color32, selected: bool) -> Rect {
    let pad = Vec2::new(S3, S2);
    let font = TextStyle::Button.resolve(ui.style());
    let galley = ui.painter().layout_no_wrap(text.to_owned(), font, color);
    let size = galley.size() + pad * 2.0;
    let (rect, resp) = ui.allocate_exact_size(size, Sense::hover());
    if ui.is_rect_visible(rect) {
        if selected {
            ui.painter().rect_stroke(
                rect.shrink(1.0),
                R_SM,
                Stroke::new(HAIRLINE, color),
                StrokeKind::Inside,
            );
        }
        ui.painter().galley(
            Pos2::new(
                rect.center().x - galley.size().x * 0.5,
                rect.center().y - galley.size().y * 0.5,
            ),
            galley,
            color,
        );
    }
    if !selected {
        resp.on_hover_text("WSMan transport not implemented");
    }
    rect
}
