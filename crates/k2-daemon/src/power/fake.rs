//! A counting [`PowerOs`] for tests. Touches nothing real: it records
//! holds, releases, wake calls and approval prompts. The daemon binary
//! compiles it but only the tests and the lib use it.

#![allow(dead_code)]

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

use chrono::{DateTime, Utc};
use parking_lot::Mutex;

use super::{LidAccess, LidFacts, LidRefusal, PowerOs, PowerSource, WakeSupport, NO_SESSION_PREFIX};

pub struct FakePowerOs {
    support: Mutex<WakeSupport>,
    needs_approval: AtomicBool,
    accept_approval: bool,
    pub source: Mutex<PowerSource>,
    taken: AtomicUsize,
    released: Arc<AtomicUsize>,
    prompts: AtomicUsize,
    /// `Some(t)` = set_wake(t); `None` = clear_wake().
    wakes: Mutex<Vec<Option<DateTime<Utc>>>>,
    /// S6: lid facts, and how a lid hold attempt ends.
    lid: Mutex<LidFacts>,
    lid_refusal: Option<LidRefusal>,
    lid_taken: AtomicUsize,
    lid_released: Arc<AtomicUsize>,
    /// `Some(e)` = `hold_awake` fails with `e`.
    assertion_error: Option<String>,
    power_reads: AtomicUsize,
}

/// The fake assertion: counts its own release.
struct FakeGuard(Arc<AtomicUsize>);

impl Drop for FakeGuard {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

impl FakePowerOs {
    fn with(support: WakeSupport, needs_approval: bool, accept_approval: bool) -> Self {
        Self {
            support: Mutex::new(support),
            needs_approval: AtomicBool::new(needs_approval),
            accept_approval,
            source: Mutex::new(PowerSource { on_ac: Some(true), battery_percent: None }),
            taken: AtomicUsize::new(0),
            released: Arc::new(AtomicUsize::new(0)),
            prompts: AtomicUsize::new(0),
            wakes: Mutex::new(Vec::new()),
            lid: Mutex::new(LidFacts {
                access: if needs_approval {
                    LidAccess::NeedsApproval
                } else {
                    LidAccess::Unavailable("not supported (fake)".into())
                },
                ac_only_unless_allowed: needs_approval,
            }),
            lid_refusal: None,
            lid_taken: AtomicUsize::new(0),
            lid_released: Arc::new(AtomicUsize::new(0)),
            assertion_error: None,
            power_reads: AtomicUsize::new(0),
        }
    }

    fn with_lid(mut self, access: LidAccess, ac_only: bool, refusal: Option<LidRefusal>) -> Self {
        self.lid = Mutex::new(LidFacts { access, ac_only_unless_allowed: ac_only });
        self.lid_refusal = refusal;
        self
    }

    /// S6: a Mac with the helper in (wake can schedule, lid hold works,
    /// AC only unless allowed).
    pub fn mac_with_helper() -> Self {
        Self::with(WakeSupport::Ready, false, true).with_lid(LidAccess::Ready, true, None)
    }

    /// S6: Linux in a desktop session (logind grants the lid lock).
    pub fn linux_session() -> Self {
        Self::with(WakeSupport::Ready, false, true).with_lid(LidAccess::Ready, false, None)
    }

    /// S6: headless Linux (D13): every logind lock is refused.
    pub fn linux_no_session() -> Self {
        let mut f = Self::with(WakeSupport::Ready, false, true).with_lid(
            LidAccess::Ready,
            false,
            Some(LidRefusal { no_session: true, reason: "Access denied".into() }),
        );
        f.assertion_error = Some(format!("{NO_SESSION_PREFIX}: systemd-inhibit refused"));
        f
    }

    /// S6: Windows whose lid action is Sleep (D14).
    pub fn windows_lid_sleeps() -> Self {
        Self::with(WakeSupport::Ready, false, true).with_lid(
            LidAccess::Unavailable(
                "the power plan's lid action is Sleep. To keep running with the lid closed, set \"When I close the lid\" to Do nothing.".into(),
            ),
            false,
            None,
        )
    }

    /// The OS refuses the sleep assertion.
    pub fn assertion_refused(err: &str) -> Self {
        let mut f = Self::with(WakeSupport::Ready, false, true);
        f.assertion_error = Some(err.to_string());
        f
    }

    pub fn set_source(&self, src: PowerSource) {
        *self.source.lock() = src;
    }

    pub fn lid_taken(&self) -> usize {
        self.lid_taken.load(Ordering::SeqCst)
    }

    pub fn lid_released(&self) -> usize {
        self.lid_released.load(Ordering::SeqCst)
    }

    /// How many times the power source was read (Off reads none).
    pub fn power_reads(&self) -> usize {
        self.power_reads.load(Ordering::SeqCst)
    }

    /// Can schedule wakes, no approval needed.
    pub fn ready() -> Self {
        Self::with(WakeSupport::Ready, false, true)
    }

    /// Cannot schedule wakes.
    pub fn unavailable(why: &str) -> Self {
        Self::with(WakeSupport::Unavailable(why.to_string()), false, false)
    }

    /// macOS without the helper: turning wake on shows the dialog, which
    /// the "user" accepts or declines.
    pub fn needs_approval(accept: bool) -> Self {
        Self::with(WakeSupport::Unavailable("wake helper not installed".into()), true, accept)
    }

    pub fn holds_taken(&self) -> usize {
        self.taken.load(Ordering::SeqCst)
    }

    pub fn holds_released(&self) -> usize {
        self.released.load(Ordering::SeqCst)
    }

    pub fn approval_prompts(&self) -> usize {
        self.prompts.load(Ordering::SeqCst)
    }

    pub fn wake_calls(&self) -> Vec<Option<DateTime<Utc>>> {
        self.wakes.lock().clone()
    }
}

impl PowerOs for FakePowerOs {
    fn hold_awake(&self, _reason: &str) -> Result<Box<dyn Send>, String> {
        if let Some(e) = &self.assertion_error {
            return Err(e.clone());
        }
        self.taken.fetch_add(1, Ordering::SeqCst);
        Ok(Box::new(FakeGuard(Arc::clone(&self.released))))
    }

    fn wake_support(&self) -> WakeSupport {
        self.support.lock().clone()
    }

    fn set_wake(&self, at: DateTime<Utc>) -> Result<(), String> {
        self.wakes.lock().push(Some(at));
        Ok(())
    }

    fn clear_wake(&self) -> Result<(), String> {
        self.wakes.lock().push(None);
        Ok(())
    }

    fn power_source(&self) -> PowerSource {
        self.power_reads.fetch_add(1, Ordering::SeqCst);
        *self.source.lock()
    }

    fn wake_needs_approval(&self) -> bool {
        self.needs_approval.load(Ordering::SeqCst)
    }

    /// The fake dialog. Accepting "installs the helper": wake can then
    /// schedule and the lid hold works.
    fn install_wake_helper(&self) -> Result<(), String> {
        self.prompts.fetch_add(1, Ordering::SeqCst);
        if self.accept_approval {
            self.needs_approval.store(false, Ordering::SeqCst);
            *self.support.lock() = WakeSupport::Ready;
            self.lid.lock().access = LidAccess::Ready;
            Ok(())
        } else {
            Err("The admin dialog was declined.".to_string())
        }
    }

    fn lid_facts(&self, _src: PowerSource) -> LidFacts {
        self.lid.lock().clone()
    }

    fn hold_lid_closed(&self, _reason: &str) -> Result<Box<dyn Send>, LidRefusal> {
        if let Some(r) = &self.lid_refusal {
            return Err(r.clone());
        }
        self.lid_taken.fetch_add(1, Ordering::SeqCst);
        Ok(Box::new(FakeGuard(Arc::clone(&self.lid_released))))
    }
}
