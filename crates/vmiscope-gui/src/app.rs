//! The VMI-Scope application: state, request/response plumbing, and UI.
//!
//! The UI never blocks. Every WMI operation is dispatched to the background
//! [`WmiWorker`] with a monotonically increasing request id; replies are drained
//! once per frame in [`VmiScopeApp::handle_responses`]. `pending` maps in-flight
//! ids to what they were for, so an error reply can clear the right spinner.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use eframe::egui;
use egui::Color32;

use crate::config::Config;
use crate::state::ids::PendingKind;
use crate::views::network::NET_REFRESH_SECS;

use vmiscope_core::{
    ClassSchema, Connection, Credential, EventMonitor, MethodOutcome, MethodTarget, MonitorMsg,
    ProviderInfo, QueryResult, SearchIndex, Subscription, SubscriptionReport, WmiWorker,
    DEFAULT_EVENT_QUERY,
};

pub(crate) const ROOT_NAMESPACE: &str = "root";
pub(crate) const DEFAULT_NAMESPACE: &str = "root\\CIMV2";
const DEFAULT_QUERY: &str = "SELECT * FROM Win32_OperatingSystem";

/// The top-level tools.
#[derive(PartialEq, Eq, Clone, Copy)]
pub(crate) enum Tab {
    Explorer,
    Network,
    Persistence,
    Providers,
    Events,
}

/// A connection tracked across snapshots so it can fade out after it closes.
pub(crate) struct TrackedConn {
    pub(crate) conn: Connection,
    pub(crate) last_seen: f64,
    /// Present in the most recent snapshot?
    pub(crate) alive: bool,
}

/// Which view the Explorer central panel shows for the selected class.
#[derive(PartialEq, Eq, Clone, Copy)]
pub(crate) enum CentralView {
    Instances,
    Schema,
}

/// Connection status shown in the top bar.
#[derive(Clone, PartialEq, Eq)]
pub(crate) enum ConnStatus {
    Local,
    Connecting,
    Remote(String),
    Failed(String),
}

/// Target language for the script generator.
#[derive(PartialEq, Eq, Clone, Copy)]
pub(crate) enum ScriptLang {
    PowerShell,
    VbScript,
}

pub struct VmiScopeApp {
    pub(crate) worker: WmiWorker,
    pub(crate) next_id: u64,
    pub(crate) pending: HashMap<u64, PendingKind>,
    pub(crate) active_tab: Tab,
    /// Persisted query history + saved queries.
    pub(crate) config: Config,
    /// Cached class list per namespace (avoids re-enumerating on revisit).
    pub(crate) class_cache: HashMap<String, Vec<String>>,
    // --- connection ---
    pub(crate) conn_host: String,
    pub(crate) conn_status: ConnStatus,
    pub(crate) conn_use_creds: bool,
    pub(crate) conn_user: String,
    pub(crate) conn_pass: String,
    pub(crate) conn_domain: String,

    // --- network tab ---
    pub(crate) net_conns: HashMap<String, TrackedConn>,
    pub(crate) net_last_refresh: f64,
    pub(crate) net_inflight: bool,
    pub(crate) net_filter: String,
    pub(crate) net_paused: bool,
    pub(crate) net_external_only: bool,
    /// (column index, ascending). `None` = default (grouped by process).
    pub(crate) net_sort: Option<(usize, bool)>,

    // --- persistence tab ---
    pub(crate) events_report: Option<SubscriptionReport>,
    pub(crate) events_loading: bool,
    pub(crate) events_sort: Option<(usize, bool)>,
    /// Loaded baseline snapshot for diffing (subscriptions from a saved file).
    pub(crate) events_baseline: Option<Vec<Subscription>>,

    // --- providers tab ---
    pub(crate) providers: Option<Vec<ProviderInfo>>,
    pub(crate) providers_loading: bool,
    pub(crate) providers_sort: Option<(usize, bool)>,
    pub(crate) providers_baseline: Option<Vec<ProviderInfo>>,

    // --- events tab (live monitor) ---
    pub(crate) monitor: Option<EventMonitor>,
    pub(crate) monitor_wql: String,
    pub(crate) monitor_error: Option<String>,
    pub(crate) events_log: Vec<Vec<(String, String)>>,

    // --- namespace tree ---
    /// namespace path -> its loaded child paths (absent = not loaded yet).
    pub(crate) ns_children: HashMap<String, Vec<String>>,
    pub(crate) ns_expanded: HashSet<String>,
    pub(crate) ns_loading: HashSet<String>,
    pub(crate) active_ns: String,

    // --- class list (for the active namespace) ---
    pub(crate) classes: Vec<String>,
    pub(crate) classes_ns: String,
    pub(crate) classes_loading: bool,
    pub(crate) class_filter: String,
    pub(crate) selected_class: Option<String>,

    // --- query + results ---
    pub(crate) query_text: String,
    pub(crate) script_lang: ScriptLang,
    pub(crate) save_query_open: bool,
    pub(crate) save_query_name: String,
    pub(crate) central_view: CentralView,
    pub(crate) schema: Option<ClassSchema>,
    pub(crate) schema_class: String,
    pub(crate) schema_loading: bool,
    pub(crate) schema_filter: String,
    // --- MOF viewer (floating window) ---
    pub(crate) mof_open: bool,
    pub(crate) mof_title: String,
    pub(crate) mof_object_path: String,
    pub(crate) mof_text: Option<String>,
    pub(crate) mof_loading: bool,
    // --- Actions (method execution) panel ---
    pub(crate) actions_open: bool,
    pub(crate) act_method: Option<String>,
    pub(crate) act_args: HashMap<String, String>,
    pub(crate) act_bools: HashMap<String, bool>,
    pub(crate) act_target: String,
    pub(crate) act_instances: Option<Vec<MethodTarget>>,
    pub(crate) act_instances_loading: bool,
    pub(crate) act_invoking: bool,
    pub(crate) act_outcome: Option<(String, MethodOutcome)>,
    pub(crate) confirm_open: bool,
    // --- global search ---
    pub(crate) search_index: Option<SearchIndex>,
    pub(crate) search_loading: bool,
    pub(crate) search_text: String,
    pub(crate) search_methods: bool,
    pub(crate) latest_query_id: u64,
    pub(crate) query_loading: bool,
    pub(crate) result: Option<QueryResult>,
    pub(crate) result_wql: String,
    pub(crate) selected_row: Option<usize>,
    /// (column index, ascending). `None` = original result order.
    pub(crate) result_sort: Option<(usize, bool)>,

    // --- status ---
    pub(crate) error: Option<String>,
    pub(crate) error_log: Vec<String>,
    pub(crate) error_log_open: bool,
}

impl VmiScopeApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        // Fonts first: `set_fonts` rebuilds the atlas, and installing the style
        // against the old font set would size text against the wrong metrics.
        crate::theme::fonts::install(&cc.egui_ctx);
        crate::theme::install(&cc.egui_ctx, crate::theme::Theme::default());

        let mut app = Self {
            worker: WmiWorker::spawn(),
            next_id: 0,
            pending: HashMap::new(),
            active_tab: Tab::Explorer,
            config: Config::load(),
            class_cache: HashMap::new(),
            conn_host: String::new(),
            conn_status: ConnStatus::Local,
            conn_use_creds: false,
            conn_user: String::new(),
            conn_pass: String::new(),
            conn_domain: String::new(),
            net_conns: HashMap::new(),
            net_last_refresh: 0.0,
            net_inflight: false,
            net_filter: String::new(),
            net_paused: false,
            net_external_only: false,
            net_sort: None,
            events_report: None,
            events_loading: false,
            events_sort: None,
            events_baseline: None,
            providers: None,
            providers_loading: false,
            providers_sort: None,
            providers_baseline: None,
            monitor: None,
            monitor_wql: DEFAULT_EVENT_QUERY.to_string(),
            monitor_error: None,
            events_log: Vec::new(),
            ns_children: HashMap::new(),
            ns_expanded: HashSet::new(),
            ns_loading: HashSet::new(),
            active_ns: DEFAULT_NAMESPACE.to_string(),
            classes: Vec::new(),
            classes_ns: String::new(),
            classes_loading: false,
            class_filter: String::new(),
            selected_class: None,
            query_text: DEFAULT_QUERY.to_string(),
            script_lang: ScriptLang::PowerShell,
            save_query_open: false,
            save_query_name: String::new(),
            central_view: CentralView::Instances,
            schema: None,
            schema_class: String::new(),
            schema_loading: false,
            schema_filter: String::new(),
            mof_open: false,
            mof_title: String::new(),
            mof_object_path: String::new(),
            mof_text: None,
            mof_loading: false,
            actions_open: false,
            act_method: None,
            act_args: HashMap::new(),
            act_bools: HashMap::new(),
            act_target: String::new(),
            act_instances: None,
            act_instances_loading: false,
            act_invoking: false,
            act_outcome: None,
            confirm_open: false,
            search_index: None,
            search_loading: false,
            search_text: String::new(),
            search_methods: false,
            latest_query_id: 0,
            query_loading: false,
            result: None,
            result_wql: String::new(),
            selected_row: None,
            result_sort: None,
            error: None,
            error_log: Vec::new(),
            error_log_open: false,
        };

        // Load the root's children, expand it, focus CIMV2, and run a query so
        // the window has real content the moment it opens.
        app.ns_expanded.insert(ROOT_NAMESPACE.to_string());
        app.request_namespaces(ROOT_NAMESPACE.to_string());
        app.request_classes(DEFAULT_NAMESPACE.to_string());
        app.run_query();
        app
    }

    // ------------------------------------------------------------------
    // UI: connection bar (remote host, current user / SSO)
    // ------------------------------------------------------------------

    fn ui_connection_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label("Host:");
            let resp = ui.add(
                egui::TextEdit::singleline(&mut self.conn_host)
                    .hint_text("local (blank) or remote hostname / IP")
                    .desired_width(160.0),
            );
            ui.checkbox(&mut self.conn_use_creds, "alt creds")
                .on_hover_text(
                    "Alternate credentials for a remote host (raw DCOM).\n\u{26a0} Experimental — \
                 unverified against a live remote host. Browse/query/network/providers only.",
                );
            if self.conn_use_creds {
                ui.add(
                    egui::TextEdit::singleline(&mut self.conn_user)
                        .hint_text("user")
                        .desired_width(90.0),
                );
                ui.add(
                    egui::TextEdit::singleline(&mut self.conn_pass)
                        .password(true)
                        .hint_text("password")
                        .desired_width(90.0),
                );
                ui.add(
                    egui::TextEdit::singleline(&mut self.conn_domain)
                        .hint_text("domain")
                        .desired_width(80.0),
                );
            }
            let go = ui.button("\u{1f50c} Connect").clicked()
                || (resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)));
            if go {
                let h = self.conn_host.trim().trim_start_matches('\\').to_string();
                let host = if h.is_empty() { None } else { Some(h) };
                let cred = if self.conn_use_creds && !self.conn_user.trim().is_empty() {
                    let d = self.conn_domain.trim();
                    Some(Credential {
                        user: self.conn_user.trim().to_string(),
                        password: self.conn_pass.clone(),
                        domain: if d.is_empty() {
                            None
                        } else {
                            Some(d.to_string())
                        },
                    })
                } else {
                    None
                };
                self.apply_host(host, cred);
            }
            ui.separator();
            match &self.conn_status {
                ConnStatus::Local => {
                    ui.weak("\u{25cf} local machine");
                }
                ConnStatus::Connecting => {
                    ui.spinner();
                    ui.weak("connecting\u{2026}");
                }
                ConnStatus::Remote(h) => {
                    let mode = if self.conn_use_creds {
                        "alt creds"
                    } else {
                        "current user"
                    };
                    ui.colored_label(
                        Color32::from_rgb(120, 210, 140),
                        format!("\u{25cf} {h} ({mode})"),
                    );
                }
                ConnStatus::Failed(e) => {
                    ui.colored_label(
                        Color32::from_rgb(240, 120, 120),
                        format!("\u{2716} {}", e.lines().next().unwrap_or("failed")),
                    );
                }
            }
        });
    }

    // ------------------------------------------------------------------
    // UI: status bar
    // ------------------------------------------------------------------

    fn ui_status(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if let Some(err) = &self.error {
                ui.colored_label(egui::Color32::from_rgb(240, 90, 90), "\u{26a0} error");
                ui.weak("\u{2014}");
                ui.label(err.replace('\n', "  \u{2014}  "));
            } else {
                ui.weak(format!("Namespace: {}", self.active_ns));
                if !self.result_wql.is_empty() {
                    ui.weak("\u{2014}");
                    ui.weak(&self.result_wql);
                }
            }
            if !self.error_log.is_empty() {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .button(format!("Log ({})", self.error_log.len()))
                        .clicked()
                    {
                        self.error_log_open = !self.error_log_open;
                    }
                });
            }
        });
    }
}

impl eframe::App for VmiScopeApp {
    // eframe 0.35 hands the app the root `Ui` directly and attaches panels to it
    // via `show(ui, ..)` (instead of the older `update(ctx, ..)` model).
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let now = ui.input(|i| i.time);
        self.handle_responses(now);

        // Drain the live event monitor (if running).
        if self.monitor.is_some() {
            let msgs = self.monitor.as_ref().unwrap().poll();
            for msg in msgs {
                match msg {
                    MonitorMsg::Event(pairs) => {
                        self.events_log.insert(0, pairs);
                        self.events_log.truncate(500);
                    }
                    MonitorMsg::Error(e) => self.monitor_error = Some(e),
                }
            }
        }

        // Drive the live network refresh from the frame clock.
        if self.active_tab == Tab::Network
            && !self.net_paused
            && !self.net_inflight
            && (now - self.net_last_refresh) >= NET_REFRESH_SECS
        {
            self.request_network(now);
        }

        // Load the persistence scan the first time its tab is opened.
        if self.active_tab == Tab::Persistence
            && self.events_report.is_none()
            && !self.events_loading
        {
            self.request_events();
        }

        // Load the provider list the first time its tab is opened.
        if self.active_tab == Tab::Providers && self.providers.is_none() && !self.providers_loading
        {
            self.request_providers();
        }

        egui::Panel::top("top").show(ui, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.heading("VMI-Scope");
                ui.separator();
                ui.selectable_value(&mut self.active_tab, Tab::Explorer, "\u{1f5c2} Explorer");
                ui.selectable_value(&mut self.active_tab, Tab::Network, "\u{1f5a7} Network");
                ui.selectable_value(
                    &mut self.active_tab,
                    Tab::Persistence,
                    "\u{1f6e1} Persistence",
                );
                ui.selectable_value(&mut self.active_tab, Tab::Providers, "\u{1f9e9} Providers");
                ui.selectable_value(&mut self.active_tab, Tab::Events, "\u{1f4e1} Events");
                // No light/dark switch: Nocturne is a dark design, and the
                // light variant would be stock egui wearing our accent.
            });
            ui.separator();
            self.ui_connection_bar(ui);
            ui.add_space(4.0);
        });

        egui::Panel::bottom("status").show(ui, |ui| {
            self.ui_status(ui);
        });

        match self.active_tab {
            Tab::Explorer => {
                egui::Panel::left("browser")
                    .resizable(true)
                    .default_size(300.0)
                    .size_range(egui::Rangef::new(200.0, 520.0))
                    .show(ui, |ui| {
                        self.ui_namespace_tree(ui);
                        ui.add_space(6.0);
                        self.ui_search(ui);
                        ui.add_space(6.0);
                        self.ui_class_list(ui);
                    });

                if self.actions_open {
                    egui::Panel::right("actions")
                        .resizable(true)
                        .default_size(360.0)
                        .size_range(egui::Rangef::new(260.0, 620.0))
                        .show(ui, |ui| {
                            self.ui_actions(ui);
                        });
                }

                if self.selected_row.is_some() {
                    egui::Panel::right("detail")
                        .resizable(true)
                        .default_size(340.0)
                        .size_range(egui::Rangef::new(220.0, 560.0))
                        .show(ui, |ui| {
                            self.ui_detail(ui);
                        });
                }

                egui::CentralPanel::default().show(ui, |ui| {
                    self.ui_central(ui);
                });
            }
            Tab::Network => {
                egui::CentralPanel::default().show(ui, |ui| {
                    self.ui_network(ui, now);
                });
            }
            Tab::Persistence => {
                egui::CentralPanel::default().show(ui, |ui| {
                    self.ui_persistence(ui);
                });
            }
            Tab::Providers => {
                egui::CentralPanel::default().show(ui, |ui| {
                    self.ui_providers(ui);
                });
            }
            Tab::Events => {
                egui::CentralPanel::default().show(ui, |ui| {
                    self.ui_events(ui);
                });
            }
        }

        // MOF viewer, method-invocation confirmation, and save-query dialog float
        // above the tabs.
        self.ui_mof_window(ui.ctx());
        self.ui_confirm_window(ui.ctx());
        self.ui_save_query_window(ui.ctx());
        self.ui_error_log_window(ui.ctx());

        // Repaint while work is in flight, and continuously on the live tab.
        if !self.pending.is_empty() {
            ui.ctx().request_repaint_after(Duration::from_millis(30));
        }
        if self.active_tab == Tab::Network && !self.net_paused {
            ui.ctx().request_repaint_after(Duration::from_millis(200));
        }
        // Keep events flowing in while the monitor runs.
        if self.monitor.is_some() {
            ui.ctx().request_repaint_after(Duration::from_millis(250));
        }
    }
}
