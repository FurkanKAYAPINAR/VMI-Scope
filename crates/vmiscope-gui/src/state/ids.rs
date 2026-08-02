//! Request-id allocation and the record of what each in-flight id was for.

use crate::app::VmiScopeApp;

/// What an in-flight request id was asking for — used to clear the correct
/// loading state when a reply (or error) arrives.
pub(crate) enum PendingKind {
    Namespaces(String),
    Classes,
    /// Per-namespace class-count rollup for the tree; carries the namespace so
    /// an error clears the right node's pending flag.
    NamespaceStats(String),
    /// Per-class instance count; carries the class name for the same reason.
    InstanceCount(String),
    /// Association lookup for the selected class.
    Associations,
    Query,
    Network,
    Events,
    Providers,
    Schema,
    Mof,
    Instances,
    Invoke,
    Search,
    Connect,
}

impl VmiScopeApp {
    pub(crate) fn alloc_id(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }
}
