//! File dialogs and disk writes, off the frame loop.
//!
//! Everything in this module used to run inline in a view. Two of them are
//! genuinely slow and one of them is unbounded:
//!
//! * `rfd::FileDialog::save_file()` / `pick_file()` **do not return until the
//!   user has finished with the dialog**. On the UI thread that is not a stall
//!   measured in milliseconds -- it is the entire application frozen, live
//!   pollers included, for as long as somebody browses their filesystem. A
//!   Network view that stops updating and a process monitor that stops
//!   collecting are the two things this tool exists to do.
//! * `Config::save` serialises the whole config and writes it. Small on a local
//!   disk; `%APPDATA%` on a roaming or redirected profile is a network path.
//!
//! # Shape
//!
//! One worker thread, one job queue, one result channel drained once a frame by
//! `VmiScopeApp::drain_io`. Jobs are serialised deliberately: two file dialogs
//! at once is not a state anyone wants, so a second Export waits behind the
//! first rather than racing it.
//!
//! The service is process-wide (`OnceLock`) rather than owned by the app. That
//! is a deliberate exception to how the rest of this crate is wired, and it
//! buys one thing worth having: [`crate::util::save_file`] keeps its two-argument
//! signature, so all fifteen of its call sites -- including the ones in views
//! this module cannot see -- become non-blocking without being touched.
//!
//! # The trade-off, stated
//!
//! A dialog opened from a worker thread is **not owned by our window**. It is a
//! separate top-level window: it can be moved behind the main one, and the main
//! one stays interactive underneath it. That is the intended behaviour -- the
//! app continues to render and poll -- but it is a real change from a modal
//! dialog, and it is the reason a queued second request is made to wait rather
//! than opening a second dialog beside the first.

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Mutex, OnceLock};

/// What a pick was for, so the reply can be routed back to the view that asked.
///
/// A tag rather than a callback: a callback would have to own a `&mut
/// VmiScopeApp` across a thread boundary, which is exactly the thing this module
/// exists to avoid.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum PickFor {
    /// Persistence: a subscription snapshot to diff against.
    PersistenceBaseline,
    /// Providers: a provider snapshot to diff against.
    ProvidersBaseline,
    /// Saved: a query library to merge in.
    SavedLibrary,
}

/// A unit of work for the IO thread.
enum Job {
    SaveAs {
        default_name: String,
        contents: String,
    },
    Pick {
        what: PickFor,
        filter_name: &'static str,
        extensions: &'static [&'static str],
    },
    Write {
        path: PathBuf,
        contents: String,
    },
}

/// What the IO thread has to report.
pub(crate) enum Note {
    /// A save completed. The path is for the status line; there is nothing to do.
    Saved(PathBuf),
    /// A pick completed and the file was read.
    Picked { what: PickFor, text: String },
    /// Anything that went wrong, already phrased for the error log.
    ///
    /// A cancelled dialog is **not** one of these: cancelling is an answer, not
    /// a failure, and it produces no note at all.
    Failed(String),
}

struct Service {
    jobs: Sender<Job>,
    /// `Mutex` only because `Receiver` is not `Sync` and this lives in a
    /// `static`. It is uncontended: `drain` is called from the UI thread alone.
    notes: Mutex<Receiver<Note>>,
}

static SERVICE: OnceLock<Service> = OnceLock::new();

/// Start the worker on first use, so a session that never touches a file never
/// pays for a thread.
fn service() -> &'static Service {
    SERVICE.get_or_init(|| {
        let (jobs, job_rx) = mpsc::channel::<Job>();
        let (note_tx, notes) = mpsc::channel::<Note>();
        // Detached: it lives as long as the process and holds nothing that must
        // be flushed. A pending write is already on the queue by the time the
        // last frame runs, and the OS reaps the thread at exit.
        std::thread::Builder::new()
            .name("vmiscope-io".into())
            .spawn(move || run(&job_rx, &note_tx))
            .expect("the IO thread could not be started");
        Service {
            jobs,
            notes: Mutex::new(notes),
        }
    })
}

fn run(jobs: &Receiver<Job>, notes: &Sender<Note>) {
    while let Ok(job) = jobs.recv() {
        let note = match job {
            Job::SaveAs {
                default_name,
                contents,
            } => rfd::FileDialog::new()
                .set_file_name(&default_name)
                .save_file()
                .map(|path| match std::fs::write(&path, contents) {
                    Ok(()) => Note::Saved(path),
                    // This used to be `let _ = fs::write(..)`: a full disk, a
                    // read-only share or a denied path wrote nothing and said
                    // nothing, and the user was left holding a file that does
                    // not exist.
                    Err(e) => Note::Failed(format!("Save {}: {e}", path.display())),
                }),
            Job::Pick {
                what,
                filter_name,
                extensions,
            } => rfd::FileDialog::new()
                .add_filter(filter_name, extensions)
                .pick_file()
                .map(|path| match std::fs::read_to_string(&path) {
                    Ok(text) => Note::Picked { what, text },
                    Err(e) => Note::Failed(format!("Open {}: {e}", path.display())),
                }),
            Job::Write { path, contents } => {
                let dir = path.parent().map(std::fs::create_dir_all).transpose();
                Some(match dir.and_then(|_| std::fs::write(&path, contents)) {
                    Ok(()) => Note::Saved(path),
                    Err(e) => Note::Failed(format!("Write {}: {e}", path.display())),
                })
            }
        };
        // `None` is a cancelled dialog. Nothing happened, so nothing is said.
        if let Some(note) = note {
            if notes.send(note).is_err() {
                return; // the app is gone
            }
        }
    }
}

/// Ask for a "Save as" dialog and write `contents` to whatever it returns.
///
/// Returns immediately. Failures arrive as [`Note::Failed`].
pub(crate) fn save_as(default_name: &str, contents: &str) {
    let _ = service().jobs.send(Job::SaveAs {
        default_name: default_name.to_owned(),
        contents: contents.to_owned(),
    });
}

/// Ask for an "Open" dialog filtered to `extensions`, and read what it returns.
pub(crate) fn pick(what: PickFor, filter_name: &'static str, extensions: &'static [&'static str]) {
    let _ = service().jobs.send(Job::Pick {
        what,
        filter_name,
        extensions,
    });
}

/// Write a file with no dialog, creating the parent directory.
pub(crate) fn write(path: PathBuf, contents: String) {
    let _ = service().jobs.send(Job::Write { path, contents });
}

/// Everything the IO thread has finished since the last call. Never blocks.
pub(crate) fn drain() -> Vec<Note> {
    // `get` rather than `service()`: draining before anything has been queued
    // must not start the thread.
    let Some(service) = SERVICE.get() else {
        return Vec::new();
    };
    let Ok(notes) = service.notes.lock() else {
        return Vec::new();
    };
    notes.try_iter().collect()
}
