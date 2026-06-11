//! Canonical daemon-owned "Active" set compute (PRD
//! `.k2so/prds/daemon-canonical-active.md`, task #672).
//!
//! The Active set is a **derived** property of the daemon — there is no
//! new storage for it. A workspace is Active iff it is manually pinned
//! (`manually_active != 0`) OR it was interacted with inside the
//! configured tenure window (`now - last_interaction_at <
//! active_window_hours`). Because `manually_active` /
//! `last_interaction_at` are global-per-daemon columns on `projects`,
//! "any client activated it" ⇒ "Active for everyone connected" falls
//! out for free — that *is* the multi-user union the PRD specifies.
//!
//! This module holds the PURE decision so it can be unit-tested without
//! a DB, a tokio runtime, or any daemon wiring. The daemon's
//! `/cli/projects/active` route + the Active reaper both call into the
//! exact same fn so the snapshot the renderer mirrors and the reap
//! gate can never disagree.
//!
//! **Time units.** `projects.last_interaction_at` is stored in unix
//! *seconds* (`unixepoch()` — see `db::schema::Project::touch_interaction`).
//! The compute fn takes `now_ms` (unix milliseconds, the renderer's
//! native clock + the daemon's `SystemEvent` clock) and converts the
//! row's seconds to ms before comparing, so callers never have to
//! reconcile the two clocks themselves.

/// A minimal, DB-free view of a project row for the Active compute.
/// Built from `db::schema::Project` (or any equivalent source) so the
/// decision logic stays a pure function with no `rusqlite` dependency
/// and a trivial truth-table unit test.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectRow {
    /// Project primary-key id (the canonical Active-set membership token).
    pub id: String,
    /// `projects.manually_active` — non-zero pins the workspace into the
    /// Active bar regardless of the interaction window.
    pub manually_active: i64,
    /// `projects.last_interaction_at` in unix **seconds**, or `None` if
    /// the workspace has never been interacted with.
    pub last_interaction_at_secs: Option<i64>,
}

/// Compute the canonical Active project-id set.
///
/// `now_ms` is unix **milliseconds**. `active_window_hours` is the
/// configured tenure window (`app_settings.active_window_hours`,
/// default 24). A project is Active iff:
///
///   * `manually_active != 0` (pinned — never ages out), OR
///   * `now_ms - (last_interaction_at_secs * 1000) < window_ms`
///     (interacted with inside the window).
///
/// Returns ids in input order. Idempotent + order-independent at the
/// call site — emitting the full set (rather than a diff) makes client
/// convergence trivial (last-write-wins on a monotonic snapshot).
///
/// **Boundary:** a row exactly at the window edge (`age == window_ms`)
/// is NOT active — the predicate is strict `<`, matching "within the
/// last N hours" rather than "N hours or older".
pub fn active_project_ids(
    now_ms: i64,
    rows: &[ProjectRow],
    active_window_hours: u32,
) -> Vec<String> {
    let window_ms = (active_window_hours as i64) * 60 * 60 * 1000;
    rows.iter()
        .filter(|r| is_active_row(now_ms, r, window_ms))
        .map(|r| r.id.clone())
        .collect()
}

/// Single-row Active predicate. `window_ms` is the precomputed window
/// in milliseconds (so the per-row hot path avoids re-multiplying).
fn is_active_row(now_ms: i64, row: &ProjectRow, window_ms: i64) -> bool {
    if row.manually_active != 0 {
        return true;
    }
    match row.last_interaction_at_secs {
        Some(secs) => {
            let last_ms = secs.saturating_mul(1000);
            let age_ms = now_ms - last_ms;
            // `age_ms < 0` (clock skew: interaction stamped in the
            // future) counts as active — a fresh interaction can never
            // be "aged out".
            age_ms < window_ms
        }
        None => false,
    }
}

/// Convenience: is a single project id in the Active set computed from
/// `rows`? Used by the reaper's fire-time re-check (it already has the
/// rows in hand). Equivalent to
/// `active_project_ids(..).contains(&id)` but avoids the allocation.
pub fn is_in_active_set(
    now_ms: i64,
    rows: &[ProjectRow],
    active_window_hours: u32,
    project_id: &str,
) -> bool {
    let window_ms = (active_window_hours as i64) * 60 * 60 * 1000;
    rows.iter()
        .any(|r| r.id == project_id && is_active_row(now_ms, r, window_ms))
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOUR_MS: i64 = 60 * 60 * 1000;

    fn row(id: &str, manually_active: i64, last_secs: Option<i64>) -> ProjectRow {
        ProjectRow {
            id: id.to_string(),
            manually_active,
            last_interaction_at_secs: last_secs,
        }
    }

    /// Truth table: pinned (manually_active) is always active; within
    /// the window is active; aged past the window is not; never-touched
    /// is not.
    #[test]
    fn truth_table_pinned_within_aged_never() {
        // now = 100h after epoch, window = 24h.
        let now_ms = 100 * HOUR_MS;
        let secs = |h: i64| Some((h * HOUR_MS) / 1000); // hours → unix secs

        let rows = vec![
            // pinned + ancient interaction → ACTIVE (pin overrides window)
            row("pinned-ancient", 1, secs(0)),
            // pinned + never touched → ACTIVE
            row("pinned-never", 1, None),
            // within window: interacted 1h ago → ACTIVE
            row("recent", 0, secs(99)),
            // within window: interacted 23h ago → ACTIVE
            row("just-inside", 0, secs(77)),
            // aged: interacted 25h ago → NOT active
            row("aged", 0, secs(75)),
            // aged: interacted 100h ago → NOT active
            row("ancient", 0, secs(0)),
            // never touched, not pinned → NOT active
            row("never", 0, None),
        ];

        let active = active_project_ids(now_ms, &rows, 24);
        assert_eq!(
            active,
            vec![
                "pinned-ancient".to_string(),
                "pinned-never".to_string(),
                "recent".to_string(),
                "just-inside".to_string(),
            ],
            "only pinned + within-window workspaces are active"
        );
    }

    /// Window boundary: `age == window` is NOT active (strict `<`),
    /// `age == window - 1ms` IS active.
    #[test]
    fn window_boundary_is_strict() {
        let window_hours = 24u32;
        let window_ms = (window_hours as i64) * HOUR_MS;
        let now_ms = 1_000 * HOUR_MS; // arbitrary large now

        // Exactly at the window edge: last = now - window (in secs).
        let edge_secs = (now_ms - window_ms) / 1000;
        let edge = row("edge", 0, Some(edge_secs));
        assert!(
            !active_project_ids(now_ms, &[edge], window_hours).contains(&"edge".to_string()),
            "a row exactly at the window edge must NOT be active (strict <)"
        );

        // Just inside: one second newer than the edge.
        let inside = row("inside", 0, Some(edge_secs + 1));
        assert!(
            active_project_ids(now_ms, &[inside], window_hours).contains(&"inside".to_string()),
            "a row one second inside the window must be active"
        );
    }

    /// Future-stamped interaction (clock skew) counts as active — a
    /// fresh interaction can never be aged out.
    #[test]
    fn future_interaction_is_active() {
        let now_ms = 50 * HOUR_MS;
        let future_secs = (now_ms + 10 * HOUR_MS) / 1000;
        let r = row("future", 0, Some(future_secs));
        assert!(active_project_ids(now_ms, &[r], 24).contains(&"future".to_string()));
    }

    /// Zero-hour window: nothing is active by recency; only pins survive.
    #[test]
    fn zero_window_only_pins() {
        let now_ms = 10 * HOUR_MS;
        let rows = vec![
            row("pinned", 1, Some(now_ms / 1000)),
            row("fresh", 0, Some(now_ms / 1000)),
        ];
        let active = active_project_ids(now_ms, &rows, 0);
        assert_eq!(active, vec!["pinned".to_string()]);
    }

    /// `is_in_active_set` matches `active_project_ids().contains()`.
    #[test]
    fn is_in_active_set_matches_full_compute() {
        let now_ms = 100 * HOUR_MS;
        let rows = vec![
            row("a", 0, Some((99 * HOUR_MS) / 1000)), // recent
            row("b", 0, Some(0)),                     // aged
            row("c", 1, None),                        // pinned
        ];
        for id in ["a", "b", "c", "missing"] {
            let via_set = active_project_ids(now_ms, &rows, 24).contains(&id.to_string());
            let via_helper = is_in_active_set(now_ms, &rows, 24, id);
            assert_eq!(via_set, via_helper, "mismatch for id={id}");
        }
        assert!(is_in_active_set(now_ms, &rows, 24, "a"));
        assert!(!is_in_active_set(now_ms, &rows, 24, "b"));
        assert!(is_in_active_set(now_ms, &rows, 24, "c"));
        assert!(!is_in_active_set(now_ms, &rows, 24, "missing"));
    }

    /// Empty input → empty set.
    #[test]
    fn empty_rows_empty_set() {
        assert!(active_project_ids(123, &[], 24).is_empty());
    }
}
