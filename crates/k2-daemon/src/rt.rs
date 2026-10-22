//! The daemon's Tokio runtime, for work that runs on a plain std thread.
//!
//! A plain `std::thread` has no Tokio context. Anything that wakes an agent
//! (`workspace_msg::deliver_live*` with `wake`, `ensure_canonical_session`)
//! spawns a session, and the spawn needs one: `cell_server::serve_cell`
//! adopts its socket with `tokio::net::UnixListener::from_std` and
//! `tokio::spawn`s the accept loop. Without a runtime that panics ("there is
//! no reactor running", the 0.46.3 r9 permission-turn crash) and the thread
//! dies with its delivery.
//!
//! Off-thread wakes go through [`spawn`] (one-shot work: refused, with a
//! log line, when there is no runtime) or enter [`handle`] themselves
//! (long-lived loops, once per pass).

use std::sync::RwLock;

use tokio::runtime::Handle;

static INSTALLED: RwLock<Option<Handle>> = RwLock::new(None);

/// Record the daemon's runtime: daemon boot, and the test harness for each
/// test runtime (the newest wins).
pub fn install(handle: Handle) {
    *INSTALLED.write().unwrap_or_else(|e| e.into_inner()) = Some(handle);
}

/// The caller's runtime, else the installed one. `None` only before boot
/// (and in unit tests that never start one).
pub fn handle() -> Option<Handle> {
    Handle::try_current()
        .ok()
        .or_else(|| INSTALLED.read().unwrap_or_else(|e| e.into_inner()).clone())
}

/// For a long-lived std-thread loop: the runtime to enter for one pass
/// (read per pass, so a loop follows the newest installed runtime). The
/// lib's unit tests keep their loops off any runtime: their wakes take the
/// test seams that key on "no runtime".
pub fn for_loop_pass() -> Option<Handle> {
    if cfg!(test) {
        None
    } else {
        handle()
    }
}

/// Start a named std thread whose body runs inside the daemon's runtime
/// context. Refused (an `Err`, nothing runs) when there is no runtime: the
/// caller logs and gives up, it never panics.
pub fn spawn<F>(name: &str, f: F) -> std::io::Result<std::thread::JoinHandle<()>>
where
    F: FnOnce() + Send + 'static,
{
    let Some(handle) = handle() else {
        return Err(std::io::Error::other(format!("no Tokio runtime for the {name} thread")));
    };
    std::thread::Builder::new().name(name.to_string()).spawn(move || {
        let _rt = handle.enter();
        f()
    })
}
