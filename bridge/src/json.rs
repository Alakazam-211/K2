//! A call's JSON, for a host's `--json`: a refusal is the kit's report, whole, with `code` (the
//! row's name) after `id`, and the echo under `on`; an answer is the echo, `dry_run` and the
//! value. A host bug has no row, so no `id` or `code`: its `exit` is null, for the host to give
//! its own, and `host_bug` says what it was.
//!
//! ```text
//! {"id": "MPC-E830", "code": "conflict", "exit": 1, "message": …, "next": …, "next_step": …,
//!  "rule": …, "details": {…}, "on": {"kind": "k5board", "path": …, "version": …, "mode": "file"}}
//! {"on": {…}, "dry_run": false, "value": …}
//! ```
//!
//! Board values in either come through [`crate::value_out`] where they were made, so they print
//! as `read` prints them.

use crate::answer::{Answer, Refused, Why};
use k2_mpc_base::errors::Value as KValue;
use k2_mpc_board::On;

/// An answer as JSON: the echo, whether it was a dry run, and the value.
pub fn answer_json(a: &Answer) -> KValue {
    KValue::obj([
        ("on", on_json(&a.on)),
        ("dry_run", KValue::Bool(a.dry_run)),
        ("value", a.value.clone()),
    ])
}

/// A refusal as JSON: the kit's report with `code` and the echo, or a host bug with the echo.
pub fn refused_json(r: &Refused) -> KValue {
    let mut out: Vec<(String, KValue)> = Vec::new();
    match &r.why {
        Why::Report(report) => {
            if let KValue::Obj(pairs) = report.to_value() {
                for (k, v) in pairs {
                    let id = k == "id";
                    out.push((k, v));
                    if id {
                        out.push(("code".into(), KValue::from(report.entry().name)));
                    }
                }
            }
        }
        Why::HostBug(bug) => {
            out.push(("exit".into(), KValue::Null));
            out.push(("host_bug".into(), KValue::from(bug.to_string())));
        }
    }
    out.push(("on".into(), on_json(&r.on)));
    KValue::Obj(out)
}

/// The echo: which kind of file, the host's path, the guard (null before the file was read),
/// and the mode.
fn on_json(on: &On) -> KValue {
    KValue::obj([
        ("kind", KValue::from(on.kind)),
        ("path", KValue::from(on.path.as_str())),
        (
            "version",
            on.version.as_deref().map_or(KValue::Null, KValue::from),
        ),
        ("mode", KValue::from(on.mode.as_str())),
    ])
}
