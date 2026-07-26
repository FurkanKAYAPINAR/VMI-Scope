//! Request-id allocation and the record of what each in-flight id was for.

use crate::app::VmiScopeApp;

/// What an in-flight request id was asking for — used to clear the correct
/// loading state when a reply (or error) arrives.
pub(crate) enum PendingKind {
    Namespaces(String),
    Classes,
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
