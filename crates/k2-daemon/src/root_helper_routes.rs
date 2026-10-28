//! `/cli/root-helper/*` (`prd-k2-root-helper-v1` §8.3).
//!
//! - `GET  /cli/root-helper/status` — Member floor; agents may read it
//!   (`is_agent_verb`). `?events=1` adds the helper's last journal lines
//!   (`helper audit-tail`).
//! - `POST /cli/root-helper/update` — Admin; passports refused. Runs the
//!   auto-heal now. Body `{"fromDir": "<dir>"}` installs from local signed
//!   assets (air-gap) — the signature still gates it, no root needed.
//!   `{"version": "x.y.z"}` fetches that release's helper instead of the
//!   running daemon's; `{"matchInstalled": true}` fetches the version of
//!   the daemon binary on disk (a swap already done, restart pending). The
//!   answer's `warnings` say when the installed or a staged daemon is newer
//!   than the helper it fetched (0.46.6: sewlab stayed on helper 0.46.4
//!   because `update` ran before the binary swap).
//! - `POST /cli/root-helper/rollback` — Admin; passports refused.
//!
//! `setup` (the box-sudo install button) is a later slice with the
//! Settings card.

use std::collections::HashMap;

use serde_json::{json, Value};

use crate::cli_response::CliResponse;
use crate::root_helper::{self, heal};
use crate::sidecar_routes::Caller;

pub const STATUS: &str = "/cli/root-helper/status";
pub const UPDATE: &str = "/cli/root-helper/update";
pub const ROLLBACK: &str = "/cli/root-helper/rollback";

pub const MAX_BODY: usize = 4 * 1024;

pub fn is_route(path: &str) -> bool {
    matches!(path, STATUS | UPDATE | ROLLBACK)
}

pub fn is_post_route(path: &str) -> bool {
    matches!(path, UPDATE | ROLLBACK)
}

fn refuse(status: &'static str, code: &str, hint: impl Into<String>) -> CliResponse {
    CliResponse {
        status,
        content_type: "application/json",
        body: json!({"error": {"code": code, "hint": hint.into()}}).to_string(),
    }
}

pub fn too_large() -> CliResponse {
    refuse(
        "413 Payload Too Large",
        "body_too_large",
        "request bodies here are at most 4 KiB",
    )
}

fn ok(v: Value) -> CliResponse {
    CliResponse::ok_json(v.to_string())
}

fn human(caller: &Caller) -> Option<String> {
    match caller {
        Caller::Owner => Some("owner".into()),
        Caller::Login { username } => Some(username.clone()),
        _ => None,
    }
}

pub fn handle(
    path: &str,
    caller: Caller,
    body: &[u8],
    params: &HashMap<String, String>,
) -> CliResponse {
    if matches!(caller, Caller::Invalid | Caller::WrongCredential) {
        return refuse(
            "401 Unauthorized",
            "unauthorized",
            "owner token, a Connect login or a K2 passport",
        );
    }
    match path {
        STATUS => {
            if params.get("refresh").map(String::as_str) == Some("1") {
                root_helper::invalidate();
            }
            let st = root_helper::state();
            let legacy_mail = crate::mail::supervisor::mail_helper_legacy_state().as_str();
            let events = if params.get("events").map(String::as_str) == Some("1")
                && st.kind == root_helper::Kind::Root
            {
                root_helper::call_text(&["helper".into(), "audit-tail".into()], None)
                    .ok()
                    .and_then(|t| serde_json::from_str::<Value>(&t).ok())
            } else {
                None
            };
            ok(root_helper::status_json(
                &st,
                legacy_mail,
                root_helper::legacy_pg_present(),
                events,
            ))
        }
        UPDATE | ROLLBACK => {
            let Some(who) = human(&caller) else {
                return refuse(
                    "403 Forbidden",
                    "owner_only",
                    "updating or rolling back the root helper is an owner/admin step: run `k2 root-helper` outside a K2 terminal",
                );
            };
            if !cfg!(target_os = "linux") {
                return refuse(
                    "409 Conflict",
                    "linux_only",
                    "the root helper exists on Linux daemons only",
                );
            }
            if path == ROLLBACK {
                root_helper::invalidate();
                let r = root_helper::call_text(&["helper".into(), "self-rollback".into()], None);
                root_helper::invalidate();
                k2_core::auth_audit::record(&k2_core::auth_audit::AuditEvent::new(
                    "root_helper_rollback",
                    &who,
                    match &r {
                        Ok(_) => "ok".to_string(),
                        Err(e) => format!("failed: {e}"),
                    },
                    "cli",
                    "-",
                    "cli",
                ));
                return match r {
                    Ok(t) => ok(serde_json::from_str(&t).unwrap_or_else(|_| json!({"result": t}))),
                    Err(e) => refuse("409 Conflict", "rollback_failed", e),
                };
            }
            let req = serde_json::from_slice::<Value>(body).unwrap_or(Value::Null);
            let from_dir = req["fromDir"]
                .as_str()
                .map(str::to_string)
                .filter(|s| !s.is_empty());
            let requested = req["version"].as_str().filter(|s| !s.is_empty());
            let match_installed = req["matchInstalled"].as_bool() == Some(true);
            let running = env!("CARGO_PKG_VERSION");
            // Read every time: it also feeds the "newer daemon" warning.
            let installed = on_disk_daemon_version();
            let target = match update_target(
                running,
                requested,
                match_installed,
                installed.as_deref(),
            ) {
                Ok(t) => t,
                Err(r) => return r,
            };
            let staged = crate::update_routes::staged_target_version();
            let warnings = newer_daemon_warnings(&target, installed.as_deref(), staged.as_deref());
            let mut plan = heal::live_plan(None);
            plan.daemon_version = target.clone();
            let out = match from_dir {
                Some(dir) => {
                    let p = std::path::PathBuf::from(&dir);
                    if !p.is_absolute() || !p.is_dir() {
                        return refuse(
                            "400 Bad Request",
                            "bad_dir",
                            "fromDir must be an existing absolute directory",
                        );
                    }
                    // Air-gap: never the network; the signature still gates it.
                    plan.airgap = false;
                    heal::run_once(&heal::DirOps { dir: p }, &plan, &who)
                }
                None => heal::run_once(&heal::LiveOps, &plan, &who),
            };
            let cli = root_helper::cli_refresh::run_once(
                &root_helper::cli_refresh::LiveCliOps,
                &root_helper::cli_refresh::daemon_cli(),
                &who,
            );
            // 0.46.5: converge the daemon unit's hand-off drop-in now,
            // through the helper just installed (it writes the file and
            // reloads systemd), rather than at the next daemon restart.
            // The answer says whether one restart is still needed.
            let dropin = crate::handoff_setup::after_helper_update(false);
            ok(
                json!({"target": target, "warnings": warnings, "heal": out, "cli": cli, "dropin": dropin, "status": root_helper::status_json(&root_helper::state(), crate::mail::supervisor::mail_helper_legacy_state().as_str(), root_helper::legacy_pg_present(), None)}),
            )
        }
        _ => refuse("404 Not Found", "not_found", "unknown root-helper route"),
    }
}

/// Which release's helper `update` fetches: `version` when asked, the
/// on-disk daemon's with `matchInstalled`, else the running daemon's.
fn update_target(
    running: &str,
    requested: Option<&str>,
    match_installed: bool,
    installed: Option<&str>,
) -> Result<String, CliResponse> {
    use k2_root_proto::version::Semver;
    match (requested, match_installed) {
        (Some(_), true) => Err(refuse(
            "400 Bad Request",
            "bad_request",
            "give --version or --match-installed, not both",
        )),
        (Some(v), false) => match Semver::parse(v) {
            Some(sv) => Ok(sv.to_string()),
            None => Err(refuse(
                "400 Bad Request",
                "bad_version",
                format!("--version must be x.y.z (got {v:?})"),
            )),
        },
        (None, true) => match installed.and_then(Semver::parse) {
            Some(sv) => Ok(sv.to_string()),
            None => Err(refuse(
                "409 Conflict",
                "installed_version_unknown",
                "could not read the version of the daemon binary on disk; pass --version <x.y.z>",
            )),
        },
        (None, false) => Ok(running.to_string()),
    }
}

/// One warning per daemon (installed on disk, staged by an update) whose
/// version is newer than the helper `update` fetches for `target`: once
/// that daemon runs it wants its own helper, and this one is already old.
fn newer_daemon_warnings(target: &str, installed: Option<&str>, staged: Option<&str>) -> Vec<String> {
    use k2_root_proto::version::Semver;
    let Some(t) = Semver::parse(target) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (what, v) in [("installed daemon binary", installed), ("staged daemon update", staged)] {
        let Some(sv) = v.and_then(Semver::parse) else {
            continue;
        };
        if sv > t {
            out.push(format!(
                "the {what} is v{sv}, newer than the helper this fetches (v{t}): \
                 run `k2 root-helper update --version {sv}` (or `--match-installed`) to install its helper now; \
                 otherwise the daemon fetches it after it restarts on v{sv}"
            ));
        }
    }
    out
}

/// The version of the daemon binary at this process's own path, read with
/// `k2-daemon --version` (which never boots). After a binary swap Linux
/// names the running image `<path> (deleted)`; the new file at `<path>` is
/// the installed daemon. `None` when it can't be read within 5 s.
fn on_disk_daemon_version() -> Option<String> {
    let exe = std::env::current_exe().ok()?;
    let path = installed_daemon_path(&exe);
    if !path.is_file() {
        return None;
    }
    let mut child = std::process::Command::new(&path)
        .arg("--version")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(50))
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    let out = child.wait_with_output().ok()?;
    parse_daemon_version_line(&String::from_utf8_lossy(&out.stdout))
}

fn installed_daemon_path(exe: &std::path::Path) -> std::path::PathBuf {
    let s = exe.to_string_lossy();
    match s.strip_suffix(" (deleted)") {
        Some(p) => std::path::PathBuf::from(p),
        None => exe.to_path_buf(),
    }
}

/// `k2-daemon 0.46.5` → `0.46.5`.
fn parse_daemon_version_line(out: &str) -> Option<String> {
    let line = out.lines().next()?.trim();
    let v = line.strip_prefix("k2-daemon ")?.trim();
    k2_root_proto::version::Semver::parse(v).map(|sv| sv.to_string())
}

#[cfg(test)]
mod update_version_tests {
    use super::*;

    fn code(r: CliResponse) -> String {
        let v: Value = serde_json::from_str(&r.body).expect("json error body");
        v["error"]["code"].as_str().expect("error code").to_string()
    }

    #[test]
    fn default_target_is_the_running_daemon() {
        assert_eq!(update_target("0.46.4", None, false, Some("0.46.5")).ok(), Some("0.46.4".into()));
    }

    #[test]
    fn version_flag_picks_that_release() {
        assert_eq!(update_target("0.46.4", Some("0.46.5"), false, None).ok(), Some("0.46.5".into()));
        let bad = update_target("0.46.4", Some("v0.46.5"), false, None).err().expect("refused");
        assert_eq!(bad.status, "400 Bad Request");
        assert_eq!(code(bad), "bad_version");
    }

    #[test]
    fn match_installed_reads_the_disk_daemon() {
        assert_eq!(update_target("0.46.4", None, true, Some("0.46.5")).ok(), Some("0.46.5".into()));
        let unknown = update_target("0.46.4", None, true, None).err().expect("refused");
        assert_eq!(code(unknown), "installed_version_unknown");
        let both = update_target("0.46.4", Some("0.46.5"), true, Some("0.46.5")).err().expect("refused");
        assert_eq!(code(both), "bad_request");
    }

    /// The sewlab case: `update` before restarting onto a swapped binary
    /// fetched the old helper silently. Now it warns, naming the fix.
    #[test]
    fn warns_when_installed_or_staged_daemon_is_newer() {
        let w = newer_daemon_warnings("0.46.4", Some("0.46.5"), None);
        assert_eq!(w.len(), 1, "{w:?}");
        assert!(w[0].contains("installed daemon binary is v0.46.5"), "{}", w[0]);
        assert!(w[0].contains("k2 root-helper update --version 0.46.5"), "{}", w[0]);

        let w = newer_daemon_warnings("0.46.4", Some("0.46.4"), Some("0.46.6"));
        assert_eq!(w.len(), 1, "{w:?}");
        assert!(w[0].contains("staged daemon update is v0.46.6"), "{}", w[0]);

        assert!(newer_daemon_warnings("0.46.5", Some("0.46.5"), Some("0.46.4")).is_empty());
        assert!(newer_daemon_warnings("0.46.5", None, None).is_empty());
    }

    #[test]
    fn installed_path_strips_the_deleted_marker() {
        assert_eq!(
            installed_daemon_path(std::path::Path::new("/opt/k2/k2-daemon (deleted)")),
            std::path::PathBuf::from("/opt/k2/k2-daemon")
        );
        assert_eq!(
            installed_daemon_path(std::path::Path::new("/opt/k2/k2-daemon")),
            std::path::PathBuf::from("/opt/k2/k2-daemon")
        );
    }

    #[test]
    fn parses_the_daemon_version_line() {
        assert_eq!(parse_daemon_version_line("k2-daemon 0.46.5\n").as_deref(), Some("0.46.5"));
        assert_eq!(parse_daemon_version_line("something else"), None);
        assert_eq!(parse_daemon_version_line(""), None);
    }
}
