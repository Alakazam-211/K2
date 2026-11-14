//! One tool's vendor check on its own thread with its own timeout (speed
//! pass P5): the old-path probe starts all three at once, so the slowest
//! sets the time, and one that hangs can't hide the others. A child of
//! [`super`], so it uses the probe's own helpers.

use super::*;

/// The longest one tool's check may take before its row says it did not
/// answer (its own transport timeouts are shorter; this is the outer cap).
const VENDOR_TIMEOUT: Duration = Duration::from_secs(30);

#[cfg(test)]
pub(super) fn test_vendor_timeout_slot() -> &'static Mutex<Option<Duration>> {
    static SLOT: Mutex<Option<Duration>> = Mutex::new(None);
    &SLOT
}

fn vendor_timeout() -> Duration {
    #[cfg(test)]
    if let Some(t) = *test_vendor_timeout_slot().lock().unwrap_or_else(|p| p.into_inner()) {
        return t;
    }
    VENDOR_TIMEOUT
}

/// One tool's vendor call, running on its own thread from the moment it is
/// started, with its own deadline. The thread holds no lock.
struct VendorCall {
    name: &'static str,
    rx: std::sync::mpsc::Receiver<HarnessUsage>,
    deadline: Instant,
    prev: Option<HarnessUsage>,
    now: DateTime<Utc>,
}

impl VendorCall {
    fn start(
        name: &'static str,
        io: Arc<dyn UsageIo>,
        prev: Option<HarnessUsage>,
        now_wall: SystemTime,
        now: DateTime<Utc>,
    ) -> VendorCall {
        let (tx, rx) = std::sync::mpsc::channel();
        let deadline = Instant::now() + vendor_timeout();
        let theirs = prev.clone();
        let spawned = std::thread::Builder::new().name(format!("usage-probe-{name}")).spawn(move || {
            let prev = theirs.as_ref();
            let row = match name {
                HARNESS_CLAUDE => probe_claude(io.claude_login(), |token| io.claude_get(token), prev, now_wall, now),
                HARNESS_CODEX => probe_codex(io.codex(), prev, now),
                _ => probe_grok(io.grok(), prev, now),
            };
            let _ = tx.send(row);
        });
        if let Err(e) = spawned {
            k2_core::log_debug!("[usage] {name}: could not start the check: {e}");
        }
        VendorCall { name, rx, deadline, prev, now }
    }

    /// The tool's row, or (past its deadline, or if its thread died) the
    /// last reading kept, with an error saying it did not answer.
    fn finish(self) -> HarnessUsage {
        let left = self.deadline.saturating_duration_since(Instant::now());
        match self.rx.recv_timeout(left) {
            Ok(row) => row,
            Err(_) => {
                let why = format!("{} did not answer in time", self.name);
                k2_core::log_debug!("[usage] {why}");
                preserved(self.name, self.prev.as_ref(), prev_status(self.prev.as_ref()), "", self.now, Some(why))
            }
        }
    }
}

/// Probe the tools (all, or `only`) and write the cache. The vendor calls
/// run at once ([`VendorCall`]); everything that reads or writes the
/// database or the cache stays on this thread, in the tools' order.
pub(super) fn probe_and_write_mode(io: Arc<dyn UsageIo>, only: Option<&str>, mode: Mode) -> Result<SubscriptionFile, String> {
    // Belt for any caller that missed the route/loop gate.
    if airgap_on() {
        return Err(k2_core::airgap::TEACHING.to_string());
    }
    probe_hits().fetch_add(1, Ordering::SeqCst);
    let now_wall = SystemTime::now();
    let now = Utc::now();
    let previous = match load_cache() {
        Loaded::Missing => SubscriptionFile {
            harnesses: Vec::new(),
        },
        Loaded::Ready(doc) => doc,
        Loaded::Unreadable(e) => return Err(e),
    };
    let cached = |name: &str| previous.harnesses.iter().find(|h| h.harness == name).cloned();
    // First, in order and quickly: which tools to ask, and what each keeps
    // if its check fails. Then every vendor call at once, each on its own
    // thread with its own timeout (speed pass P5), so the slowest tool sets
    // the time, not the sum, and one that hangs can't hide the others.
    enum Slot {
        Keep(Option<HarnessUsage>),
        Ask {
            measured: DateTime<Utc>,
            gen_before: Option<(String, i64)>,
            answer: VendorCall,
        },
    }
    let mut slots: Vec<(&str, Slot)> = Vec::new();
    for name in PROBED_HARNESSES {
        if only.is_some_and(|o| o != name) {
            slots.push((name, Slot::Keep(cached(name))));
            continue;
        }
        // This token's own call starts here, not at the top of the loop.
        let measured = Utc::now();
        let gen_before = wallet_generation(name);
        let owner = gen_before.as_ref().map(|g| g.0.as_str());
        if let Some(why) = skip_reason(cached(name).as_ref(), owner, name, mode, now) {
            k2_core::log_debug!("[usage] {name}: not probed ({why})");
            slots.push((name, Slot::Keep(cached(name))));
            continue;
        }
        // A failed check keeps THIS token's last reading: the token's own
        // row when the wallet knows which token is live, else (no wallet
        // entry) the cache row, only if it isn't some other token's.
        let prev = match &gen_before {
            Some((owner, _)) => wallet_row(owner).or_else(|| cached(name).filter(|r| r.account_id.as_deref() == Some(owner.as_str()))),
            None => cached(name).filter(|r| r.account_id.is_none()),
        };
        let answer = VendorCall::start(name, io.clone(), prev, now_wall, now);
        slots.push((name, Slot::Ask { measured, gen_before, answer }));
    }
    let mut rows = Vec::new();
    for (name, slot) in slots {
        let (measured, gen_before, answer) = match slot {
            Slot::Keep(row) => {
                rows.extend(row);
                continue;
            }
            Slot::Ask { measured, gen_before, answer } => (measured, gen_before, answer),
        };
        let mut row = answer.finish();
        row.source = "probe".into();
        note_outcome(name, &row, now);
        let gen_after = wallet_generation(name);
        match (&gen_before, &gen_after) {
            (Some(b), Some(a)) if a == b => {
                row.account_id = Some(b.0.clone());
                // Not the default's reading while the live sign-in is known
                // to be another account (LT-A6).
                let other = k2_core::llm_accounts::Tool::parse(name).is_some_and(k2_core::llm_accounts::wallet::live_is_other_account);
                if !other {
                    record_on_wallet(&b.0, &row, measured);
                }
            }
            (Some(_), _) => {
                // A switch ran during the probe: the reading may be either
                // token's. Show nothing as fresh; the switch re-checks.
                k2_core::log_debug!("[usage] {name}: the server default changed during the check; reading dropped");
                row = preserved(name, None, "", &row.plan, now, Some("the server default changed during the check".into()));
            }
            _ => {}
        }
        rows.push(row);
    }
    let mut doc = SubscriptionFile { harnesses: rows };
    {
        // A live feed may have written a newer reading while the probe ran
        // (it held no lock): keep that one.
        let _g = cache_lock();
        if let Loaded::Ready(current) = load_cache() {
            for row in &mut doc.harnesses {
                let Some(cur) = current.harnesses.iter().find(|h| h.harness == row.harness) else { continue };
                let cur_t = DateTime::parse_from_rfc3339(&cur.checked_at).ok();
                let row_t = DateTime::parse_from_rfc3339(&row.checked_at).ok();
                let newer = matches!((cur_t, row_t), (Some(c), Some(r)) if c > r) || (cur_t.is_some() && row_t.is_none());
                if cur.source.starts_with("live") && newer && cur.account_id == row.account_id {
                    *row = cur.clone();
                }
            }
        }
        write_cache(&doc)?;
    }
    crate::session_events::emit_subscription_usage_changed(only, None, "probe");
    Ok(doc)
}
