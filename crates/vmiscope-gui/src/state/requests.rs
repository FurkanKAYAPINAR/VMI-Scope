//! Every state mutation that dispatches work to the background worker: the
//! `request_*` helpers plus the selection actions that trigger them.

use crate::app::{CentralView, ConnStatus, VmiScopeApp, DEFAULT_NAMESPACE, ROOT_NAMESPACE};
use crate::state::ids::PendingKind;

use vmiscope_core::{Credential, MethodArg, Request};

impl VmiScopeApp {
    // ------------------------------------------------------------------
    // request helpers
    // ------------------------------------------------------------------

    pub(crate) fn request_namespaces(&mut self, namespace: String) {
        if self.ns_loading.contains(&namespace) || self.ns_children.contains_key(&namespace) {
            return;
        }
        let id = self.alloc_id();
        self.ns_loading.insert(namespace.clone());
        self.pending
            .insert(id, PendingKind::Namespaces(namespace.clone()));
        self.worker
            .send(Request::ListChildNamespaces { id, namespace });
    }

    pub(crate) fn request_classes(&mut self, namespace: String) {
        self.classes_ns = namespace.clone();
        // Serve from cache instantly when we've already enumerated this namespace.
        if let Some(cached) = self.class_cache.get(&namespace) {
            self.classes = cached.clone();
            self.classes_loading = false;
            return;
        }
        let id = self.alloc_id();
        self.classes_loading = true;
        self.classes.clear();
        self.pending.insert(id, PendingKind::Classes);
        self.worker.send(Request::ListClasses { id, namespace });
    }

    pub(crate) fn request_network(&mut self, now: f64) {
        let id = self.alloc_id();
        self.net_inflight = true;
        self.net_last_refresh = now;
        self.pending.insert(id, PendingKind::Network);
        self.worker.send(Request::NetworkSnapshot { id });
    }

    pub(crate) fn request_events(&mut self) {
        let id = self.alloc_id();
        self.events_loading = true;
        self.pending.insert(id, PendingKind::Events);
        self.worker.send(Request::ListEventSubscriptions { id });
    }

    pub(crate) fn request_providers(&mut self) {
        let id = self.alloc_id();
        self.providers_loading = true;
        self.pending.insert(id, PendingKind::Providers);
        self.worker.send(Request::ListProviders { id });
    }

    pub(crate) fn request_schema(&mut self, class: String) {
        if class.is_empty() {
            return;
        }
        // Already have (or are fetching) this class's schema.
        if self.schema_class == class && (self.schema.is_some() || self.schema_loading) {
            return;
        }
        let id = self.alloc_id();
        self.schema_class = class.clone();
        self.schema = None;
        self.schema_loading = true;
        self.pending.insert(id, PendingKind::Schema);
        self.worker.send(Request::ClassSchema {
            id,
            namespace: self.active_ns.clone(),
            class,
        });
    }

    pub(crate) fn request_search_index(&mut self, include_methods: bool) {
        let id = self.alloc_id();
        self.search_loading = true;
        self.pending.insert(id, PendingKind::Search);
        self.worker.send(Request::BuildSearchIndex {
            id,
            namespace: self.active_ns.clone(),
            include_methods,
        });
    }

    pub(crate) fn apply_host(&mut self, host: Option<String>, cred: Option<Credential>) {
        let id = self.alloc_id();
        self.conn_status = ConnStatus::Connecting;
        self.pending.insert(id, PendingKind::Connect);
        self.worker.send(Request::SetHost { id, host, cred });
    }

    /// Wipe host-scoped state and re-seed the tree/query for a new target.
    pub(crate) fn reset_and_reseed(&mut self) {
        self.ns_children.clear();
        self.ns_expanded.clear();
        self.ns_loading.clear();
        self.class_cache.clear();
        self.classes.clear();
        self.classes_ns.clear();
        self.selected_class = None;
        self.result = None;
        self.selected_row = None;
        self.schema = None;
        self.schema_class.clear();
        self.search_index = None;
        self.net_conns.clear();
        self.providers = None;
        self.events_report = None;
        self.act_instances = None;
        self.active_ns = DEFAULT_NAMESPACE.to_string();
        self.ns_expanded.insert(ROOT_NAMESPACE.to_string());
        self.request_namespaces(ROOT_NAMESPACE.to_string());
        self.request_classes(DEFAULT_NAMESPACE.to_string());
        self.run_query();
    }

    pub(crate) fn request_instances(&mut self, class: String) {
        let id = self.alloc_id();
        self.act_instances_loading = true;
        self.pending.insert(id, PendingKind::Instances);
        self.worker.send(Request::ListInstances {
            id,
            namespace: self.active_ns.clone(),
            class,
        });
    }

    pub(crate) fn request_invoke(
        &mut self,
        class: String,
        object_path: String,
        method: String,
        is_static: bool,
        args: Vec<MethodArg>,
    ) {
        // Audit every mutating call.
        let args_str = args
            .iter()
            .map(|a| format!("{}={}", a.name, a.value))
            .collect::<Vec<_>>()
            .join(", ");
        let target = if is_static {
            "(static)".to_string()
        } else {
            object_path.clone()
        };
        crate::config::append_audit(&format!(
            "INVOKE {}\\{class}.{method}  target={target}  args=[{args_str}]",
            self.active_ns
        ));
        let id = self.alloc_id();
        self.act_invoking = true;
        self.act_outcome = None;
        self.pending.insert(id, PendingKind::Invoke);
        self.worker.send(Request::InvokeMethod {
            id,
            namespace: self.active_ns.clone(),
            class,
            object_path,
            method,
            is_static,
            args,
        });
    }

    pub(crate) fn request_mof(&mut self, object_path: String, title: String) {
        let id = self.alloc_id();
        self.mof_open = true;
        self.mof_loading = true;
        self.mof_title = title;
        self.mof_object_path = object_path.clone();
        self.mof_text = None;
        self.pending.insert(id, PendingKind::Mof);
        self.worker.send(Request::ClassMof {
            id,
            namespace: self.active_ns.clone(),
            object_path,
        });
    }

    pub(crate) fn run_query(&mut self) {
        let wql = self.query_text.trim().to_string();
        if wql.is_empty() {
            return;
        }
        self.config.push_history(&wql);
        let id = self.alloc_id();
        self.latest_query_id = id;
        self.query_loading = true;
        self.error = None;
        self.pending.insert(id, PendingKind::Query);
        self.worker.send(Request::Query {
            id,
            namespace: self.active_ns.clone(),
            wql,
        });
    }

    // ------------------------------------------------------------------
    // selection actions
    // ------------------------------------------------------------------

    pub(crate) fn select_namespace(&mut self, namespace: String) {
        if self.active_ns == namespace {
            return;
        }
        self.active_ns = namespace.clone();
        self.selected_class = None;
        self.schema = None;
        self.schema_class.clear();
        self.request_classes(namespace);
    }

    pub(crate) fn toggle_namespace(&mut self, path: &str) {
        if self.ns_expanded.contains(path) {
            self.ns_expanded.remove(path);
        } else {
            self.ns_expanded.insert(path.to_string());
            self.request_namespaces(path.to_string());
        }
    }

    pub(crate) fn select_class(&mut self, class: String) {
        self.query_text = format!("SELECT * FROM {class}");
        self.selected_class = Some(class.clone());
        self.run_query();
        if self.central_view == CentralView::Schema {
            self.request_schema(class);
        }
    }
}
