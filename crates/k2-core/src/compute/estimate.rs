//! A job's expected run time (design-compute-duration-aware-queue-v1): the
//! number the scheduler orders and backfills by, and the ETA reads.
//!
//! A job's **family** is its workspace and `--tag` when it has one, else
//! its workspace and argv (never the sha: every commit changes it). The
//! estimate, first that applies:
//! 1. the family's history (the median of its last [`HISTORY`] `done`
//!    runs, this node's when it has [`ENOUGH`], else any node's) with
//!    [`ENOUGH`] runs or more. A `--expect` outside ½×–2× of it loses, and
//!    `get` says so ([`Basis::HistoryOverExpect`]);
//! 2. the submitter's `--expect`, unless the family is history-only;
//! 3. history with fewer runs;
//! 4. none ([`Basis::Average`]): ordered as the scheduler's average, no ETA.
//!
//! **History-only**: a finished run whose `--expect` was under ⅓ of its
//! measured time, and short by [`SHORT_BY_SECS`] or more, makes the family
//! ignore `--expect` for [`HISTORY_ONLY_SECS`] after that run ended, so a
//! stated time can't be used to jump the queue. It is read from the runs'
//! own stored `expectSecs`: no table of its own.

use rusqlite::{params, Connection};

use super::message::duration;

/// Runs read per family.
pub const HISTORY: i64 = 20;
/// Runs that make history win over a stated time.
pub const ENOUGH: usize = 3;
/// How long a family stays history-only after an under-stated run.
pub const HISTORY_ONLY_SECS: i64 = 7 * 86_400;
/// An under-stated run was short by at least this much.
pub const SHORT_BY_SECS: i64 = 300;
/// The longest `--expect` taken.
pub const EXPECT_MAX_SECS: i64 = 7 * 86_400;

/// One finished run of a family.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Run {
    /// How long it ran.
    pub secs: i64,
    /// The `--expect` it was sent with.
    pub expect_secs: Option<i64>,
    pub ended_at: i64,
}

/// What the submitter said (`--expect`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Said {
    /// Nothing.
    #[default]
    Nothing,
    /// `--expect unknown`.
    Unknown,
    /// `--expect <duration>`, in seconds.
    Secs(i64),
}

impl Said {
    pub fn secs(self) -> Option<i64> {
        match self {
            Said::Secs(s) => Some(s),
            _ => None,
        }
    }
}

/// Where an estimate came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Basis {
    /// [`ENOUGH`] or more earlier runs (a stated time agreeing or none).
    History,
    /// [`ENOUGH`] or more earlier runs, and a stated time far off them.
    HistoryOverExpect,
    /// The stated `--expect`.
    Expect,
    /// The family is history-only: the stated time was ignored.
    HistoryOnly,
    /// Fewer than [`ENOUGH`] earlier runs, nothing stated.
    HistoryFew,
    /// Nothing known.
    Average,
}

impl Basis {
    pub fn code(self) -> &'static str {
        match self {
            Basis::History => "history",
            Basis::HistoryOverExpect => "history_over_expect",
            Basis::Expect => "expect",
            Basis::HistoryOnly => "history_only",
            Basis::HistoryFew => "history_few",
            Basis::Average => "average",
        }
    }

    pub fn from_code(code: &str) -> Option<Basis> {
        [Basis::History, Basis::HistoryOverExpect, Basis::Expect, Basis::HistoryOnly, Basis::HistoryFew, Basis::Average].into_iter().find(|b| b.code() == code)
    }
}

/// A stored job's estimate as `get`, `jobs` and `wait` show it (JSON):
/// `secs`, `basis`, `runs`, `said` (the `--expect`, or `"unknown"`),
/// `historySecs`, `words`. Null for a job stored before this.
pub fn shown(est_secs: Option<i64>, ctl: &super::store::JobCtl, avg_secs: u64) -> serde_json::Value {
    let Some(basis) = ctl.est_basis.as_deref().and_then(Basis::from_code) else { return serde_json::Value::Null };
    let said = match (ctl.expect_secs, ctl.expect_unknown) {
        (Some(s), _) => Said::Secs(s),
        (None, true) => Said::Unknown,
        _ => Said::Nothing,
    };
    let e = Estimate {
        secs: est_secs,
        p90: None,
        basis,
        runs: ctl.est_runs.unwrap_or(0) as usize,
        history_secs: ctl.est_history_secs,
        expect_ignored_until: ctl.expect_ignored_until,
    };
    let said_v = match said {
        Said::Secs(s) => serde_json::Value::from(s),
        Said::Unknown => "unknown".into(),
        Said::Nothing => serde_json::Value::Null,
    };
    serde_json::json!({
        "secs": est_secs,
        "basis": basis.code(),
        "runs": e.runs,
        "said": said_v,
        "historySecs": e.history_secs,
        "expectIgnoredUntil": e.expect_ignored_until,
        "words": words(&e, said, avg_secs),
    })
}

/// A job's estimate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Estimate {
    /// Expected seconds (`None`: unknown, [`Basis::Average`]).
    pub secs: Option<i64>,
    /// The 90th percentile of the history read (else `secs`).
    pub p90: Option<i64>,
    pub basis: Basis,
    /// Earlier runs read.
    pub runs: usize,
    /// The history's median, when there is one.
    pub history_secs: Option<i64>,
    /// History-only until then.
    pub expect_ignored_until: Option<i64>,
}

/// The family's last runs: by `tag` when given, else by `argv`; on
/// `node_id` only when given. Newest first.
pub fn family_runs(conn: &Connection, workspace_id: &str, argv: &[String], tag: Option<&str>, node_id: Option<&str>, limit: i64) -> Vec<Run> {
    let Ok(argv_json) = serde_json::to_string(argv) else { return Vec::new() };
    let Ok(mut stmt) = conn.prepare(
        "SELECT ended_at - started_at, json_extract(plan_json, '$.k2ctl.expectSecs'), ended_at FROM compute_jobs \
         WHERE workspace_id = ?1 AND state = 'done' AND started_at IS NOT NULL AND ended_at IS NOT NULL \
         AND (CASE WHEN ?3 IS NULL THEN argv_json = ?2 ELSE json_extract(plan_json, '$.k2ctl.tag') = ?3 END) \
         AND (?4 IS NULL OR node_id = ?4) ORDER BY ended_at DESC LIMIT ?5",
    ) else {
        return Vec::new();
    };
    stmt.query_map(params![workspace_id, argv_json, tag, node_id, limit.clamp(1, 200)], |r| {
        Ok(Run { secs: r.get::<_, i64>(0)?, expect_secs: r.get::<_, Option<i64>>(1)?, ended_at: r.get::<_, i64>(2)? })
    })
    .map(|rows| rows.filter_map(Result::ok).filter(|r| r.secs >= 0).collect())
    .unwrap_or_default()
}

/// Did this run under-state its time badly ([`SHORT_BY_SECS`], under ⅓)?
pub fn under_stated(r: &Run) -> bool {
    r.expect_secs.is_some_and(|e| e.saturating_mul(3) < r.secs && r.secs - e >= SHORT_BY_SECS)
}

/// The estimate from this node's runs and any node's runs of the family,
/// and what was said, at `now`.
pub fn pick(on_node: &[Run], any_node: &[Run], said: Said, now: i64) -> Estimate {
    let runs = if on_node.len() >= ENOUGH { on_node } else { any_node };
    let ignored_until = any_node.iter().filter(|r| under_stated(r)).map(|r| r.ended_at + HISTORY_ONLY_SECS).filter(|u| *u > now).max();
    let secs: Vec<i64> = runs.iter().map(|r| r.secs).collect();
    let hist = super::store::median_p90(&secs);
    let n = runs.len();
    let est = |secs: Option<i64>, p90: Option<i64>, basis: Basis| Estimate {
        secs,
        p90,
        basis,
        runs: n,
        history_secs: hist.map(|h| h.0),
        expect_ignored_until: ignored_until,
    };
    match (hist, said.secs()) {
        (Some((m, p)), Some(e)) if n >= ENOUGH && (e.saturating_mul(2) < m || e > m.saturating_mul(2)) => est(Some(m), Some(p), Basis::HistoryOverExpect),
        (Some((m, p)), _) if n >= ENOUGH => est(Some(m), Some(p), Basis::History),
        (Some((m, p)), Some(_)) if ignored_until.is_some() => est(Some(m), Some(p), Basis::HistoryOnly),
        (_, Some(e)) => est(Some(e), Some(e), Basis::Expect),
        (Some((m, p)), None) => est(Some(m), Some(p), Basis::HistoryFew),
        (None, None) => est(None, None, Basis::Average),
    }
}

/// `--expect` as the next submission should say it: rounded up to the
/// minute, past an hour to 5 minutes (`12m`, `1h35m`, `2h`).
pub fn expect_flag(secs: i64) -> String {
    let s = secs.max(1);
    if s < 3600 {
        return format!("{}m", (s + 59) / 60);
    }
    let m = (s + 299) / 300 * 5;
    match m % 60 {
        0 => format!("{}h", m / 60),
        r => format!("{}h{r}m", m / 60),
    }
}

/// `took 11m40s; next time: --expect 12m`.
pub fn took_words(secs: i64) -> String {
    format!("took {}; next time: --expect {}", duration(secs), expect_flag(secs))
}

/// `~25m` (whole minutes, at least 1; hours past 90 min).
pub fn approx(secs: i64) -> String {
    let m = (secs.max(1) + 59) / 60;
    if m >= 90 {
        let h = m / 60;
        match m % 60 {
            0 => format!("~{h}h"),
            r => format!("~{h}h{r:02}m"),
        }
    } else {
        format!("~{m}m")
    }
}

/// What `get` and the queue line say of an estimate. `said`: the stated
/// time; `avg_secs`: what an unknown one counts as.
pub fn words(e: &Estimate, said: Said, avg_secs: u64) -> String {
    let runs = |n: usize| if n == 1 { "1 earlier run".to_string() } else { format!("{n} earlier runs") };
    let at = |s: Option<i64>| approx(s.unwrap_or(0));
    match e.basis {
        Basis::History => format!("{} by its last {}", at(e.secs), runs(e.runs)),
        Basis::HistoryOverExpect => format!(
            "you said {}; its last {} took {}: queued as {}",
            duration(said.secs().unwrap_or(0)),
            runs(e.runs),
            at(e.secs),
            at(e.secs).trim_start_matches('~')
        ),
        Basis::Expect => match e.history_secs {
            Some(_) => format!("{} as stated (--expect; {} so far)", at(e.secs), runs(e.runs)),
            None => format!("{} as stated (--expect; no earlier runs yet)", at(e.secs)),
        },
        Basis::HistoryOnly => format!(
            "{} by {}: --expect is ignored for this job's family for now (a run took over 3× its stated time)",
            at(e.secs),
            runs(e.runs)
        ),
        Basis::HistoryFew => format!("{} by {} (no --expect)", at(e.secs), runs(e.runs)),
        Basis::Average => {
            let why = if said == Said::Unknown { "--expect unknown" } else { "no earlier runs, no --expect" };
            format!("unknown ({why}): placed as an average job ({})", approx(avg_secs as i64).trim_start_matches('~'))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(secs: i64) -> Run {
        Run { secs, expect_secs: None, ended_at: 1_000 }
    }

    #[test]
    fn history_with_enough_runs_wins_and_a_far_off_stated_time_loses() {
        let h = vec![run(1500), run(1400), run(1600)];
        let e = pick(&h, &h, Said::Nothing, 2_000);
        assert_eq!((e.secs, e.basis, e.runs), (Some(1500), Basis::History, 3));
        let e = pick(&h, &h, Said::Secs(1200), 2_000);
        assert_eq!((e.secs, e.basis), (Some(1500), Basis::History), "within ½×–2×: history, no note");
        let e = pick(&h, &h, Said::Secs(120), 2_000);
        assert_eq!((e.secs, e.basis), (Some(1500), Basis::HistoryOverExpect));
        assert_eq!(words(&e, Said::Secs(120), 600), "you said 2m00s; its last 3 earlier runs took ~25m: queued as 25m");
        let e = pick(&h, &h, Said::Secs(4000), 2_000);
        assert_eq!(e.basis, Basis::HistoryOverExpect, "over-stated far too");
    }

    #[test]
    fn a_stated_time_beats_thin_history_and_nothing_is_average() {
        let few = vec![run(600)];
        let e = pick(&[], &few, Said::Secs(700), 2_000);
        assert_eq!((e.secs, e.basis), (Some(700), Basis::Expect));
        let e = pick(&[], &few, Said::Nothing, 2_000);
        assert_eq!((e.secs, e.basis), (Some(600), Basis::HistoryFew));
        let e = pick(&[], &[], Said::Secs(90), 2_000);
        assert_eq!((e.secs, e.basis), (Some(90), Basis::Expect));
        assert_eq!(words(&e, Said::Secs(90), 600), "~2m as stated (--expect; no earlier runs yet)");
        let e = pick(&[], &[], Said::Unknown, 2_000);
        assert_eq!((e.secs, e.basis), (None, Basis::Average));
        assert_eq!(words(&e, Said::Unknown, 600), "unknown (--expect unknown): placed as an average job (10m)");
    }

    #[test]
    fn this_nodes_runs_count_only_with_enough_of_them() {
        let node = vec![run(100), run(110), run(120)];
        let any = vec![run(100), run(110), run(120), run(900), run(950)];
        assert_eq!(pick(&node, &any, Said::Nothing, 2_000).secs, Some(110));
        assert_eq!(pick(&node[..2], &any, Said::Nothing, 2_000).secs, Some(120), "too few here: every node's median");
    }

    /// A run that took 25 min against a stated 2 makes the family ignore
    /// `--expect` for 7 days after it ended; then it counts again.
    #[test]
    fn an_under_stated_run_makes_the_family_history_only_for_a_while() {
        let liar = Run { secs: 1500, expect_secs: Some(120), ended_at: 10_000 };
        assert!(under_stated(&liar));
        assert!(!under_stated(&Run { secs: 299, expect_secs: Some(60), ended_at: 0 }), "short by under 5 min");
        assert!(!under_stated(&Run { secs: 1500, expect_secs: Some(600), ended_at: 0 }), "over ⅓");
        let e = pick(&[], &[liar], Said::Secs(60), 10_100);
        assert_eq!((e.secs, e.basis, e.expect_ignored_until), (Some(1500), Basis::HistoryOnly, Some(10_000 + HISTORY_ONLY_SECS)));
        let later = pick(&[], &[liar], Said::Secs(60), 10_000 + HISTORY_ONLY_SECS + 1);
        assert_eq!((later.secs, later.basis), (Some(60), Basis::Expect), "after 7 days a stated time counts again");
    }

    #[test]
    fn the_next_expect_rounds_up() {
        assert_eq!(expect_flag(700), "12m");
        assert_eq!(expect_flag(45), "1m");
        assert_eq!(expect_flag(3600), "1h");
        assert_eq!(expect_flag(3601), "1h5m");
        assert_eq!(expect_flag(5_640), "1h35m");
        assert_eq!(took_words(700), "took 11m40s; next time: --expect 12m");
        assert_eq!(approx(1500), "~25m");
        assert_eq!(approx(7200), "~2h");
    }
}
