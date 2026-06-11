//! Egress composer — where the Awareness Bus actually delivers.
//!
//! E4 of Phase 3. Takes a signal, resolves liveness, plans delivery
//! via `routing::resolve`, and fires every channel the plan asks
//! for: inbox file, PTY-inject, scheduler wake, bus broadcast,
//! activity_feed row.
//!
//! **Ambient providers.** PTY-inject and scheduler-wake are
//! per-deployment concerns — the daemon knows how to reach live
//! sessions and trigger wake, tests want mocks, the smoke-test
//! example wants in-process calls. This module exposes two
//! provider traits registered at startup via `set_inject_provider`
//! / `set_wake_provider`. With no provider registered, the
//! relevant paths become no-ops but audit (bus + activity_feed)
//! still fires.
//!
//! **Liveness source.** `session::registry` — we walk registered
//! sessions looking for one tagged with the target agent's name.
//! Fast, in-process, no DB round-trip.
//!
//! **activity_feed row.** Always fired (PRD: "always, for audit").
//! Uses `db::shared()` to insert via `ActivityFeedEntry::insert`.
//! `project_id` comes from the signal's `from.workspace` if it
//! matches a registered project row; otherwise falls back to the
//! `_orphan` sentinel (seeded by `db::seed_audit_sentinels` at
//! startup). `AgentAddress::Broadcast` senders use the `_broadcast`
//! sentinel. The original unresolved workspace id is preserved in
//! the row's `to_project_id` column when falling back, so audit
//! consumers can still see what the sender claimed.

use std::path::PathBuf;
use std::sync::OnceLock;

use parking_lot::Mutex;

use crate::awareness::{
    bus, inbox, resolve, routing::DeliveryPlan, AgentAddress, AgentSignal,
    TargetState,
};
use crate::db::schema::ActivityFeedEntry;
use crate::log_debug;
use crate::session::registry;

// ─────────────────────────────────────────────────────────────────────
// Provider traits — host registers implementations at startup
// ─────────────────────────────────────────────────────────────────────

/// Implemented by the host to inject bytes into a live agent's
/// session. Daemon's E7 impl will look up the
/// `SessionStreamSession` by agent_name and call `write(bytes)` on
/// it. Test impls can just collect the calls in a vec.
pub trait InjectProvider: Send + Sync {
    /// Best-effort inject. `Ok` on success; error paths logged by
    /// the caller. If the agent has no live session, return `Err` —
    /// egress degrades to audit-only for that signal.
    fn inject(&self, agent: &str, bytes: &[u8]) -> std::io::Result<()>;

    /// Liveness probe used by [`is_agent_live`]. Default returns
    /// false so existing test impls and pre-A9 hosts compile
    /// unchanged — those callers fall back to the legacy
    /// `session::registry` walk which catches every legacy session.
    /// Hosts whose sessions don't all live in `session::registry`
    /// (the daemon's v2 path) override this to consult their own
    /// session maps; without it, Live signals to v2-only agents
    /// short-circuit through the wake path and the inject
    /// never runs.
    fn is_live(&self, _agent: &str) -> bool {
        false
    }
}

/// Implemented by the host to trigger a scheduler wake for an
/// offline agent. Daemon's E7 impl queues a pending-live-delivery
/// file and fires the existing heartbeat-wake pathway.
pub trait WakeProvider: Send + Sync {
    /// Best-effort wake. `Ok` means "wake request accepted"; the
    /// actual session boot happens asynchronously. Errors on
    /// unknown agent / invalid state.
    fn wake(&self, agent: &str, signal: &AgentSignal) -> std::io::Result<()>;
}

static INJECT_PROVIDER: OnceLock<Mutex<Option<Box<dyn InjectProvider>>>> =
    OnceLock::new();
static WAKE_PROVIDER: OnceLock<Mutex<Option<Box<dyn WakeProvider>>>> =
    OnceLock::new();

fn inject_slot() -> &'static Mutex<Option<Box<dyn InjectProvider>>> {
    INJECT_PROVIDER.get_or_init(|| Mutex::new(None))
}

fn wake_slot() -> &'static Mutex<Option<Box<dyn WakeProvider>>> {
    WAKE_PROVIDER.get_or_init(|| Mutex::new(None))
}

/// Register the host's inject provider. Idempotent — second call
/// overwrites. Test helpers use this to install a mock.
pub fn set_inject_provider(p: Box<dyn InjectProvider>) {
    *inject_slot().lock() = Some(p);
}

/// Register the host's wake provider. Idempotent.
pub fn set_wake_provider(p: Box<dyn WakeProvider>) {
    *wake_slot().lock() = Some(p);
}

/// Test helper — clear provider registrations so tests see a
/// clean slate. Mirror of the `registry::clear_for_tests` pattern.
#[cfg(any(test, feature = "test-util"))]
pub fn clear_providers_for_tests() {
    *inject_slot().lock() = None;
    *wake_slot().lock() = None;
}

// ─────────────────────────────────────────────────────────────────────
// DeliveryReport — what the composer did
// ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default, PartialEq)]
pub struct DeliveryReport {
    /// `Delivery::Live` sender whose bytes reached the target's PTY
    /// directly. False if the target was offline or inject failed.
    pub injected_to_pty: bool,
    /// `Delivery::Live` sender whose target was offline; wake
    /// provider fired successfully.
    pub woke_offline_target: bool,
    /// `Delivery::Inbox` sender whose file landed at this path.
    pub inbox_path: Option<PathBuf>,
    /// Row id of the activity_feed entry written. 0 if no DB was
    /// available (tests without `test-util`) — not an error, just
    /// means audit couldn't persist.
    pub activity_feed_row_id: i64,
    /// Bus publish always fires; marker exists for symmetry and to
    /// make "everything succeeded" assertions easy.
    pub published_to_bus: bool,
    /// G5: `true` if the sender exceeded their per-coordination-level
    /// emit budget for this daemon run. Delivery channels (inject /
    /// wake / inbox / bus) were ALL skipped; only the denial-audit
    /// row was written. Sender agents can inspect this to back off
    /// or slow down.
    pub budget_denied: bool,
}

// ─────────────────────────────────────────────────────────────────────
// deliver — the entry point
// ─────────────────────────────────────────────────────────────────────

/// Deliver `signal`. Returns what actually happened; callers (the
/// `k2so msg` CLI, APC ingress) use the report to decide
/// follow-up (prints, retries, etc.).
///
/// `inbox_root` is the directory under which per-agent inboxes
/// live. Daemon passes `<project>/.k2so/awareness/inbox/` for
/// same-workspace delivery; cross-workspace delivery (Phase 4)
/// rebuilds this per target.
///
/// For `AgentAddress::Broadcast` / `Workspace`, this function
/// handles the "always" channels (bus + activity_feed). Fanout
/// across individual agents is the caller's responsibility — use
/// `roster::query` to enumerate members, then call `deliver_to_agent`
/// for each.
pub fn deliver(signal: &AgentSignal, inbox_root: &std::path::Path) -> DeliveryReport {
    // G5 — budget gate. Every signal runs through here regardless
    // of Agent/Workspace/Broadcast addressing so a chatty sender
    // can't bypass their limit by broadcasting instead of
    // 1-on-1-ing. On Deny: skip every delivery channel, write an
    // audit row tagged `signal:budget-denied`, return a report
    // with `budget_denied=true`.
    if let Some(deny_report) = maybe_deny_over_budget(signal) {
        return deny_report;
    }

    match &signal.to {
        AgentAddress::Agent { name, .. } => {
            deliver_to_agent(signal, name, inbox_root)
        }
        AgentAddress::Workspace { .. } | AgentAddress::Broadcast => {
            // Multi-target paths only run the audit channels here.
            // Caller enumerates and loops.
            let mut report = DeliveryReport::default();
            report.published_to_bus = true;
            bus::publish(signal.clone());
            report.activity_feed_row_id = write_audit(signal, None);
            report
        }
    }
}

/// Check the sender's per-coordination-level budget. Returns
/// `Some(DeliveryReport { budget_denied: true, ... })` if the
/// sender is over budget — caller returns this directly instead
/// of running any delivery channel. Returns `None` if the sender
/// is within budget (the budget counter has been incremented as a
/// side effect).
fn maybe_deny_over_budget(signal: &AgentSignal) -> Option<DeliveryReport> {
    use crate::workspace::launch_profile::{load_coordination_level, CoordinationLevel};
    use crate::awareness::budget::{
        check_and_increment, BudgetCheck, BudgetDecision, CLI_SENDER_NAME,
    };

    let (workspace_id, from_agent) = match &signal.from {
        AgentAddress::Agent { workspace, name } => (workspace.0.as_str(), name.as_str()),
        // Workspace + Broadcast senders have no per-agent budget
        // (no AGENT.md to read). Keep the invariant "every
        // emit audits" but skip budget enforcement. These address
        // forms are only emitted by system-level callers today.
        _ => return None,
    };
    if from_agent == CLI_SENDER_NAME {
        // CLI shortcut — check_and_increment would allow this
        // anyway, but skipping the AGENT.md lookup is free +
        // avoids noise in cli-heavy workflows.
        return None;
    }

    // Resolve the sender's coord level. Falls back to
    // `CoordinationLevel::default()` = Moderate when no AGENT.md
    // exists; `_orphan`-style workspaces (G0) also fall back to
    // Moderate because we can't locate their agents dir.
    let level = resolve_coordination_level(workspace_id, from_agent)
        .unwrap_or_else(CoordinationLevel::default);

    let decision = check_and_increment(&BudgetCheck {
        workspace_id,
        from_agent,
        level,
        is_status: matches!(signal.kind, crate::awareness::SignalKind::Status { .. }),
        is_urgent: matches!(signal.priority, crate::awareness::Priority::Urgent),
    });

    match decision {
        BudgetDecision::Allow => None,
        BudgetDecision::Deny { level, budget, used } => {
            log_debug!(
                "[awareness/egress] budget denied: from={from_agent} \
                 workspace={workspace_id} level={level:?} used={used}/{budget}"
            );
            let mut report = DeliveryReport::default();
            report.budget_denied = true;
            // Audit row with a dedicated event_type so higher-level
            // queries can spot + count denials distinctly from real
            // delivery events. Uses write_audit_event to override
            // the default `signal:<kind>` event_type.
            report.activity_feed_row_id =
                write_audit_event(signal, None, "signal:budget-denied");
            Some(report)
        }
    }
}

/// Resolve the sender agent's `CoordinationLevel`, looking up
/// project path from the workspace id then reading AGENT.md.
/// Returns `None` if the workspace isn't a registered project or
/// AGENT.md is absent — caller substitutes default level.
fn resolve_coordination_level(
    workspace_id: &str,
    agent_name: &str,
) -> Option<crate::workspace::launch_profile::CoordinationLevel> {
    let db = crate::db::shared();
    let conn = db.lock();
    let project_path: String = conn
        .query_row(
            "SELECT path FROM projects WHERE id = ?1",
            rusqlite::params![workspace_id],
            |row| row.get(0),
        )
        .ok()?;
    drop(conn);
    Some(crate::workspace::launch_profile::load_coordination_level(
        &project_path,
        agent_name,
    ))
}

/// Deliver to a specific agent name. Resolves liveness from
/// session::registry, plans via routing::resolve_for_agent, fires
/// each channel the plan prescribes.
///
/// Invariants held:
/// - Inbox writes only when sender picked `Delivery::Inbox`. Never
///   as a fallback for offline targets.
/// - Live+live tries inject. On inject failure (provider unregistered,
///   session mid-shutdown, etc.), the report's `injected_to_pty`
///   stays false but audit still fires — the signal isn't lost, it's
///   recorded in activity_feed for later manual inspection.
/// - Every delivery writes activity_feed. Every delivery publishes
///   to bus.
pub fn deliver_to_agent(
    signal: &AgentSignal,
    target_agent: &str,
    inbox_root: &std::path::Path,
) -> DeliveryReport {
    let state = if is_agent_live(target_agent, signal) {
        TargetState::Live
    } else {
        TargetState::Offline
    };
    let plan = resolve(signal, state);
    execute(signal, target_agent, plan, inbox_root)
}

/// Run a pre-computed `DeliveryPlan`. Exposed publicly for callers
/// that already resolved the plan themselves (e.g. a dry-run UI that
/// shows "if you click send, here's what will happen" before actually
/// clicking).
pub fn execute(
    signal: &AgentSignal,
    target_agent: &str,
    plan: DeliveryPlan,
    inbox_root: &std::path::Path,
) -> DeliveryReport {
    let mut report = DeliveryReport::default();

    if plan.inject_to_pty {
        match try_inject(target_agent, signal) {
            Ok(()) => report.injected_to_pty = true,
            Err(e) => {
                log_debug!(
                    "[awareness/egress] inject to {} (workspace={:?}) failed: {} — audit-only",
                    target_agent,
                    workspace_id_from_signal_to(signal),
                    e
                );
            }
        }
    }

    if plan.wake_target {
        match try_wake(target_agent, signal) {
            Ok(()) => report.woke_offline_target = true,
            Err(e) => {
                log_debug!(
                    "[awareness/egress] wake {} failed: {} — audit-only",
                    target_agent,
                    e
                );
            }
        }
    }

    if plan.write_to_inbox {
        match inbox::write(inbox_root, target_agent, signal) {
            Ok(path) => report.inbox_path = Some(path),
            Err(e) => {
                log_debug!(
                    "[awareness/egress] inbox write for {} failed: {}",
                    target_agent,
                    e
                );
            }
        }
    }

    if plan.publish_to_bus {
        bus::publish(signal.clone());
        report.published_to_bus = true;
    }

    if plan.write_activity_feed {
        report.activity_feed_row_id = write_audit(signal, Some(target_agent));
    }

    report
}

// ─────────────────────────────────────────────────────────────────────
// Internals
// ─────────────────────────────────────────────────────────────────────

/// Extract the project_id from `signal.to` when it identifies a
/// per-agent target (used to disambiguate multi-workspace name
/// collisions like two workspaces both running an agent named
/// `manager`). Returns `None` for `Workspace` / `Broadcast`
/// addresses.
fn workspace_id_from_signal_to(signal: &AgentSignal) -> Option<&str> {
    match &signal.to {
        AgentAddress::Agent { workspace, .. } => Some(workspace.0.as_str()),
        _ => None,
    }
}

fn is_agent_live(agent: &str, signal: &AgentSignal) -> bool {
    // 1. Legacy session::registry — Kessel-T0 / SessionStreamSession.
    let in_registry = registry::list_ids()
        .into_iter()
        .filter_map(|id| registry::lookup(&id).and_then(|e| e.agent_name()))
        .any(|n| n == agent);
    if in_registry {
        return true;
    }
    // 2. Host inject provider's own liveness probe — daemons whose
    // sessions don't all live in `session::registry` (the v2 path)
    // override `InjectProvider::is_live` to consult their additional
    // session maps. Default impl returns false, so this is a strict
    // additive check: false here means no v2 host has claimed it.
    //
    // **0.37.5 keying:** when the signal targets a specific workspace
    // (`signal.to.workspace` set), check the canonical bare
    // `<workspace_id>` key. The agent's name is metadata about the
    // session, not part of its address (post-0.37.0 unification).
    // If the target workspace's session isn't live under that key,
    // return false so the wake provider can spawn it — never fall
    // back to a bare-name lookup, because the bare slot points to
    // whichever workspace happened to register last and would
    // re-introduce the cross-workspace cross-wiring bug.
    //
    // Bare-agent-name lookup is only used when there's no workspace
    // disambiguator in the signal (Workspace/Broadcast addresses, or
    // CLI callers that don't set K2SO_PROJECT_PATH) — those flow
    // through the legacy Kessel path which still uses bare keys.
    let slot = inject_slot().lock();
    if let Some(p) = slot.as_ref() {
        if let Some(workspace_id) = workspace_id_from_signal_to(signal) {
            return p.is_live(workspace_id);
        }
        if p.is_live(agent) {
            return true;
        }
    }
    false
}

fn try_inject(agent: &str, signal: &AgentSignal) -> std::io::Result<()> {
    let slot = inject_slot().lock();
    let provider = slot.as_ref().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::NotConnected,
            "no InjectProvider registered",
        )
    })?;
    let bytes = render_signal_for_inject(signal);

    // **0.37.5 keying:** when the signal targets a specific workspace,
    // inject ONLY into the canonical bare-`<workspace_id>` session.
    // Do NOT fall back to bare-agent-name lookup — that bare slot
    // points to whichever workspace registered last (last-write-wins
    // for back-compat with legacy bare-keyed callers), so falling
    // back would re-introduce the cross-workspace cross-wiring bug.
    // NotFound on the canonical key correctly bubbles up as
    // "session offline" → egress's wake path runs and the signal is
    // queued under the same canonical key for delivery on next spawn.
    //
    // Bare-agent-name inject is reserved for callers without
    // workspace context (Workspace/Broadcast, or legacy Kessel paths).
    if let Some(workspace_id) = workspace_id_from_signal_to(signal) {
        return provider.inject(workspace_id, bytes.as_bytes());
    }
    provider.inject(agent, bytes.as_bytes())
}

fn try_wake(agent: &str, signal: &AgentSignal) -> std::io::Result<()> {
    let slot = wake_slot().lock();
    let provider = slot.as_ref().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::NotConnected,
            "no WakeProvider registered",
        )
    })?;
    provider.wake(agent, signal)
}

/// Format a signal as the body bytes to inject into the target's
/// PTY. Returns just `[from] body` — without a trailing newline or
/// carriage return. The caller (`InjectProvider::inject` impl) must
/// write the submit trigger (`\r`) separately after a brief settle
/// delay, otherwise TUI input widgets treat a body+\r single write
/// as a multi-line paste and the message lands typed-but-not-sent.
/// Future refinements (ANSI formatting, agent-speak control codes)
/// live in per-harness integrations.
fn render_signal_for_inject(signal: &AgentSignal) -> String {
    use crate::awareness::SignalKind;
    let body = match &signal.kind {
        SignalKind::Msg { text } => text.clone(),
        SignalKind::Status { text } => format!("[status] {text}"),
        SignalKind::Presence { state } => {
            format!("[presence {state:?}]")
        }
        SignalKind::Reservation { paths, action } => {
            format!("[{action:?}] {}", paths.join(", "))
        }
        SignalKind::TaskLifecycle { phase, task_ref } => {
            let r = task_ref.as_deref().unwrap_or("");
            format!("[task {phase:?}] {r}")
        }
        SignalKind::Custom { kind, payload } => {
            format!("[{kind}] {payload}")
        }
    };
    let from = match &signal.from {
        AgentAddress::Agent { name, .. } => name.clone(),
        AgentAddress::Workspace { .. } => "workspace".into(),
        AgentAddress::Broadcast => "broadcast".into(),
    };
    format!("[{from}] {body}")
}

fn write_audit(signal: &AgentSignal, target_agent: Option<&str>) -> i64 {
    let event_type = format!("signal:{}", signal_kind_tag(signal));
    write_audit_event(signal, target_agent, &event_type)
}

/// Write an activity_feed row with a CALLER-specified event_type
/// rather than the default `signal:<kind>`. G5's budget-denial
/// path uses this to tag denials with `signal:budget-denied` so
/// higher-level queries can distinguish delivered signals from
/// dropped ones. The full AgentSignal JSON still lands in
/// `metadata`, preserving the "audit is reconstructable" primitive.
fn write_audit_event(
    signal: &AgentSignal,
    target_agent: Option<&str>,
    event_type: &str,
) -> i64 {
    let project_id = match &signal.from {
        AgentAddress::Agent { workspace, .. }
        | AgentAddress::Workspace { workspace } => workspace.0.as_str(),
        AgentAddress::Broadcast => "_broadcast",
    };
    let from_agent = match &signal.from {
        AgentAddress::Agent { name, .. } => Some(name.as_str()),
        _ => None,
    };
    let summary = signal_summary(signal);
    // Store the full AgentSignal JSON in the metadata column so
    // callers can reconstruct the entire message from the audit
    // log — including signal id (for bus/inbox correlation), full
    // body (summary truncates at 80 chars), priority, reply_to,
    // exact timestamp. activity_feed is the PRIMITIVE audit
    // surface; higher-level views (conversation threads, per-agent
    // message history, reply chains) are SQL queries on top.
    let full_signal = serde_json::to_string(signal).ok();

    let db = crate::db::shared();
    let conn = db.lock();
    // Primary attempt: write with the sender-supplied project_id.
    match ActivityFeedEntry::insert(
        &conn,
        project_id,
        target_agent,
        event_type,
        from_agent,
        target_agent,
        None,
        Some(&summary),
        full_signal.as_deref(),
    ) {
        Ok(id) => id,
        Err(e) if is_fk_violation(&e) => {
            // FK miss — the sender's workspace isn't a registered
            // project. Fall back to the `_orphan` sentinel so audit
            // still fires. Primitive promise: activity_feed ALWAYS
            // records every delivered signal; we never silently drop
            // because a caller passed an unregistered project id.
            log_debug!(
                "[awareness/egress] project_id={:?} not registered; \
                 retrying audit insert under _orphan bucket",
                project_id
            );
            match ActivityFeedEntry::insert(
                &conn,
                "_orphan",
                target_agent,
                event_type,
                from_agent,
                target_agent,
                Some(project_id),
                Some(&summary),
                full_signal.as_deref(),
            ) {
                Ok(id) => id,
                Err(e2) => {
                    log_debug!(
                        "[awareness/egress] _orphan fallback insert failed: {}",
                        e2
                    );
                    0
                }
            }
        }
        Err(e) => {
            log_debug!("[awareness/egress] activity_feed insert failed: {}", e);
            0
        }
    }
}

/// True iff `err` represents a SQLite FOREIGN KEY constraint
/// violation (extended code `SQLITE_CONSTRAINT_FOREIGNKEY`, 787).
/// Used to distinguish "project_id doesn't match any projects.id"
/// from other insert failures (disk full, DB locked, etc.) — only
/// the FK case triggers the `_orphan` fallback.
fn is_fk_violation(err: &rusqlite::Error) -> bool {
    matches!(
        err,
        rusqlite::Error::SqliteFailure(e, _)
            if e.extended_code
                == rusqlite::ffi::SQLITE_CONSTRAINT_FOREIGNKEY
    )
}

fn signal_kind_tag(signal: &AgentSignal) -> &'static str {
    use crate::awareness::SignalKind;
    match &signal.kind {
        SignalKind::Msg { .. } => "msg",
        SignalKind::Status { .. } => "status",
        SignalKind::Reservation { .. } => "reservation",
        SignalKind::Presence { .. } => "presence",
        SignalKind::TaskLifecycle { .. } => "task",
        SignalKind::Custom { .. } => "custom",
    }
}

fn signal_summary(signal: &AgentSignal) -> String {
    use crate::awareness::SignalKind;
    let delivery_tag = match signal.delivery {
        crate::awareness::Delivery::Live => "live",
        crate::awareness::Delivery::Inbox => "inbox",
    };
    let body_snippet = match &signal.kind {
        SignalKind::Msg { text } | SignalKind::Status { text } => {
            text.chars().take(80).collect::<String>()
        }
        SignalKind::Presence { state } => format!("{state:?}"),
        SignalKind::Reservation { paths, .. } => paths.join(","),
        SignalKind::TaskLifecycle { phase, .. } => format!("{phase:?}"),
        SignalKind::Custom { kind, .. } => kind.clone(),
    };
    format!("[{delivery_tag}] {body_snippet}")
}
