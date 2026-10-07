//! Lead + roster → display and reason (DA21), then decay (DA29, §6.2).

use super::row::{ChildKind, ChildState, Display, LeadState, Reason, Row, STALE_AFTER_MS};

/// DA21, first match wins:
/// 1. lead waiting → waiting;
/// 2. any child waiting → waiting;
/// 3. lead working → working;
/// 4. any subagent running or owed → working;
/// 5. any shell, monitor, cron, unknown, or owed shell → monitoring;
/// 6. else idle.
pub fn raw_fold(row: &Row) -> (Display, Reason) {
    if !row.confirmed {
        return (Display::Idle, Reason::Unconfirmed);
    }
    if row.lead.state == LeadState::Waiting {
        let question = row.lead.waiting.as_ref().is_some_and(|w| w.question);
        let reason = if question { Reason::WaitingQuestion } else { Reason::WaitingPermission };
        return (Display::Waiting, reason);
    }
    if row.children.values().any(|c| c.state == ChildState::Waiting) {
        return (Display::Waiting, Reason::WaitingPermission);
    }
    if row.lead.state == LeadState::Working {
        let reason = if row.in_flight.is_empty() { Reason::TurnRunning } else { Reason::ToolRunning };
        return (Display::Working, reason);
    }
    let running = |kind: ChildKind| {
        row.children
            .values()
            .any(|c| c.kind == kind && c.state == ChildState::Running)
    };
    let owed = |subagent: bool| {
        row.children
            .values()
            .any(|c| c.state == ChildState::Owed && (c.kind == ChildKind::Subagent) == subagent)
    };
    if running(ChildKind::Subagent) {
        return (Display::Working, Reason::SubagentsRunning);
    }
    if owed(true) {
        return (Display::Working, Reason::OwedNotification);
    }
    if running(ChildKind::Shell) || running(ChildKind::Unknown) {
        return (Display::Monitoring, Reason::BackgroundShell);
    }
    if running(ChildKind::Monitor) {
        return (Display::Monitoring, Reason::MonitorRunning);
    }
    if owed(false) {
        return (Display::Monitoring, Reason::OwedNotification);
    }
    if running(ChildKind::Cron) {
        return (Display::Monitoring, Reason::CronsScheduled);
    }
    (Display::Idle, row.end_reason.unwrap_or(Reason::TurnDone))
}

/// The display before decay.
pub fn raw_display(row: &Row) -> Display {
    raw_fold(row).0
}

/// DA29: a live display whose evidence is older than [`STALE_AFTER_MS`]
/// shows `unverifiable` (the daemon's liveness sweep turns a dead PTY into
/// `idle` / `pty_exited` itself). New evidence re-folds at once.
pub fn decay(display: Display, reason: Reason, evidence_at: Option<i64>, now: i64) -> (Display, Reason, Option<i64>) {
    match evidence_at {
        Some(at) if display.is_live() && now - at >= STALE_AFTER_MS => {
            (Display::Unverifiable, Reason::StaleNoEvidence, Some(at + STALE_AFTER_MS))
        }
        _ => (display, reason, None),
    }
}

/// Re-fold the row in place.
pub(crate) fn refold(row: &mut Row, now: i64) {
    let (display, reason) = raw_fold(row);
    let (display, reason, stale_since) = decay(display, reason, row.evidence_at, now);
    row.display = display;
    row.reason = reason;
    row.stale_since = stale_since;
}

/// Keep awake (Q10, A32): a row holds the Mac awake while it is working,
/// waiting or monitoring, except when the only thing left is a cron; an
/// `unverifiable` row keeps holding until `hold_stale_ms` after its last
/// evidence (2 h in `power::keep_awake`).
pub fn holds_awake(row: &Row, now: i64, hold_stale_ms: i64) -> bool {
    match row.display {
        Display::Working | Display::Waiting => true,
        Display::Monitoring => row.reason != Reason::CronsScheduled,
        Display::Unverifiable => {
            raw_fold(row).1 != Reason::CronsScheduled
                && row.evidence_at.is_some_and(|at| now - at < hold_stale_ms)
        }
        Display::Idle => false,
    }
}

#[cfg(test)]
mod tests {
    use super::super::row::{Child, ChildOrigin, Outcome, RowFacts, Waiting as LeadWait};
    use super::*;

    fn row(lead: LeadState, children: &[(ChildKind, ChildState)]) -> Row {
        let mut r = Row::new(RowFacts::default(), 0);
        r.confirmed = true;
        r.evidence_at = Some(0);
        r.lead.state = lead;
        if lead == LeadState::Waiting {
            r.lead.waiting = Some(LeadWait { question: false, waiting_for: None, tool_name: None });
        }
        for (i, (kind, state)) in children.iter().enumerate() {
            r.children.insert(
                format!("c{i}"),
                Child {
                    kind: *kind,
                    state: *state,
                    origin: ChildOrigin::Hook,
                    since: 0,
                    owed_at: None,
                    background: true,
                },
            );
        }
        r
    }

    /// T-S2b: §6.2 exhaustively over lead × one child of every kind/state.
    #[test]
    fn fold_table_is_exhaustive() {
        use ChildKind::*;
        use ChildState::*;
        let leads = [LeadState::Idle, LeadState::Working, LeadState::Waiting];
        let kinds = [Subagent, Shell, Monitor, Cron, Unknown];
        let states = [Running, ChildState::Waiting, Owed];
        for lead in leads {
            // No children.
            let want = match lead {
                LeadState::Waiting => Display::Waiting,
                LeadState::Working => Display::Working,
                LeadState::Idle => Display::Idle,
            };
            assert_eq!(raw_display(&row(lead, &[])), want, "{lead:?} alone");
            for kind in kinds {
                for state in states {
                    let got = raw_display(&row(lead, &[(kind, state)]));
                    let want = match (lead, state, kind) {
                        (LeadState::Waiting, _, _) => Display::Waiting,
                        (_, ChildState::Waiting, _) => Display::Waiting,
                        (LeadState::Working, _, _) => Display::Working,
                        (LeadState::Idle, Running | Owed, Subagent) => Display::Working,
                        (LeadState::Idle, Running | Owed, _) => Display::Monitoring,
                    };
                    assert_eq!(got, want, "lead={lead:?} child={kind:?}/{state:?}");
                }
            }
        }
    }

    #[test]
    fn reasons_follow_the_fold() {
        use ChildKind::*;
        use ChildState::*;
        let r = |lead, ch: &[(ChildKind, ChildState)]| raw_fold(&row(lead, ch)).1;
        assert_eq!(r(LeadState::Working, &[]), Reason::TurnRunning);
        assert_eq!(r(LeadState::Idle, &[(Subagent, Running)]), Reason::SubagentsRunning);
        assert_eq!(r(LeadState::Idle, &[(Subagent, Owed)]), Reason::OwedNotification);
        assert_eq!(r(LeadState::Idle, &[(Shell, Running)]), Reason::BackgroundShell);
        assert_eq!(r(LeadState::Idle, &[(Unknown, Running)]), Reason::BackgroundShell);
        assert_eq!(r(LeadState::Idle, &[(Monitor, Running)]), Reason::MonitorRunning);
        assert_eq!(r(LeadState::Idle, &[(Cron, Running)]), Reason::CronsScheduled);
        assert_eq!(r(LeadState::Idle, &[(Shell, Owed)]), Reason::OwedNotification);
        assert_eq!(r(LeadState::Idle, &[(Shell, ChildState::Waiting)]), Reason::WaitingPermission);
        let mut q = row(LeadState::Waiting, &[]);
        q.lead.waiting = Some(LeadWait { question: true, waiting_for: None, tool_name: None });
        assert_eq!(raw_fold(&q).1, Reason::WaitingQuestion);
        // Unconfirmed always folds idle, whatever the lead says.
        let mut u = row(LeadState::Working, &[]);
        u.confirmed = false;
        assert_eq!(raw_fold(&u), (Display::Idle, Reason::Unconfirmed));
        // Idle shows the last end.
        let mut d = row(LeadState::Idle, &[]);
        d.lead.outcome = Outcome::Failure;
        d.end_reason = Some(Reason::TurnFailed);
        assert_eq!(raw_fold(&d), (Display::Idle, Reason::TurnFailed));
    }

    /// T-S2e: 29 m 59 s → still working; 30 m → unverifiable; idle never decays.
    #[test]
    fn decay_is_thirty_minutes_and_never_done() {
        let at = 1_000;
        let (d, _, s) = decay(Display::Working, Reason::TurnRunning, Some(at), at + STALE_AFTER_MS - 1_000);
        assert_eq!((d, s), (Display::Working, None));
        let (d, r, s) = decay(Display::Working, Reason::TurnRunning, Some(at), at + STALE_AFTER_MS);
        assert_eq!((d, r, s), (Display::Unverifiable, Reason::StaleNoEvidence, Some(at + STALE_AFTER_MS)));
        for live in [Display::Monitoring, Display::Waiting] {
            assert_eq!(decay(live, Reason::BackgroundShell, Some(at), at + STALE_AFTER_MS).0, Display::Unverifiable);
        }
        assert_eq!(decay(Display::Idle, Reason::TurnDone, Some(at), at + 10 * STALE_AFTER_MS).0, Display::Idle);
    }

    #[test]
    fn keep_awake_holds_monitoring_but_not_cron_only() {
        let hold = 2 * 3600 * 1000;
        let mut shell = row(LeadState::Idle, &[(ChildKind::Shell, ChildState::Running)]);
        refold(&mut shell, 0);
        assert!(holds_awake(&shell, 0, hold));
        let mut cron = row(LeadState::Idle, &[(ChildKind::Cron, ChildState::Running)]);
        refold(&mut cron, 0);
        assert_eq!(cron.display, Display::Monitoring);
        assert!(!holds_awake(&cron, 0, hold), "a /loop cron must not hold the Mac awake");
        // Unverifiable keeps holding to 2 h after the last evidence.
        let mut w = row(LeadState::Working, &[]);
        refold(&mut w, 40 * 60 * 1000);
        assert_eq!(w.display, Display::Unverifiable);
        assert!(holds_awake(&w, 40 * 60 * 1000, hold));
        assert!(!holds_awake(&w, hold, hold));
        let mut idle = row(LeadState::Idle, &[]);
        refold(&mut idle, 0);
        assert!(!holds_awake(&idle, 0, hold));
    }
}
