//! One COM thread per host.
//!
//! A [`crate::worker::WmiWorker`] is a single thread that owns a COM apartment
//! and one target. Talking to two machines therefore means two threads, not one
//! thread that keeps changing its mind: a `SetHost` is a *flush* — it drops
//! every cached connection and every cached class kind — so alternating two
//! hosts through one worker pays a reconnect per switch and serialises work
//! that has no reason to be serial.
//!
//! The registry is deliberately thin. It owns the map from [`HostRef`] to
//! worker and nothing else; the workers keep answering with their own
//! host-stamped [`Response`]s, so a reply is interpretable even if the caller
//! loses track of which handle it came from.

use std::collections::HashMap;

use crate::host::{HostRef, Impersonation};
use crate::remote::Credential;
use crate::worker::{Request, Response, WmiWorker};

/// A set of live per-host workers, keyed by target identity.
#[derive(Default)]
pub struct WorkerRegistry {
    workers: HashMap<HostRef, WmiWorker>,
}

impl WorkerRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Open (or reuse) the worker for `target` and ask it to connect.
    ///
    /// Returns `true` when a thread was spawned, `false` when one was already
    /// live. Either way a [`Request::SetHost`] is sent, so a
    /// [`Response::HostConnected`] — with real `connect_ms`/`probe_ms` and the
    /// host's [`crate::host::HostInfo`] — always follows, and a caller that
    /// pressed "Connect" twice gets an answer twice rather than a spinner that
    /// never clears.
    ///
    /// The credential is passed straight through and never stored here: the
    /// registry keeps identities, the worker thread keeps the secret.
    pub fn open(&mut self, id: u64, target: &HostRef, cred: Option<Credential>) -> bool {
        self.open_with(id, target, cred, Impersonation::default())
    }

    /// [`WorkerRegistry::open`] with an explicit impersonation level.
    pub fn open_with(
        &mut self,
        id: u64,
        target: &HostRef,
        cred: Option<Credential>,
        impersonation: Impersonation,
    ) -> bool {
        let spawned = !self.workers.contains_key(target);
        let worker = self
            .workers
            .entry(target.clone())
            .or_insert_with(WmiWorker::spawn);
        worker.send(Request::SetHost {
            id,
            host: target.host().map(str::to_string),
            cred,
            impersonation,
        });
        spawned
    }

    /// Queue `req` on `target`'s worker.
    ///
    /// `false` means there is no worker for that target — the caller asked a
    /// machine it never opened. Silently spawning one here would connect
    /// without credentials, which is precisely the "ran as the wrong principal"
    /// failure this phase exists to remove, so it is refused instead.
    #[must_use]
    pub fn send(&self, target: &HostRef, req: Request) -> bool {
        match self.workers.get(target) {
            Some(w) => {
                w.send(req);
                true
            }
            None => false,
        }
    }

    /// Cancel request `id` on `target`.
    pub fn cancel(&self, target: &HostRef, id: u64) {
        if let Some(w) = self.workers.get(target) {
            w.cancel(id);
        }
    }

    /// Drain every worker's replies, each tagged with the target that produced
    /// it.
    ///
    /// The tag is the `HostRef`, not the `host` string on the response: those
    /// differ for the case that matters, since `\\SRV1` reached as the current
    /// user and `\\SRV1` reached as `CORP\admin` are two targets with one host
    /// name and two different views of the machine.
    pub fn poll(&self) -> Vec<(HostRef, Response)> {
        let mut out = Vec::new();
        for (target, worker) in &self.workers {
            out.extend(worker.poll().into_iter().map(|r| (target.clone(), r)));
        }
        out
    }

    /// Shut down and join `target`'s worker. `false` if it was not open.
    pub fn close(&mut self, target: &HostRef) -> bool {
        // `WmiWorker::drop` raises the shutdown flag before joining, so this
        // returns promptly even if the thread is inside a runaway enumeration.
        self.workers.remove(target).is_some()
    }

    /// Is there a live worker for `target`?
    pub fn is_open(&self, target: &HostRef) -> bool {
        self.workers.contains_key(target)
    }

    /// Every open target.
    pub fn targets(&self) -> impl Iterator<Item = &HostRef> {
        self.workers.keys()
    }

    /// How many COM threads are live.
    pub fn len(&self) -> usize {
        self.workers.len()
    }

    pub fn is_empty(&self) -> bool {
        self.workers.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Sending to a target that was never opened must fail loudly rather than
    /// spawn an unconfigured worker — an unconfigured worker is a worker
    /// pointed at the local machine as the current user, which is the wrong
    /// answer given confidently.
    #[test]
    fn a_request_to_an_unopened_target_is_refused() {
        let reg = WorkerRegistry::new();
        let sent = reg.send(
            &HostRef::Sso {
                host: "NEVER_OPENED".into(),
            },
            Request::NetworkSnapshot { id: 1 },
        );
        assert!(!sent);
        assert!(reg.is_empty());
    }

    #[test]
    fn closing_an_unopened_target_is_harmless() {
        let mut reg = WorkerRegistry::new();
        assert!(!reg.close(&HostRef::Local));
        assert_eq!(reg.len(), 0);
    }
}
