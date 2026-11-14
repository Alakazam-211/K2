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
/// caller logs and gives up, it never panics. The lib's unit tests still
/// get a plain thread then: their deliveries go to test sinks inside it.
pub fn spawn<F>(name: &str, f: F) -> std::io::Result<std::thread::JoinHandle<()>>
where
    F: FnOnce() + Send + 'static,
{
    let handle = handle();
    if handle.is_none() && !cfg!(test) {
        return Err(std::io::Error::other(format!("no Tokio runtime for the {name} thread")));
    }
    std::thread::Builder::new().name(name.to_string()).spawn(move || {
        let _rt = handle.as_ref().map(|h| h.enter());
        f()
    })
}

/// Run blocking work (a wait on the database lock, a file walk) from an
/// async task without starving the runtime. On the daemon's multi-thread
/// runtime this is `tokio::task::block_in_place`: the worker hands its other
/// tasks (the accept loop, sockets) to another thread first, so a slow
/// handler can't park every worker. On a current-thread runtime (some tests)
/// or with no runtime, `f` just runs, as before (vs-live SP-5).
pub fn blocking<T>(f: impl FnOnce() -> T) -> T {
    match Handle::try_current() {
        Ok(h) if h.runtime_flavor() == tokio::runtime::RuntimeFlavor::MultiThread => tokio::task::block_in_place(f),
        _ => f(),
    }
}

#[cfg(test)]
mod blocking_tests {
    use super::blocking;

    #[test]
    fn without_a_runtime_it_just_runs() {
        assert_eq!(blocking(|| 7), 7);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn on_a_current_thread_runtime_it_just_runs() {
        assert_eq!(blocking(|| 8), 8);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    async fn on_a_multi_thread_runtime_other_tasks_keep_running() {
        let (tx, rx) = std::sync::mpsc::channel();
        // Both tasks share the one worker. Without block_in_place the
        // blocking wait holds that worker and the sender never runs.
        let waiter = tokio::spawn(async move { blocking(|| rx.recv_timeout(std::time::Duration::from_secs(5)).is_ok()) });
        tokio::task::yield_now().await;
        let sender = tokio::spawn(async move {
            tx.send(()).expect("send");
        });
        assert!(waiter.await.expect("join waiter"), "the other task ran while this one blocked");
        sender.await.expect("join sender");
    }
}
