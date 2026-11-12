//! Expiry: a waiting request past its `answer_by` turns `expired`, its card
//! says so, waiters wake and the agent hears once. The daemon's own loop
//! (headless-safe, never a client timer); every read of a request also
//! sweeps first, so a status never shows a stale `waiting`.

/// How often the loop runs.
const EVERY_SECS: u64 = 15;

/// One sweep: returns how many requests expired.
pub(crate) fn sweep(now: i64) -> usize {
    let gone = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        match k2_core::approve_requests::expire_due(&conn, now) {
            Ok(g) => g,
            Err(e) => {
                k2_core::log_debug!("[approve] expiry sweep: {e}");
                return 0;
            }
        }
    };
    if gone.is_empty() {
        return 0;
    }
    for r in &gone {
        super::card::close(r);
        super::card::tell_agent(r);
    }
    super::wait::notify();
    gone.len()
}

/// Start the loop once per process.
pub(crate) fn spawn() {
    static STARTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if STARTED.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return;
    }
    let r = std::thread::Builder::new().name("approval-expiry".into()).spawn(|| loop {
        let rt = crate::rt::for_loop_pass();
        let entered = rt.as_ref().map(|h| h.enter());
        sweep(k2_core::overlay::now_secs());
        drop(entered);
        std::thread::sleep(std::time::Duration::from_secs(EVERY_SECS));
    });
    if let Err(e) = r {
        k2_core::log_debug!("[approve] could not start the expiry sweep: {e}");
    }
}
