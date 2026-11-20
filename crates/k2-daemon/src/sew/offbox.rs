//! Off-box copies of Sew's bundles carried by K2's backup platform
//! (0.46.17): the "external" ship.
//!
//! Hosted servers are on K2's backup platform: the box's backup client
//! (`k2-backup`, restic, nightly via `k2-backup.timer`) takes
//! `/var/lib/sew/backups` off the box. They have one disk, so
//! `k2 hostchat ship dir` refuses every folder on them, and nobody ran
//! `k2 hostchat ship box-backup`: the ship setting stayed `none` and
//! `k2_backup_offbox_none` said the backups stay on the box when they don't.
//!
//! Now, when the setting is `none` or `box-backup` and K2 sees the box
//! backup carry the bundles ([`super::box_backup::evidence`]: `paths`
//! covers the folder, `excludes` doesn't drop it, the timer is enabled),
//! the ship is **external**:
//!
//! - `status.backup.ship.kind` reads `external` for setting `none` (a
//!   chosen `box-backup` keeps its kind, so older clients still show it);
//!   `setting` is the stored choice, `label` is [`LABEL`], `lastShippedAt`
//!   is the newest snapshot that holds Sew's bundles, and `external` has
//!   the details ([`External::json`]).
//! - The newest snapshot: the end of the last **good** run of
//!   [`SERVICE_UNIT`] (one run = one snapshot of every listed path, and the
//!   evidence says Sew's folder is one of them). systemd keeps the last
//!   run's result and end time (`systemctl show`, no root needed); K2 keeps
//!   the newest good one it has seen in its own record ([`RUN_RECORD`]),
//!   because systemd forgets on a reboot and a failed run replaces the last
//!   good one. Nothing shells out to restic: the read is two systemd
//!   properties, cached for [`CACHE_SECS`].
//! - Alarms ([`alarms`]): no evidence keeps `k2_backup_offbox_none` (as
//!   before); evidence with a snapshot under [`STALE_HOURS`] old clears it;
//!   an older snapshot, or none K2 has seen, raises
//!   `k2_backup_offbox_stale` instead.
//!
//! The keys stay out of restic on purpose until key escrow lands, so the
//! `no_offbox_key_copy` warning and moveOffK2's `keyring_mirror` are not
//! touched here.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use super::box_backup::{self, Evidence};
use super::store::Row;
use super::supervisor::Env;
use k2_core::product_name;

/// `status.backup.ship.kind` for a ship carried by the box backup while
/// the setting is `none`.
pub const EXTERNAL: &str = "external";
/// How people see it.
pub const LABEL: &str = concat!(product_name!(), " backups (restic)");
/// The newest snapshot may be this old before `k2_backup_offbox_stale`.
pub const STALE_HOURS: i64 = 36;
pub const ALARM_NONE: &str = "k2_backup_offbox_none";
pub const ALARM_STALE: &str = "k2_backup_offbox_stale";
/// K2's record of the newest good run it has seen (`~/.k2/sew/<name>.json`).
pub const RUN_RECORD: &str = "box-backup-run";
/// The box backup client's service (the timer starts it).
pub const SERVICE_UNIT: &str = "k2-backup.service";
/// `ship none`'s note when the box backup still carries the bundles.
pub const NONE_BUT_CARRIED: &str = concat!(
    product_name!(),
    " copies nothing, but this box's own backup (",
    product_name!(),
    " backups (restic)) still carries Sew's bundles off the box: status shows the ship as external"
);
/// How long one `systemctl show` answer is reused.
pub const CACHE_SECS: u64 = 30;

/// What systemd says of [`SERVICE_UNIT`]'s last run. All empty / `None`
/// when systemd knows nothing (no unit, never ran since boot).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RunFacts {
    /// `Result=` (`success`, `exit-code`, `signal`, `timeout`, …).
    pub result: String,
    /// `ExecMainStatus=`.
    pub exit_status: Option<i32>,
    /// `ExecMainExitTimestamp=`, unix seconds.
    pub finished_at: Option<i64>,
    /// `ActiveState=` (`activating` while a run is under way).
    pub state: String,
}

impl RunFacts {
    /// A run that finished and worked.
    pub fn good(&self) -> bool {
        self.result == "success" && self.exit_status == Some(0) && self.finished_at.is_some()
    }

    fn json(&self) -> Value {
        json!({
            "result": (!self.result.is_empty()).then_some(&self.result),
            "exitStatus": self.exit_status,
            "finishedAt": self.finished_at,
            "state": (!self.state.is_empty()).then_some(&self.state),
            "ok": self.good(),
        })
    }
}

/// The external ship as K2 judges it now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct External {
    /// The newest snapshot with Sew's bundles K2 knows of (unix seconds).
    pub last_shipped_at: Option<i64>,
    /// Its age in seconds (`None` without one).
    pub age_secs: Option<i64>,
    /// Under [`STALE_HOURS`] old.
    pub fresh: bool,
    pub run: RunFacts,
}

impl External {
    pub fn line(&self) -> String {
        match (self.last_shipped_at, self.age_secs) {
            (Some(_), Some(age)) if self.fresh => {
                format!("{LABEL} carry Sew's bundles off the box (newest snapshot {} ago)", hours(age))
            }
            (Some(_), Some(age)) => format!(
                "{LABEL} carry Sew's bundles, but the newest snapshot is {} old (over {STALE_HOURS} h): check {SERVICE_UNIT}",
                hours(age)
            ),
            _ => format!(
                "{LABEL} are set to carry Sew's bundles, but {} hasn't seen a good run of {SERVICE_UNIT} yet \
                 (it reads systemd after each run; {ALARM_STALE} until then)",
                product_name!()
            ),
        }
    }

    pub fn json(&self) -> Value {
        json!({
            "kind": "k2-backup",
            "label": LABEL,
            "lastShippedAt": self.last_shipped_at,
            "ageHours": self.age_secs.map(|s| s / 3600),
            "staleAfterHours": STALE_HOURS,
            "fresh": self.fresh,
            "lastRun": self.run.json(),
            "line": self.line(),
        })
    }
}

fn hours(secs: i64) -> String {
    let h = secs.max(0) / 3600;
    if h == 0 {
        "under 1 h".to_string()
    } else {
        format!("{h} h")
    }
}

/// The evidence for a setting that may be carried by the box backup
/// (`none`, `box-backup`); `None` for `dir`.
pub fn evidence(env: &Env, row: &Row) -> Option<Evidence> {
    matches!(row.ship_kind.as_str(), "none" | box_backup::KIND).then(|| box_backup::evidence(env))
}

/// The external ship, when the box backup carries Sew's bundles.
pub fn external(env: &Env, row: &Row) -> Option<External> {
    evidence(env, row).filter(|e| e.ok).map(|_| judge_run(env))
}

/// Does the box backup carry Sew's bundles (whatever its freshness)?
pub fn carries(env: &Env, row: &Row) -> bool {
    evidence(env, row).is_some_and(|e| e.ok)
}

/// systemd's last run, merged with K2's record of the newest good one.
/// The record is written only when a newer good run shows up.
pub fn judge_run(env: &Env) -> External {
    let run = env.ops.box_backup_run();
    let recorded = env.ops.record_load(RUN_RECORD).and_then(|r| r.get("lastOkAt").and_then(Value::as_i64));
    let seen = if run.good() { run.finished_at } else { None };
    let newest = recorded.max(seen);
    let now = env.ops.now();
    if newest.is_some() && newest != recorded {
        let rec = json!({"lastOkAt": newest, "seenAt": now, "unit": SERVICE_UNIT});
        if let Err(e) = env.ops.record_save(RUN_RECORD, Some(&rec)) {
            k2_core::log_debug!("[sew] box backup run record: {e}");
        }
    }
    let age_secs = newest.map(|t| now - t);
    let fresh = age_secs.is_some_and(|a| a <= STALE_HOURS * 3600);
    External { last_shipped_at: newest, age_secs, fresh, run }
}

/// The off-box alarm: [`ALARM_NONE`] while nothing carries the bundles off
/// (setting `none` or `box-backup` without evidence), [`ALARM_STALE`] when
/// the box backup carries them but its newest snapshot is too old or
/// unknown. `dir` has its own (`k2_backup_not_shipped`, S32).
pub fn alarms(env: &Env, row: &Row) -> Vec<String> {
    match evidence(env, row) {
        None => vec![],
        Some(e) if !e.ok => vec![ALARM_NONE.into()],
        Some(_) if judge_run(env).fresh => vec![],
        Some(_) => vec![ALARM_STALE.into()],
    }
}

/// Add the external fields to a `{kind, path, …}` ship object: `setting`
/// always; `boxBackup` (the evidence) for a chosen `box-backup` or when it
/// carries the bundles; and, when it does, `kind: external` (setting
/// `none` only), `label`, `lastShippedAt` and `external`.
pub fn decorate_ship(env: &Env, row: &Row, ship: &mut Value) {
    ship["setting"] = json!(row.ship_kind);
    ship["external"] = Value::Null;
    let Some(ev) = evidence(env, row) else { return };
    if row.ship_kind == box_backup::KIND || ev.ok {
        ship["boxBackup"] = ev.json();
    }
    if !ev.ok {
        return;
    }
    let x = judge_run(env);
    if row.ship_kind == "none" {
        ship["kind"] = json!(EXTERNAL);
    }
    ship["label"] = json!(LABEL);
    ship["lastShippedAt"] = json!(x.last_shipped_at);
    ship["external"] = x.json();
}

// ── reading systemd ─────────────────────────────────────────────────────

/// `systemctl show` for [`SERVICE_UNIT`] as the daemon user. Asks for unix
/// timestamps (systemd 247+); an older systemd refuses the flag, and the
/// read is done again in its default form under `TZ=UTC`.
pub fn read_run() -> RunFacts {
    let props = "Result,ExecMainStatus,ExecMainExitTimestamp,ActiveState";
    let show = |unix: bool| {
        let mut c = k2_core::bg_proc::command("/usr/bin/systemctl");
        c.arg("show").arg(SERVICE_UNIT).arg("-p").arg(props);
        if unix {
            c.arg("--timestamp=unix");
        }
        c.env("TZ", "UTC")
            .env("LC_ALL", "C")
            .stdin(std::process::Stdio::null())
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
    };
    show(true).or_else(|| show(false)).map(|t| parse_show(&t)).unwrap_or_default()
}

/// `Key=Value` lines of `systemctl show` → [`RunFacts`].
pub fn parse_show(text: &str) -> RunFacts {
    let mut f = RunFacts::default();
    for line in text.lines() {
        let Some((k, v)) = line.split_once('=') else { continue };
        let v = v.trim();
        match k.trim() {
            "Result" => f.result = v.to_string(),
            "ExecMainStatus" => f.exit_status = v.parse().ok(),
            "ExecMainExitTimestamp" => f.finished_at = parse_timestamp(v),
            "ActiveState" => f.state = v.to_string(),
            _ => {}
        }
    }
    f
}

/// `@1700000000` (`--timestamp=unix`) or `Thu 2026-11-19 07:41:02 UTC`
/// (the default form under `TZ=UTC`); empty or `n/a` = none.
pub fn parse_timestamp(v: &str) -> Option<i64> {
    if let Some(n) = v.strip_prefix('@') {
        return n.split('.').next()?.parse().ok().filter(|t| *t > 0);
    }
    let rest = v.strip_suffix(" UTC")?;
    let rest = rest.split_once(' ').map(|(_, r)| r)?;
    chrono::NaiveDateTime::parse_from_str(rest, "%Y-%m-%d %H:%M:%S").ok().map(|t| t.and_utc().timestamp())
}

static CACHE: Mutex<Option<(Instant, RunFacts)>> = Mutex::new(None);

/// [`CACHE_SECS`]-old facts, or a fresh read.
pub fn cached(read: impl FnOnce() -> RunFacts) -> RunFacts {
    let mut c = CACHE.lock().unwrap_or_else(|p| p.into_inner());
    if let Some((at, f)) = c.as_ref() {
        if at.elapsed() < Duration::from_secs(CACHE_SECS) {
            return f.clone();
        }
    }
    let f = read();
    *c = Some((Instant::now(), f.clone()));
    f
}

#[cfg(test)]
#[path = "offbox_tests.rs"]
mod tests;
