//! Every state mutation that dispatches work to the background worker: the
//! `request_*` helpers plus the selection actions that trigger them.

use std::time::Duration;

use crate::app::{ConnStatus, VmiScopeApp, DEFAULT_NAMESPACE, ROOT_NAMESPACE};
use crate::state::ids::PendingKind;

use vmiscope_core::{Credential, Impersonation, MethodArg, Request};

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

    /// Class/child counts for one namespace node of the tree, fired lazily the
    /// first time a node is shown. Non-recursive: a per-node rollup over the
    /// whole subtree would be minutes of binds for a number the row cannot show.
    /// Deduped against both the cache and the in-flight set.
    pub(crate) fn request_namespace_stats(&mut self, namespace: String) {
        if self.ns_stats.contains_key(&namespace)
            || self.ns_stats_pending.contains(&namespace)
            // A namespace that denied us once will deny us every frame; trying
            // again on each repaint is an error-log loop, not a retry.
            || self.ns_stats_failed.contains(&namespace)
        {
            return;
        }
        let id = self.alloc_id();
        self.ns_stats_pending.insert(namespace.clone());
        self.pending
            .insert(id, PendingKind::NamespaceStats(namespace.clone()));
        self.worker.send(Request::NamespaceStats {
            id,
            namespace,
            recursive: false,
        });
    }

    /// Count one class's instances. Fired only for the selected class or by the
    /// explicit "Count" action -- never for a whole namespace on arrival, which
    /// is the rule task 3.11/3.19 exists to keep: a count is expensive, per-row,
    /// and `CIM_DataFile` never finishes. The core skips abstract/association/
    /// event classes without touching WMI and bounds the rest by a deadline.
    pub(crate) fn request_instance_count(&mut self, class: String) {
        if class.is_empty()
            || self.instance_counts.contains_key(&class)
            || self.counting.contains(&class)
        {
            return;
        }
        let id = self.alloc_id();
        self.counting.insert(class.clone());
        self.pending
            .insert(id, PendingKind::InstanceCount(class.clone()));
        self.worker.send(Request::InstanceCount {
            id,
            namespace: self.active_ns.clone(),
            class,
            // Shallow: the count is of this class's own instances, matching the
            // `SELECT * FROM <class>` the Instances tab shows.
            deep: false,
        });
    }

    /// The relationships a class takes part in, for the Schema sub-tab. Bounded
    /// by `ASSOCIATIONS_BUDGET` in the core. Deduped against the class already
    /// held or being fetched.
    pub(crate) fn request_associations(&mut self, class: String) {
        if class.is_empty() {
            return;
        }
        if self.assoc_class == class && (self.associations.is_some() || self.assoc_loading) {
            return;
        }
        let id = self.alloc_id();
        self.assoc_class = class.clone();
        self.associations = None;
        self.assoc_loading = true;
        self.pending.insert(id, PendingKind::Associations);
        self.worker.send(Request::Associations {
            id,
            namespace: self.active_ns.clone(),
            class,
        });
    }

    /// Fetch a class's MOF for inline display in the Schema sub-tab.
    ///
    /// Deliberately does not raise `mof_open`: the floating MOF window is
    /// superseded by the inline panel (task 3.28), so the same `mof_*` state is
    /// reused but the window stays closed.
    pub(crate) fn request_class_mof_inline(&mut self, class: String) {
        if class.is_empty() {
            return;
        }
        if self.mof_object_path == class && (self.mof_text.is_some() || self.mof_loading) {
            return;
        }
        let id = self.alloc_id();
        self.mof_loading = true;
        self.mof_title = class.clone();
        self.mof_object_path = class.clone();
        self.mof_text = None;
        self.pending.insert(id, PendingKind::Mof);
        self.worker.send(Request::ClassMof {
            id,
            namespace: self.active_ns.clone(),
            object_path: class,
        });
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

    /// Point the worker at `host` under `cred`, at `impersonation`. The reply
    /// (`HostConnected` or a `Connect` error) is what the Machines view records
    /// against the target it was for.
    pub(crate) fn apply_host(
        &mut self,
        host: Option<String>,
        cred: Option<Credential>,
        impersonation: Impersonation,
    ) {
        let id = self.alloc_id();
        self.conn_status = ConnStatus::Connecting;
        self.pending.insert(id, PendingKind::Connect);
        self.worker.send(Request::SetHost {
            id,
            host,
            cred,
            impersonation,
        });
    }

    /// Wipe host-scoped state and re-seed the tree/query for a new target,
    /// opening the Explorer to `namespace` (blank falls back to the default).
    ///
    /// Seeding the namespace from the connected target is what keeps the
    /// Machines view's per-target namespace from being decorative. The one-shot
    /// query is only re-run when opening to the default namespace: firing the OS
    /// query into, say, `root\subscription` -- where `Win32_OperatingSystem` does
    /// not exist -- would raise an error that the connection did not cause.
    pub(crate) fn reset_and_reseed(&mut self, namespace: String) {
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
        self.instance_counts.clear();
        self.counting.clear();
        self.ns_stats.clear();
        self.ns_stats_pending.clear();
        self.ns_stats_failed.clear();
        self.last_ns_stats_ms = None;
        self.associations = None;
        self.assoc_class.clear();
        self.search_index = None;
        self.net_conns.clear();
        self.providers = None;
        self.provider_hosts = None;
        self.events_report = None;
        self.act_instances = None;
        let ns = if namespace.trim().is_empty() {
            DEFAULT_NAMESPACE.to_string()
        } else {
            namespace
        };
        self.active_ns = ns.clone();
        self.ns_expanded.insert(ROOT_NAMESPACE.to_string());
        self.request_namespaces(ROOT_NAMESPACE.to_string());
        self.request_classes(ns.clone());
        if ns == DEFAULT_NAMESPACE {
            self.run_query();
        }
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

    // `request_mof` (which raised `mof_open` to show the floating MOF window)
    // was removed with the Explorer rebuild: the Schema sub-tab loads MOF inline
    // via `request_class_mof_inline`, and nothing else opened the window. The
    // window itself lives in `overlays::mof` (not owned here) and is now dormant.

    pub(crate) fn run_query(&mut self) {
        let wql = self.query_text.trim().to_string();
        if wql.is_empty() {
            return;
        }
        // The namespace goes in with the text: a history entry that only knows
        // the WQL cannot be replayed, because the same query means different
        // things in different namespaces. The run's timings are attached later,
        // by `note_query_run`, when the reply that measured them arrives.
        let namespace = self.active_ns.clone();
        self.config.push_history(&wql, &namespace);
        let id = self.alloc_id();
        self.latest_query_id = id;
        self.query_loading = true;
        // Cleared on dispatch, not on reply: re-running is the user saying they
        // know about the last failure. The reply clears it again anyway.
        self.clear_error();
        self.pending.insert(id, PendingKind::Query);
        self.worker.send(Request::Query {
            id,
            namespace,
            wql,
            // From Settings, not from the constants: those are only the
            // defaults a fresh config starts at.
            max_rows: Some(self.config.row_limit),
            timeout: Some(Duration::from_secs(self.config.operation_timeout_secs)),
            // Identity columns off: `__RELPATH`/`__PATH`/`__CLASS` are noise in
            // a plain result table. The Compare view (task 6.5) is what asks for
            // them, and it is not built yet.
            //
            // NOTE: this field arrived in the core in another agent's in-flight
            // Phase 6 work and left this -- the GUI's only `Request::Query` call
            // site -- uncompilable. Filled in here with the behaviour the table
            // has always had, not designed here.
            include_system: false,
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
        // Instance counts are a fact about a namespace's population, not about a
        // class name, so they cannot survive a namespace switch.
        self.instance_counts.clear();
        self.counting.clear();
        self.associations = None;
        self.assoc_class.clear();
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
        // The detail header ("N properties · M methods · derives from X") needs
        // the schema whatever the active sub-tab is, and it is a single class
        // read; fetch it on selection rather than only when the Schema tab opens.
        self.request_schema(class.clone());
        // The one expensive per-class request, fired only for this selection.
        self.request_instance_count(class);
    }
}
