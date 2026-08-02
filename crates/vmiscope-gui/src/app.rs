//! The VMI-Scope application: state, request/response plumbing, and UI.
//!
//! The UI never blocks. Every WMI operation is dispatched to the background
//! [`WmiWorker`] with a monotonically increasing request id; replies are drained
//! once per frame in [`VmiScopeApp::handle_responses`]. `pending` maps in-flight
//! ids to what they were for, so an error reply can clear the right spinner.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use eframe::egui;

use crate::config::Config;
use crate::shell;
use crate::state::ids::PendingKind;
use crate::theme::icons;
use crate::theme::tokens::{muted, BAD, DIVIDER, NEUTRAL, OK, S2, S6, SURFACE};
use crate::views::nav::View;
use crate::views::network::NET_REFRESH_SECS;
use crate::views::process::ProcessView;
use crate::views::saved::SavedView;
use crate::widgets::button::{btn_primary, focus_ring};
use crate::widgets::card::card;
use crate::widgets::chip::dot_chip;
use crate::widgets::field::mono_input;
use crate::widgets::loading::spinner;
use crate::widgets::rule::vrule;

use vmiscope_core::{
    AssocInfo, ClassBrief, ClassSchema, Completion, Connection, Credential, EventMonitor,
    MethodOutcome, MethodTarget, MonitorMsg, NamespaceStats, ProviderInfo, QueryResult,
    SearchIndex, Subscription, SubscriptionReport, Tally, WmiWorker, DEFAULT_EVENT_QUERY,
};

pub(crate) const ROOT_NAMESPACE: &str = "root";
pub(crate) const DEFAULT_NAMESPACE: &str = "root\\CIMV2";
const DEFAULT_QUERY: &str = "SELECT * FROM Win32_OperatingSystem";

/// Widths of the connection bar's fields. The kit's inputs fill whatever they
/// are handed, and a host box the width of the window would swamp the row.
const HOST_W: f32 = 160.0;
const CRED_W: f32 = 90.0;
const DOMAIN_W: f32 = 80.0;

/// The temporary connect card's maximum width inside the Machines placeholder.
/// Full-bleed, the row's fields would spread across the whole window.
const CONNECT_CARD_W: f32 = 640.0;

/// A connection tracked across snapshots so it can fade out after it closes.
pub(crate) struct TrackedConn {
    pub(crate) conn: Connection,
    pub(crate) last_seen: f64,
    /// Present in the most recent snapshot?
    pub(crate) alive: bool,
}

/// Which sub-tab the Explorer detail pane shows for the selected class.
///
/// Named `CentralView` for continuity: the command palette (`overlays::palette`)
/// and the global search (`views::explorer::search`) both reach for
/// `CentralView::Instances` by name, so the type keeps it. The three tabs added
/// by the Phase 3 rebuild sit alongside the original two.
#[derive(PartialEq, Eq, Clone, Copy)]
pub(crate) enum CentralView {
    Instances,
    Properties,
    Methods,
    Schema,
    Code,
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
    /// Where we are. The rail's eleven destinations; see `views::nav`.
    pub(crate) view: View,
    /// `--decorated`: the OS draws the caption and the resize border, so the
    /// shell must not draw its own. Read by the title bar and by `shell::chrome`.
    pub(crate) decorated: bool,
    /// The command palette's open flag. Set by the title bar's trigger, by
    /// Ctrl+K, and cleared by the palette itself. See `overlays::palette`.
    pub(crate) palette_open: bool,
    /// Was the palette drawn last frame? The difference against `palette_open`
    /// is what the palette reads as "just opened", which is the one frame it
    /// takes focus and selects the previous query.
    pub(crate) palette_shown: bool,
    /// The palette's query. Kept across opens deliberately -- it is pre-selected
    /// when the palette reopens, so it is a starting point rather than clutter.
    pub(crate) palette_query: String,
    /// Index of the highlighted palette row.
    pub(crate) palette_sel: usize,
    /// Persisted query history + saved queries.
    pub(crate) config: Config,
    /// Cached class list per namespace (avoids re-enumerating on revisit).
    pub(crate) class_cache: HashMap<String, Vec<ClassBrief>>,
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

    // --- process tab ---
    /// The live start/stop monitor, the log it retains, and the filters over
    /// it. One struct rather than a dozen fields, because unlike the older tabs
    /// this view owns a subscription with a lifetime of its own; see
    /// `views::process`.
    pub(crate) proc: ProcessView,

    // --- saved library view ---
    /// The Saved view's own filters. One struct rather than four fields, for the
    /// same reason `proc` is one: they mean nothing outside that view.
    pub(crate) saved_view: SavedView,

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
    pub(crate) classes: Vec<ClassBrief>,
    pub(crate) classes_ns: String,
    pub(crate) classes_loading: bool,
    pub(crate) class_filter: String,
    pub(crate) selected_class: Option<String>,
    /// The active class-list facet chip (All / Dynamic / Association / Event /
    /// System), matched against each row's `ClassKind`.
    pub(crate) class_chip: crate::views::explorer::ClassChip,

    // --- explorer counts (per active namespace) ---
    /// Per-class instance counts, keyed by class name. A `Tally` rather than a
    /// number so a skipped class shows an em dash and a timed-out one shows a
    /// lower bound -- a zero that means "we did not look" is a lie the core's
    /// own type exists to prevent.
    pub(crate) instance_counts: HashMap<String, Tally>,
    /// Classes with an instance count in flight: a row shows a spinner and a
    /// second request is not queued.
    pub(crate) counting: HashSet<String>,
    /// Per-namespace class/child stats for the tree, keyed by namespace path.
    pub(crate) ns_stats: HashMap<String, NamespaceStats>,
    /// Namespaces with a stats request in flight (dedupe + one lazy fire).
    pub(crate) ns_stats_pending: HashSet<String>,
    /// Namespaces whose stats request failed (almost always access-denied, e.g.
    /// `root\SECURITY`). Kept so the tree does not re-request them every repaint
    /// -- a lazy count that can never succeed must be tried once, not forever.
    pub(crate) ns_stats_failed: HashSet<String>,
    /// Elapsed ms of the most recent namespace-stats reply, for the tree footer.
    pub(crate) last_ns_stats_ms: Option<u64>,

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
    // --- associations (Schema sub-tab) ---
    /// The class whose associations `associations` holds.
    pub(crate) assoc_class: String,
    pub(crate) associations: Option<Vec<AssocInfo>>,
    pub(crate) assoc_loading: bool,
    /// Why an association lookup was partial, if it was.
    pub(crate) assoc_completion: Completion,
    // --- Code sub-tab ---
    /// The Code sub-tab's language. Four-way, unlike the two-way `script_lang`
    /// that Settings persists; PowerShell and VBScript delegate to
    /// `util::generate_script` through `script_lang`, C# and WQL are generated
    /// in `views::explorer::code`.
    pub(crate) code_tab: crate::views::explorer::CodeTab,
    // --- MOF viewer (floating window) ---
    pub(crate) mof_open: bool,
    pub(crate) mof_title: String,
    pub(crate) mof_object_path: String,
    pub(crate) mof_text: Option<String>,
    pub(crate) mof_loading: bool,
    // --- Actions (method execution) ---
    /// Raised by the explorer's Invoke triggers, which still open the old
    /// `Panel::right`; the `ui_actions` trampoline in `overlays::invoke` hands
    /// that across to `invoke_open`. Retained only until the explorer view opens
    /// the modal directly and drops the panel (task 3.32).
    pub(crate) actions_open: bool,
    /// The method-invocation modal's open flag. See `overlays::invoke`.
    pub(crate) invoke_open: bool,
    pub(crate) act_method: Option<String>,
    pub(crate) act_args: HashMap<String, String>,
    pub(crate) act_bools: HashMap<String, bool>,
    pub(crate) act_target: String,
    pub(crate) act_instances: Option<Vec<MethodTarget>>,
    pub(crate) act_instances_loading: bool,
    pub(crate) act_invoking: bool,
    pub(crate) act_outcome: Option<(String, MethodOutcome)>,
    /// The modal's confirm step is armed (Review clicked); the next Yes fires.
    pub(crate) act_armed: bool,
    /// Frame time the invoke was sent, and the measured round trip once the reply
    /// lands -- `MethodOutcome` carries no timing of its own, so the modal times
    /// it here.
    pub(crate) act_invoke_started: Option<f64>,
    pub(crate) act_elapsed_ms: Option<u64>,
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
    pub fn new(cc: &eframe::CreationContext<'_>, decorated: bool) -> Self {
        // Fonts first: `set_fonts` rebuilds the atlas, and installing the style
        // against the old font set would size text against the wrong metrics.
        crate::theme::fonts::install(&cc.egui_ctx);
        // Load config before installing the style so a persisted accent/density
        // lands on the first frame instead of flashing the default and swapping.
        let config = Config::load();
        crate::theme::install(
            &cc.egui_ctx,
            crate::theme::Theme {
                accent: config.accent,
                density: config.density,
            },
        );
        // The generator's live language starts from the persisted default; the
        // Settings control keeps the two in step after that.
        let script_lang = match config.default_lang {
            crate::config::CodeLang::PowerShell => ScriptLang::PowerShell,
            crate::config::CodeLang::VbScript => ScriptLang::VbScript,
        };
        // The Code sub-tab starts on the same language Settings persists; the two
        // stay in step because the tab writes PowerShell/VBScript back through
        // `script_lang`. Read here (rather than from `config` in the struct
        // literal) because `config` is moved into the struct below.
        let code_tab = match config.default_lang {
            crate::config::CodeLang::PowerShell => crate::views::explorer::CodeTab::PowerShell,
            crate::config::CodeLang::VbScript => crate::views::explorer::CodeTab::VbScript,
        };

        // Windows 11 rounds a decorated window itself; an undecorated one keeps
        // square corners unless DWM is told otherwise, and the shell frame's
        // `R_LG` radius would then be drawn inside a square hole.
        #[cfg(windows)]
        if !decorated {
            use winit::platform::windows::{CornerPreference, WindowExtWindows};
            if let Some(window) = cc.winit_window() {
                window.set_corner_preference(CornerPreference::Round);
            }
        }

        let mut app = Self {
            worker: WmiWorker::spawn(),
            next_id: 0,
            pending: HashMap::new(),
            view: View::Explorer,
            decorated,
            palette_open: false,
            palette_shown: false,
            palette_query: String::new(),
            palette_sel: 0,
            config,
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
            proc: ProcessView::default(),
            saved_view: SavedView::default(),
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
            class_chip: crate::views::explorer::ClassChip::All,
            instance_counts: HashMap::new(),
            counting: HashSet::new(),
            ns_stats: HashMap::new(),
            ns_stats_pending: HashSet::new(),
            ns_stats_failed: HashSet::new(),
            last_ns_stats_ms: None,
            query_text: DEFAULT_QUERY.to_string(),
            script_lang,
            save_query_open: false,
            save_query_name: String::new(),
            central_view: CentralView::Instances,
            schema: None,
            schema_class: String::new(),
            schema_loading: false,
            schema_filter: String::new(),
            assoc_class: String::new(),
            associations: None,
            assoc_loading: false,
            assoc_completion: Completion::Complete,
            code_tab,
            mof_open: false,
            mof_title: String::new(),
            mof_object_path: String::new(),
            mof_text: None,
            mof_loading: false,
            actions_open: false,
            invoke_open: false,
            act_method: None,
            act_args: HashMap::new(),
            act_bools: HashMap::new(),
            act_target: String::new(),
            act_instances: None,
            act_instances_loading: false,
            act_invoking: false,
            act_outcome: None,
            act_armed: false,
            act_invoke_started: None,
            act_elapsed_ms: None,
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
    // Shell services
    // ------------------------------------------------------------------

    /// Re-fetch whatever the active view is showing. Bound to the title bar's
    /// refresh button, and to F5 once task 2.21 lands the shortcut.
    ///
    /// The placeholder destinations have nothing to re-fetch, and Events is
    /// deliberately not restarted: the monitor is a live subscription on the
    /// far side, so tearing it down and rebuilding it is a decision its own
    /// view asks for explicitly.
    pub(crate) fn refresh_active_view(&mut self, now: f64) {
        match self.view {
            View::Explorer | View::Query => self.run_query(),
            View::Network => self.request_network(now),
            View::Persistence => self.request_events(),
            View::Providers => self.request_providers(),
            // Saved has nothing to re-fetch: the library is a file, not a WMI
            // query, and it is written by this process alone.
            View::Events
            | View::Process
            | View::Saved
            | View::Compare
            | View::Machines
            | View::Settings => {}
        }
    }

    /// The empty state a destination shows before it is built.
    ///
    /// The rail deliberately lists every planned destination, so every one of
    /// them has to land somewhere that says what it will be rather than on a
    /// blank pane that reads as a rendering failure.
    fn ui_placeholder(&self, ui: &mut egui::Ui, view: View) {
        ui.vertical_centered(|ui| {
            ui.add_space(S6);
            ui.label(icons::glyph(view.icon()).size(34.0).color(muted(20)));
            ui.add_space(S2);
            ui.heading(view.title());
            ui.label(egui::RichText::new(view.hint()).color(muted(55)));
            ui.add_space(S2);
            ui.label(
                egui::RichText::new("Not built yet \u{2014} this destination is on the roadmap.")
                    .text_style(egui::TextStyle::Small)
                    .color(muted(38)),
            );
        });
    }

    // ------------------------------------------------------------------
    // UI: connection bar (remote host, current user / SSO)
    //
    // Moved out of the top bar by task 2.26. Its logic is untouched; only its
    // home changed. It now lives inside the Machines placeholder, which is
    // where the real connection manager lands in Phase 5 and where the title
    // bar's machine chip already navigates -- so someone looking for "point
    // this at another box" arrives at it from the thing that shows the box.
    // ------------------------------------------------------------------

    fn ui_connection_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label("Host:");
            let resp = ui
                .scope(|ui| {
                    ui.set_max_width(HOST_W);
                    mono_input(
                        ui,
                        &mut self.conn_host,
                        "local (blank) or remote hostname / IP",
                    )
                })
                .inner;
            // No icon in the tooltip: `on_hover_text` takes a plain string, so
            // there is nowhere to name the icon family and the glyph would
            // render in whatever font answered its codepoint.
            ui.checkbox(&mut self.conn_use_creds, "alt creds")
                .on_hover_text(
                    "Alternate credentials for a remote host (raw DCOM).\nExperimental — \
                 unverified against a live remote host. Browse/query/network/providers only.",
                );
            if self.conn_use_creds {
                ui.scope(|ui| {
                    ui.set_max_width(CRED_W);
                    mono_input(ui, &mut self.conn_user, "user");
                });
                // Hand-rolled rather than `mono_input`: the kit's field has no
                // password mode, and a masked field is the one place where the
                // difference matters. Everything else about it is the kit's.
                ui.scope(|ui| {
                    ui.set_max_width(CRED_W);
                    let pass = ui.add(
                        egui::TextEdit::singleline(&mut self.conn_pass)
                            .password(true)
                            .font(egui::TextStyle::Monospace)
                            .hint_text(egui::RichText::new("password").color(muted(38)))
                            .background_color(SURFACE),
                    );
                    focus_ring(ui, &pass);
                });
                ui.scope(|ui| {
                    ui.set_max_width(DOMAIN_W);
                    mono_input(ui, &mut self.conn_domain, "domain");
                });
            }
            let go = btn_primary(ui, icons::labelled(ui, icons::PLUGS_CONNECTED, "Connect"))
                .clicked()
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
            vrule(ui, DIVIDER);
            match &self.conn_status {
                // The dot was already the shape here, drawn as a text glyph;
                // `dot_chip` is the same thing with the design's metrics. Local
                // is neutral rather than OK: it is the resting state, not an
                // achievement.
                ConnStatus::Local => {
                    dot_chip(ui, NEUTRAL[4], "local machine");
                }
                ConnStatus::Connecting => {
                    spinner(ui, "connecting\u{2026}");
                }
                ConnStatus::Remote(h) => {
                    let mode = if self.conn_use_creds {
                        "alt creds"
                    } else {
                        "current user"
                    };
                    dot_chip(ui, OK, &format!("{h} ({mode})"));
                }
                ConnStatus::Failed(e) => {
                    ui.label(icons::labelled_styled(
                        ui,
                        icons::X,
                        e.lines().next().unwrap_or("failed"),
                        egui::TextStyle::Body,
                        BAD,
                    ));
                }
            }
        });
    }

    // The status bar moved wholesale into `shell::statusbar` with task 2.15;
    // its error / namespace / query line and the `Log (n)` toggle live there.

    /// The per-view content, added inside the shell's central panel.
    ///
    /// Every one of the eleven destinations is dispatched here. The six with no
    /// view yet get the empty state rather than falling through to a blank
    /// pane.
    fn ui_view(&mut self, ui: &mut egui::Ui, now: f64) {
        match self.view {
            View::Explorer => {
                // The whole three-column layout (tree 224 · classes 290 · detail)
                // plus the sub-tab strip lives in `views::explorer`.
                self.ui_explorer(ui);
            }
            View::Query => {
                // Owns its own panels (the 262px history rail and the row-detail
                // reveal), so it takes the `Ui` rather than a central panel.
                self.ui_query(ui);
            }
            View::Saved => {
                egui::CentralPanel::default().show(ui, |ui| {
                    self.ui_saved(ui);
                });
            }
            View::Process => {
                egui::CentralPanel::default().show(ui, |ui| {
                    self.ui_process(ui, now);
                });
            }
            View::Network => {
                egui::CentralPanel::default().show(ui, |ui| {
                    self.ui_network(ui, now);
                });
            }
            View::Persistence => {
                egui::CentralPanel::default().show(ui, |ui| {
                    self.ui_persistence(ui);
                });
            }
            View::Providers => {
                egui::CentralPanel::default().show(ui, |ui| {
                    self.ui_providers(ui);
                });
            }
            View::Events => {
                egui::CentralPanel::default().show(ui, |ui| {
                    self.ui_events(ui);
                });
            }
            View::Machines => {
                egui::CentralPanel::default().show(ui, |ui| {
                    self.ui_placeholder(ui, View::Machines);
                    ui.add_space(S6);
                    self.ui_connect_card(ui);
                });
            }
            View::Settings => {
                egui::CentralPanel::default().show(ui, |ui| {
                    self.ui_settings(ui);
                });
            }
            other => {
                egui::CentralPanel::default().show(ui, |ui| {
                    self.ui_placeholder(ui, other);
                });
            }
        }
    }

    /// The temporary home of the connection bar. See the note above
    /// `ui_connection_bar`.
    fn ui_connect_card(&mut self, ui: &mut egui::Ui) {
        let width = CONNECT_CARD_W.min(ui.available_width());
        ui.vertical_centered(|ui| {
            ui.allocate_ui(egui::vec2(width, 0.0), |ui| {
                ui.set_width(width);
                card(ui, |ui| {
                    ui.label(icons::labelled(
                        ui,
                        icons::PLUGS_CONNECTED,
                        "Connect to a host",
                    ));
                    ui.add_space(S2);
                    self.ui_connection_bar(ui);
                });
            });
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

        // Drain the process monitor. Unlike the event log above, this one is
        // filled whatever view is on screen: the question it answers is "what
        // ran while I wasn't watching", which cannot be answered by a
        // subscription that only collects while its own tab is visible.
        self.drain_processes(now);

        // Drive the live network refresh from the frame clock. `live_polling` is
        // the Settings-level switch for all live views; `net_paused` is the
        // Network view's own per-visit pause on top of it.
        if self.view == View::Network
            && self.config.live_polling
            && !self.net_paused
            && !self.net_inflight
            && (now - self.net_last_refresh) >= NET_REFRESH_SECS
        {
            self.request_network(now);
        }

        // Load the persistence scan the first time its view is opened.
        if self.view == View::Persistence && self.events_report.is_none() && !self.events_loading {
            self.request_events();
        }

        // Load the provider list the first time its view is opened.
        if self.view == View::Providers && self.providers.is_none() && !self.providers_loading {
            self.request_providers();
        }

        // The global keys, before anything draws: a shortcut has to be decided
        // before a view gets a chance to read the same keystroke. See
        // `overlays::palette` for the focus rule and the matching order.
        self.handle_shortcuts(ui, now);

        // The shell. Panel order is the whole trick here:
        //
        //   1. `title_drag` BEFORE the title bar, so the bar's buttons -- which
        //      register later -- win the hit test where they overlap it.
        //   2. title bar, status bar, rail, then the view's own panels, then a
        //      `CentralPanel` LAST. egui requires the central panel last, and
        //      the chrome has to be outermost or the views would lay out over it.
        //   3. `resize_strips` AFTER everything, or the panels swallow the
        //      window edges and nothing resizes.
        //
        // See `shell::chrome` for why 1 and 3 are the way round they are.
        egui::CentralPanel::default()
            .frame(shell::chrome::shell_frame(ui, self.decorated))
            .show(ui, |ui| {
                shell::chrome::title_drag(ui, self.decorated);
                shell::titlebar::show(self, ui);
                shell::statusbar::show(self, ui);
                shell::rail::show(self, ui);
                self.ui_view(ui, now);
                shell::chrome::resize_strips(ui, self.decorated);
            });

        // MOF viewer, method-invocation confirmation, and save-query dialog float
        // above the views.
        self.ui_mof_window(ui.ctx());
        self.ui_invoke_modal(ui.ctx());
        self.ui_save_query_window(ui.ctx());
        self.ui_error_log_window(ui.ctx());

        // The palette is last, and a `Modal` rather than a `Window`: it is the
        // frontmost thing in the app and it dims what it covers.
        self.ui_palette(ui, now);

        // Coalesced config write (task 4.7). `push_history` used to serialize and
        // write the whole file on every query run; it now only marks the config
        // dirty, and the write happens here at most once per debounce window.
        // The returned wait keeps the frame loop alive long enough to perform
        // it -- an app that went idle with a pending write would leave the last
        // query out of the history until the next keystroke.
        if let Some(wait) = self.config.poll_save() {
            ui.ctx().request_repaint_after(wait);
        }

        // Repaint while work is in flight, and continuously on the live views.
        if !self.pending.is_empty() {
            ui.ctx().request_repaint_after(Duration::from_millis(30));
        }
        if self.view == View::Network && self.config.live_polling && !self.net_paused {
            ui.ctx().request_repaint_after(Duration::from_millis(200));
        }
        // Keep events flowing in while the monitor runs.
        if self.monitor.is_some() {
            ui.ctx().request_repaint_after(Duration::from_millis(250));
        }
        // Same interval, and for a sharper reason: the process log stamps each
        // event with the frame clock, so a frame that never comes is an event
        // that lands late carrying the wrong time. It also keeps the ended
        // rows' fade moving while the view is open.
        if self.proc_running() {
            ui.ctx().request_repaint_after(Duration::from_millis(250));
        }
    }
}
