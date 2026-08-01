//! Draining the background worker: one pass per frame turns replies into state
//! and clears the spinner the request was holding.

use crate::app::{ConnStatus, TrackedConn, VmiScopeApp};
use crate::state::ids::PendingKind;
use crate::views::network::NET_FADE_SECS;

use vmiscope_core::Response;

impl VmiScopeApp {
    // ------------------------------------------------------------------
    // response handling
    // ------------------------------------------------------------------

    pub(crate) fn handle_responses(&mut self, now: f64) {
        for resp in self.worker.poll() {
            match resp {
                Response::ChildNamespaces {
                    id,
                    namespace,
                    children,
                    ..
                } => {
                    self.pending.remove(&id);
                    self.ns_loading.remove(&namespace);
                    self.ns_children.insert(namespace, children);
                }
                Response::Classes {
                    id,
                    namespace,
                    classes,
                    ..
                } => {
                    self.pending.remove(&id);
                    // Only apply if it still matches the namespace we care about.
                    if namespace == self.classes_ns {
                        self.classes = classes.clone();
                        self.classes_loading = false;
                    }
                    self.class_cache.insert(namespace, classes);
                }
                Response::QueryResult {
                    id, wql, result, ..
                } => {
                    self.pending.remove(&id);
                    // Ignore stale results from superseded queries.
                    if id == self.latest_query_id {
                        self.result = Some(result);
                        self.result_wql = wql;
                        self.selected_row = None;
                        self.result_sort = None;
                        self.query_loading = false;
                        self.error = None;
                    }
                }
                Response::Network { id, snapshot, .. } => {
                    self.pending.remove(&id);
                    self.net_inflight = false;
                    // Mark everything stale, then revive whatever is present now.
                    for tc in self.net_conns.values_mut() {
                        tc.alive = false;
                    }
                    for conn in snapshot.connections {
                        let key = conn.key();
                        match self.net_conns.get_mut(&key) {
                            Some(tc) => {
                                tc.conn = conn;
                                tc.last_seen = now;
                                tc.alive = true;
                            }
                            None => {
                                self.net_conns.insert(
                                    key,
                                    TrackedConn {
                                        conn,
                                        last_seen: now,
                                        alive: true,
                                    },
                                );
                            }
                        }
                    }
                    // Drop connections that have fully faded out.
                    self.net_conns
                        .retain(|_, tc| tc.alive || (now - tc.last_seen) < NET_FADE_SECS);
                }
                Response::EventSubscriptions { id, report, .. } => {
                    self.pending.remove(&id);
                    self.events_loading = false;
                    self.events_report = Some(report);
                }
                Response::Providers { id, providers, .. } => {
                    self.pending.remove(&id);
                    self.providers_loading = false;
                    self.providers = Some(providers);
                }
                Response::Schema {
                    id, class, schema, ..
                } => {
                    self.pending.remove(&id);
                    if class == self.schema_class {
                        self.schema = Some(schema);
                        self.schema_loading = false;
                    }
                }
                Response::Mof {
                    id,
                    object_path,
                    mof,
                } => {
                    self.pending.remove(&id);
                    if object_path == self.mof_object_path {
                        self.mof_text = Some(mof);
                        self.mof_loading = false;
                    }
                }
                Response::Instances { id, targets, .. } => {
                    self.pending.remove(&id);
                    self.act_instances_loading = false;
                    self.act_instances = Some(targets);
                }
                Response::MethodDone {
                    id,
                    method,
                    outcome,
                    ..
                } => {
                    self.pending.remove(&id);
                    self.act_invoking = false;
                    self.act_outcome = Some((method, outcome));
                }
                Response::SearchIndex { id, index, .. } => {
                    self.pending.remove(&id);
                    self.search_loading = false;
                    self.search_index = Some(index);
                }
                Response::HostConnected { id, host } => {
                    self.pending.remove(&id);
                    self.conn_status = match &host {
                        Some(h) => ConnStatus::Remote(h.clone()),
                        None => ConnStatus::Local,
                    };
                    self.reset_and_reseed();
                }
                // Phase 3's Explorer consumes these; until its rebuild lands
                // they are acknowledged so the request is cleared, rather than
                // left pending -- an unhandled reply reads as a hung spinner.
                Response::NamespaceStats { id, .. }
                | Response::InstanceCount { id, .. }
                | Response::Associations { id, .. } => {
                    self.pending.remove(&id);
                }
                Response::Error {
                    id,
                    context,
                    message,
                } => {
                    let kind = self.pending.remove(&id);
                    match kind {
                        Some(PendingKind::Namespaces(ns)) => {
                            self.ns_loading.remove(&ns);
                        }
                        Some(PendingKind::Classes) => {
                            self.classes_loading = false;
                        }
                        Some(PendingKind::Query) => {
                            if id == self.latest_query_id {
                                self.query_loading = false;
                            }
                        }
                        Some(PendingKind::Network) => {
                            self.net_inflight = false;
                        }
                        Some(PendingKind::Events) => {
                            self.events_loading = false;
                        }
                        Some(PendingKind::Providers) => {
                            self.providers_loading = false;
                        }
                        Some(PendingKind::Schema) => {
                            self.schema_loading = false;
                        }
                        Some(PendingKind::Mof) => {
                            self.mof_loading = false;
                        }
                        Some(PendingKind::Instances) => {
                            self.act_instances_loading = false;
                        }
                        Some(PendingKind::Invoke) => {
                            self.act_invoking = false;
                        }
                        Some(PendingKind::Search) => {
                            self.search_loading = false;
                        }
                        Some(PendingKind::Connect) => {
                            self.conn_status = ConnStatus::Failed(message.clone());
                        }
                        None => {}
                    }
                    self.push_error(format!("{context}\n{message}"));
                }
            }
        }
    }
}
