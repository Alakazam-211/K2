//! `_compute_render get`: one job, for people.

use super::super::lines::{py_line, LIB};
use super::super::pyrt::{self as p, s};
use super::{ago, dur, Cx};
use crate::families::Raise;
use crate::out::json::Value;
use k2_client::cli_name;

macro_rules! ln {
    ($m:literal) => {
        const { py_line(LIB, "\n_compute_render() {", $m) }
    };
}

fn empty() -> Value {
    Value::Object(Vec::new())
}

fn list() -> Value {
    Value::Array(Vec::new())
}

/// `x or {}`.
fn or_dict(x: Value) -> Value {
    p::or(x, empty())
}

pub fn get(cx: &mut Cx, j: &Value, full: bool) -> Result<(), Raise> {
    let g = |cx: &Cx, line: u32, k: &str| cx.at(line, p::get(j, k));
    let i = |cx: &Cx, line: u32, k: &str| cx.at(line, p::idx(j, k));

    // 0.46.17: an auto job held on this server, on no node yet.
    let l = ln!("if j.get(\"state\") == \"queued\" and (j.get(\"queue\") or {}).get(\"held\"):");
    let held = p::eq_s(&g(cx, l, "state")?, "queued") && {
        let q = or_dict(g(cx, l, "queue")?);
        p::truthy(&cx.at(l, p::get(&q, "held"))?)
    };
    if held {
        let l = ln!("print(\"job %s: held on this server (auto)\"");
        let short = i(cx, l, "short")?;
        let t = cx.at(l, p::pct("job %s: held on this server (auto)", &[short]))?;
        cx.print(&t);
    } else {
        let l = ln!("print(\"job %s on %s: %s%s\"");
        let short = i(cx, l, "short")?;
        let node = g(cx, l, "node")?;
        let state = i(cx, l, "state")?;
        let reason = if p::truthy(&g(cx, l, "reason")?) {
            let r = i(cx, l, "reason")?;
            cx.at(l, p::pct(" (%s)", &[r]))?
        } else {
            String::new()
        };
        let t = cx.at(
            l,
            p::pct("job %s on %s: %s%s", &[short, node, state, s(&reason)]),
        )?;
        cx.print(&t);
    }

    let l = ln!("argv = \" \".join(j.get(\"argv\") or [])");
    let argv = p::or(g(cx, l, "argv")?, list());
    let argv = cx.at(l, p::join(" ", &argv))?;
    let n = argv.chars().count();
    if full || n <= 160 {
        cx.print(&format!("  argv: {argv}"));
    } else {
        let head: String = argv.chars().take(120).collect();
        cx.print(&format!(
            "  argv: {head}… ({n} chars; --full for all of it)"
        ));
    }

    let ctl = or_dict(g(cx, ln!("ctl = j.get(\"ctl\") or {}"), "ctl")?);
    let l = ln!("if j.get(\"tierAsked\") and j.get(\"tierAsked\") != j.get(\"tier\"):");
    let asked = g(cx, l, "tierAsked")?;
    if p::truthy(&asked) && !p::eq(&asked, &g(cx, l, "tier")?) {
        let l = ln!("print(\"  tier %s (asked %s: downgraded)\"");
        let tier = p::or(g(cx, l, "tier")?, s("normal"));
        let a = i(cx, l, "tierAsked")?;
        let t = cx.at(l, p::pct("  tier %s (asked %s: downgraded)", &[tier, a]))?;
        cx.print(&t);
    } else {
        let l = ln!("elif j.get(\"tier\") and j[\"tier\"] != \"normal\":");
        if p::truthy(&g(cx, l, "tier")?) && !p::eq_s(&i(cx, l, "tier")?, "normal") {
            let l = ln!("print(\"  tier %s\" % j[\"tier\"])");
            let t = i(cx, l, "tier")?;
            let t = cx.at(l, p::pct("  tier %s", &[t]))?;
            cx.print(&t);
        }
    }
    let l = ln!("if ctl.get(\"why\"):");
    if p::truthy(&cx.at(l, p::get(&ctl, "why"))?) {
        let l = ln!("print(\"  why (Priority): %s\" % ctl[\"why\"])");
        let w = cx.at(l, p::idx(&ctl, "why"))?;
        let t = cx.at(l, p::pct("  why (Priority): %s", &[w]))?;
        cx.print(&t);
    }
    if p::truthy(&cx.at(
        ln!("if ctl.get(\"memDefaulted\"):"),
        p::get(&ctl, "memDefaulted"),
    )?) {
        cx.print("  memory: the default reservation (no --mem); set --mem next time");
    }
    if p::truthy(&g(cx, ln!("if j.get(\"tag\"):"), "tag")?) {
        let l = ln!("print(\"  tag: %s\" % j[\"tag\"])");
        let t = i(cx, l, "tag")?;
        let t = cx.at(l, p::pct("  tag: %s", &[t]))?;
        cx.print(&t);
    }
    let by = g(cx, ln!("by = j.get(\"submittedBy\")"), "submittedBy")?;
    let l = ln!("if by or j.get(\"submitter\"):");
    if p::truthy(&by) || p::truthy(&g(cx, l, "submitter")?) {
        let l = ln!("print(\"  submitted by: %s%s%s\"");
        let as_ = if p::truthy(&g(cx, l, "submitter")?) {
            let x = i(cx, l, "submitter")?;
            cx.at(l, p::pct(" as %s", &[x]))?
        } else {
            String::new()
        };
        let via = if p::eq_s(&g(cx, l, "via")?, "owner_token") {
            " (owner token: no passport in that shell)"
        } else {
            ""
        };
        let t = cx.at(
            l,
            p::pct(
                "  submitted by: %s%s%s",
                &[p::or(by, s("-")), s(&as_), s(via)],
            ),
        )?;
        cx.print(&t);
    }
    if p::truthy(&g(cx, ln!("if j.get(\"supersedeKey\"):"), "supersedeKey")?) {
        let l = ln!("print(\"  supersede key: %s\" % j[\"supersedeKey\"])");
        let x = i(cx, l, "supersedeKey")?;
        let t = cx.at(l, p::pct("  supersede key: %s", &[x]))?;
        cx.print(&t);
    }
    if p::truthy(&g(cx, ln!("if j.get(\"secrets\"):"), "secrets")?) {
        let l = ln!("print(\"  secrets (env names): %s\"");
        let x = i(cx, l, "secrets")?;
        let x = cx.at(l, p::join(", ", &x))?;
        cx.print(&format!("  secrets (env names): {x}"));
    }
    let nw = g(cx, ln!("nw = j.get(\"network\")"), "network")?;
    if p::is_dict(&nw) {
        let l = ln!("print(\"  network: %s (asked by the job: loopback%s only");
        let line = cx.at(l, p::get(&nw, "line"))?;
        let dummies = if p::truthy(&cx.at(l, p::get(&nw, "dummies"))?) {
            " and its dummy interfaces"
        } else {
            ""
        };
        let t = cx.at(
            l,
            p::pct(
                "  network: %s (asked by the job: loopback%s only, whatever the node setting)",
                &[line, s(dummies)],
            ),
        )?;
        cx.print(&t);
    }
    // 0.46.18: the job may make user namespaces (`--userns`).
    if p::truthy(&g(cx, ln!("if j.get(\"userns\"):"), "userns")?) {
        cx.print("  userns:   allowed (--userns, granted by the node's owner)");
    }
    if p::truthy(&g(cx, ln!("if j.get(\"noAddrFilter\"):"), "noAddrFilter")?) {
        cx.print("  address filter: off (--no-addr-filter, granted by the node's owner)");
    }
    let l = ln!("pl = j.get(\"placement\") or ctl.get(\"placement\") or {}");
    let mut pl = g(cx, l, "placement")?;
    if !p::truthy(&pl) {
        pl = cx.at(l, p::get(&ctl, "placement"))?;
    }
    let pl = or_dict(pl);
    let l = ln!("if pl.get(\"mode\") == \"auto\":");
    if p::eq_s(&cx.at(l, p::get(&pl, "mode"))?, "auto") {
        let l = ln!("code = pl.get(\"code\") or \"\"");
        let code = p::or(cx.at(l, p::get(&pl, "code"))?, s(""));
        let l = ln!("how = (\"moved: \" + code[len(\"moved_\"):]) if code.startswith(\"moved_\") else \"auto\"");
        let how = if cx.at(l, p::startswith(&code, "moved_"))? {
            let rest = cx.at(l, p::slice(&code, Some(6), None))?;
            p::str_of(&cx.at(l, p::add(&s("moved: "), &rest))?)
        } else {
            "auto".to_string()
        };
        let l = ln!("print(\"  placed on %s (%s, %s): %s%s%s\"");
        let pn = cx.at(l, p::get(&pl, "node"))?;
        let policy = cx.at(l, p::get(&pl, "policy"))?;
        let why = cx.at(l, p::get(&pl, "why"))?;
        let fits = if p::truthy(&cx.at(l, p::get(&pl, "fits"))?) {
            let f = cx.at(l, p::idx(&pl, "fits"))?;
            p::str_of(&cx.at(l, p::add(&s(" · "), &f))?)
        } else {
            String::new()
        };
        let avoid = if p::truthy(&g(cx, l, "avoid")?) {
            let a = i(cx, l, "avoid")?;
            format!(" · avoids {}", cx.at(l, p::join(", ", &a))?)
        } else {
            String::new()
        };
        let t = cx.at(
            l,
            p::pct(
                "  placed on %s (%s, %s): %s%s%s",
                &[pn, s(&how), policy, why, s(&fits), s(&avoid)],
            ),
        )?;
        cx.print(&t);
    }
    let l = ln!("if (j.get(\"move\") or {}).get(\"note\"):");
    let mv = or_dict(g(cx, l, "move")?);
    if p::truthy(&cx.at(l, p::get(&mv, "note"))?) {
        let l = ln!("print(\"  move: %s\" % j[\"move\"][\"note\"])");
        let m = i(cx, l, "move")?;
        let n = cx.at(l, p::idx(&m, "note"))?;
        let t = cx.at(l, p::pct("  move: %s", &[n]))?;
        cx.print(&t);
    }

    // The times.
    let hms = |cx: &mut Cx, line: u32, t: &Value| -> Result<String, Raise> {
        cx.call(line, "<lambda>", |cx| {
            let now = cx.now();
            cx.at(
                ln!("hms = lambda t: time.strftime(\"%m-%d %H:%M:%S\""),
                p::strftime("%m-%d %H:%M:%S", t, now),
            )
        })
    };
    let mut times: Vec<String> = Vec::new();
    let l = ln!("times = [\"queued %s (%s)\"");
    if p::truthy(&g(cx, l, "createdAt")?) {
        let c = i(cx, l, "createdAt")?;
        let a = hms(cx, l, &c)?;
        let b = ago(cx, l, &c)?;
        times.push(format!("queued {a} ({b})"));
    }
    let l = ln!("    if j.get(\"startedAt\"):\n        times.append");
    if p::truthy(&g(cx, l, "startedAt")?) {
        let l = ln!("times.append(\"started %s (%s)\"");
        let c = i(cx, l, "startedAt")?;
        let a = hms(cx, l, &c)?;
        let b = ago(cx, l, &c)?;
        times.push(format!("started {a} ({b})"));
    } else {
        let l = ln!("elif j.get(\"state\") not in (\"queued\", \"assigned\", \"preparing\"):");
        let st = g(cx, l, "state")?;
        if !["queued", "assigned", "preparing"]
            .iter()
            .any(|x| p::eq_s(&st, x))
        {
            times.push("never started".into());
        }
    }
    if p::truthy(&g(cx, ln!("if j.get(\"endedAt\"):"), "endedAt")?) {
        let l = ln!("times.append(\"ended %s (%s)\"");
        let c = i(cx, l, "endedAt")?;
        let a = hms(cx, l, &c)?;
        let b = ago(cx, l, &c)?;
        times.push(format!("ended {a} ({b})"));
    }
    if !times.is_empty() {
        cx.print(&format!("  {}", times.join(" · ")));
    }
    let l = ln!("print(\"  commit: %s  tree: %s\"");
    let mut commit = g(cx, l, "srcCommit")?;
    if !p::truthy(&commit) {
        commit = g(cx, l, "submitCommit")?;
    }
    let tree = p::or(g(cx, l, "srcTree")?, s("-"));
    let t = cx.at(
        l,
        p::pct("  commit: %s  tree: %s", &[p::or(commit, s("-")), tree]),
    )?;
    cx.print(&t);
    let l = ln!("print(\"  exit: %s  signal: %s  attempt %s (generation %s)\"");
    let args = [
        g(cx, l, "exitCode")?,
        g(cx, l, "signal")?,
        g(cx, l, "attempt")?,
        g(cx, l, "generation")?,
    ];
    let t = cx.at(
        l,
        p::pct("  exit: %s  signal: %s  attempt %s (generation %s)", &args),
    )?;
    cx.print(&t);
    // --expect: what a finished job took, and the value to pass next time.
    let l = ln!("if j.get(\"state\") == \"done\" and j.get(\"took\"):");
    if p::eq_s(&g(cx, l, "state")?, "done") && p::truthy(&g(cx, l, "took")?) {
        let l = ln!("print(\"  %s\" % j[\"took\"])");
        let took = i(cx, l, "took")?;
        let t = cx.at(l, p::pct("  %s", &[took]))?;
        cx.print(&t);
    }

    attempts(cx, j)?;
    queue_lines(cx, j)?;
    rest(cx, j)
}

fn attempts(cx: &mut Cx, j: &Value) -> Result<(), Raise> {
    let l = ln!("rows = j.get(\"attempts\") or []");
    let rows = p::or(cx.at(l, p::get(j, "attempts"))?, list());
    let l = ln!("if len(rows) > 1:");
    if cx.at(l, p::len(&rows))? <= 1 {
        return Ok(());
    }
    let n = cx.at(l, p::len(&rows))?;
    cx.print(&format!("  attempts ({n}):"));
    let hm = |cx: &mut Cx, line: u32, t: &Value| -> Result<String, Raise> {
        cx.call(line, "<lambda>", |cx| {
            if !p::truthy(t) {
                return Ok("?".to_string());
            }
            let now = cx.now();
            cx.at(
                ln!("hm = lambda t: time.strftime(\"%m-%d %H:%M\""),
                p::strftime("%m-%d %H:%M", t, now),
            )
        })
    };
    let l = ln!("        for a in rows:");
    for a in cx.at(l, p::iter(&rows))? {
        let ga = |cx: &Cx, line: u32, k: &str| cx.at(line, p::get(&a, k));
        let l = ln!("when = (\"%s to %s\" % (hm(a.get(\"startedAt\"))");
        let when = if p::truthy(&ga(cx, l, "startedAt")?) {
            let st = ga(cx, l, "startedAt")?;
            let x = hm(cx, l, &st)?;
            let y = if p::truthy(&ga(cx, l, "current")?) && !p::truthy(&ga(cx, l, "endedAt")?) {
                "now".to_string()
            } else {
                let e = ga(cx, l, "endedAt")?;
                hm(cx, l, &e)?
            };
            format!("{x} to {y}")
        } else {
            "never started".to_string()
        };
        let l = ln!("ended = (\"%s/%s\" % (a[\"state\"], a[\"reason\"]) if a.get(\"reason\") else a[\"state\"])");
        let ended = if p::truthy(&ga(cx, l, "reason")?) {
            let st = cx.at(l, p::idx(&a, "state"))?;
            let r = cx.at(l, p::idx(&a, "reason"))?;
            s(&cx.at(l, p::pct("%s/%s", &[st, r]))?)
        } else {
            cx.at(l, p::idx(&a, "state"))?
        };
        let l = ln!("line = \"    attempt %s · generation %s · %s · %s%s\"");
        let at = cx.at(l, p::idx(&a, "attempt"))?;
        let gen = cx.at(l, p::idx(&a, "generation"))?;
        let node = p::or(ga(cx, l, "node")?, s("?"));
        let this = if p::truthy(&ga(cx, l, "current")?) {
            "  (this one)"
        } else {
            ""
        };
        let line = cx.at(
            l,
            p::pct(
                "    attempt %s · generation %s · %s · %s%s",
                &[at, gen, node, s(&when), s(this)],
            ),
        )?;
        cx.print(&line);
        let l = ln!("if a.get(\"why\") and not a.get(\"current\"):");
        if p::truthy(&ga(cx, l, "why")?) && !p::truthy(&ga(cx, l, "current")?) {
            let l = ln!("print(\"      %s: %s\" % (ended, a[\"why\"]))");
            let w = cx.at(l, p::idx(&a, "why"))?;
            let t = cx.at(l, p::pct("      %s: %s", &[ended.clone(), w]))?;
            cx.print(&t);
        }
        if p::truthy(&ga(cx, ln!("if a.get(\"next\"):"), "next")?) {
            let l = ln!("print(\"      then: %s\" % a[\"next\"])");
            let x = cx.at(l, p::idx(&a, "next"))?;
            let t = cx.at(l, p::pct("      then: %s", &[x]))?;
            cx.print(&t);
        }
        let l = ln!("lg, ar = a.get(\"log\") or {}, a.get(\"artifacts\") or {}");
        let lg = or_dict(ga(cx, l, "log")?);
        let ar = or_dict(ga(cx, l, "artifacts")?);
        let l = ln!("if not a.get(\"current\") and lg.get(\"available\"):");
        if !p::truthy(&ga(cx, l, "current")?) && p::truthy(&cx.at(l, p::get(&lg, "available"))?) {
            let l = ln!("print(\"      log: %d KB (");
            let b = p::or(cx.at(l, p::get(&lg, "bytes"))?, p::int(0));
            let kb = cx.at(l, p::floordiv(&b, &p::int(1024)))?;
            let short = cx.at(l, p::idx(j, "short"))?;
            let at = cx.at(l, p::idx(&a, "attempt"))?;
            let t = cx.at(
                l,
                p::pct(
                    concat!(
                        "      log: %d KB (",
                        cli_name!(),
                        " compute logs %s --attempt %s)"
                    ),
                    &[kb, short, at],
                ),
            )?;
            cx.print(&t);
        }
        let l = ln!("if not a.get(\"current\") and ar.get(\"status\") in (\"kept\", \"not_collected\", \"none_matched\"):");
        let current = p::truthy(&ga(cx, l, "current")?);
        let listed = !current && {
            let status = cx.at(l, p::get(&ar, "status"))?;
            ["kept", "not_collected", "none_matched"]
                .iter()
                .any(|x| p::eq_s(&status, x))
        };
        if listed {
            let l = ln!("print(\"      artifacts: %s%s\"");
            let st = cx.at(l, p::idx(&ar, "status"))?;
            let kept = p::eq_s(&st, "kept");
            let what = if kept {
                let files = p::or(cx.at(l, p::get(&ar, "files"))?, list());
                let nf = cx.at(l, p::len(&files))?;
                let short = cx.at(l, p::idx(j, "short"))?;
                let at = cx.at(l, p::idx(&a, "attempt"))?;
                cx.at(
                    l,
                    p::pct(
                        concat!(
                            "%d file(s) (",
                            cli_name!(),
                            " compute fetch %s --attempt %s)"
                        ),
                        &[p::int(nf as i128), short, at],
                    ),
                )?
            } else {
                cx.at(l, p::replace(&st, "_", " "))?
            };
            let note = if p::truthy(&cx.at(l, p::get(&ar, "note"))?) && !kept {
                let n = cx.at(l, p::idx(&ar, "note"))?;
                p::str_of(&cx.at(l, p::add(&s(" · "), &n))?)
            } else {
                String::new()
            };
            let t = cx.at(l, p::pct("      artifacts: %s%s", &[s(&what), s(&note)]))?;
            cx.print(&t);
        }
    }
    Ok(())
}

fn queue_lines(cx: &mut Cx, j: &Value) -> Result<(), Raise> {
    let g = |cx: &Cx, line: u32, k: &str| cx.at(line, p::get(j, k));
    let l = ln!("if j.get(\"queue\") and j[\"queue\"].get(\"onNode\"):");
    let q0 = g(cx, l, "queue")?;
    let on_node = p::truthy(&q0) && {
        let q = cx.at(l, p::idx(j, "queue"))?;
        p::truthy(&cx.at(l, p::get(&q, "onNode"))?)
    };
    let held = !on_node && {
        let l = ln!("elif j.get(\"queue\") and j[\"queue\"].get(\"held\"):");
        p::truthy(&g(cx, l, "queue")?) && {
            let q = cx.at(l, p::idx(j, "queue"))?;
            p::truthy(&cx.at(l, p::get(&q, "held"))?)
        }
    };
    if on_node {
        let q = cx.at(
            ln!("        q = j[\"queue\"]\n        print(\"  %s%s\""),
            p::idx(j, "queue"),
        )?;
        let l = ln!("print(\"  %s%s\" % (q[\"message\"], (\"  [class %s]\"");
        let m = cx.at(l, p::idx(&q, "message"))?;
        let class = if p::truthy(&g(cx, l, "class")?) {
            let c = cx.at(l, p::idx(j, "class"))?;
            cx.at(l, p::pct("  [class %s]", &[c]))?
        } else {
            String::new()
        };
        let t = cx.at(l, p::pct("  %s%s", &[m, s(&class)]))?;
        cx.print(&t);
        let l = ln!("if q.get(\"reason\") == \"pull_requested\":");
        let l2 = ln!("elif q.get(\"reason\") and q[\"reason\"] != \"node_queued\":");
        if p::eq_s(&cx.at(l, p::get(&q, "reason"))?, "pull_requested") {
            // 0.46.17: the node was asked to give it back.
            cx.print(concat!(
                "  waiting for the node to give it back (",
                cli_name!(),
                " compute move); it is placed again then"
            ));
        } else if p::truthy(&cx.at(l2, p::get(&q, "reason"))?)
            && !p::eq_s(&cx.at(l2, p::idx(&q, "reason"))?, "node_queued")
        {
            let l = ln!("print(\"  waiting for the node to answer: %s\"");
            let r = cx.at(l, p::idx(&q, "reason"))?;
            let t = cx.at(l, p::pct("  waiting for the node to answer: %s", &[r]))?;
            cx.print(&t);
        }
    } else if held {
        // 0.46.17: held on this server; the daemon's words name the
        // nodes it fits.
        let l = ln!("print(\"  %s\" % j[\"queue\"][\"line\"])");
        let q = cx.at(l, p::idx(j, "queue"))?;
        let line = cx.at(l, p::idx(&q, "line"))?;
        let t = cx.at(l, p::pct("  %s", &[line]))?;
        cx.print(&t);
    } else if p::truthy(&g(cx, ln!("    elif j.get(\"queue\"):"), "queue")?) {
        let q = cx.at(
            ln!("        q = j[\"queue\"]\n        at ="),
            p::idx(j, "queue"),
        )?;
        let l = ln!("at = q.get(\"aheadTiers\") or {}");
        let at = or_dict(cx.at(l, p::get(&q, "aheadTiers"))?);
        let l = ln!("behind = \", \".join(x for x in");
        let mut parts = Vec::new();
        if p::truthy(&cx.at(l, p::get(&at, "ci"))?) {
            let x = cx.at(l, p::idx(&at, "ci"))?;
            parts.push(cx.at(l, p::pct("%d CI", &[x]))?);
        }
        if p::truthy(&cx.at(l, p::get(&at, "priority"))?) {
            let x = cx.at(l, p::idx(&at, "priority"))?;
            parts.push(cx.at(l, p::pct("%d Priority", &[x]))?);
        }
        let behind = parts.join(", ");
        let l = ln!("        if q.get(\"line\"):\n            # 0.46.6");
        if p::truthy(&cx.at(l, p::get(&q, "line"))?) {
            let l = ln!("print(\"  %s%s\" % (q[\"line\"], (\"  [behind %s]\"");
            let x = cx.at(l, p::idx(&q, "line"))?;
            let b = if behind.is_empty() {
                String::new()
            } else {
                format!("  [behind {behind}]")
            };
            let t = cx.at(l, p::pct("  %s%s", &[x, s(&b)]))?;
            cx.print(&t);
        } else {
            let l = ln!("print(\"  queued: position %s%s%s");
            let pos = cx.at(l, p::idx(&q, "position"))?;
            let of = if p::truthy(&cx.at(l, p::get(&q, "of"))?) {
                let x = cx.at(l, p::idx(&q, "of"))?;
                cx.at(l, p::pct(" of %s", &[x]))?
            } else {
                String::new()
            };
            let b = if behind.is_empty() {
                String::new()
            } else {
                format!(", behind {behind}")
            };
            let reason = cx.at(l, p::idx(&q, "reason"))?;
            let message = cx.at(l, p::idx(&q, "message"))?;
            let t = cx.at(
                l,
                p::pct(
                    "  queued: position %s%s%s — %s (%s)",
                    &[pos, s(&of), s(&b), reason, message],
                ),
            )?;
            cx.print(&t);
        }
        let l = ln!("if q.get(\"parallel\"):");
        if p::truthy(&cx.at(l, p::get(&q, "parallel"))?) {
            let l = ln!("print(\"  the node runs %s job(s) from this workspace at a time\"");
            let x = cx.at(l, p::idx(&q, "parallel"))?;
            let t = cx.at(
                l,
                p::pct(
                    "  the node runs %s job(s) from this workspace at a time",
                    &[x],
                ),
            )?;
            cx.print(&t);
        }
        let l = ln!("for a in q.get(\"ahead\") or []:");
        let ahead = cx.at(l, p::get(&q, "ahead"))?;
        for a in cx.at(l, p::iter_or_empty(ahead))? {
            let l = ln!("age = a.get(\"elapsedSecs\") if a.get(\"elapsedSecs\") is not None else a.get(\"ageSecs\")");
            let e = cx.at(l, p::get(&a, "elapsedSecs"))?;
            let age = if p::is_none(&e) {
                cx.at(l, p::get(&a, "ageSecs"))?
            } else {
                e
            };
            let est = cx.at(ln!("est = a.get(\"estSecs\")"), p::get(&a, "estSecs"))?;
            let l = ln!("print(\"  ahead: %-9s %-9s %-14s %s%s  %s%s\"");
            let short = p::or(cx.at(l, p::get(&a, "short"))?, s("-"));
            let state = cx.at(l, p::get(&a, "state"))?;
            let ws = p::or(cx.at(l, p::get(&a, "workspace"))?, s("(other)"));
            let el = if p::is_none(&age) {
                "-".to_string()
            } else {
                let m = cx.at(l, p::floordiv(&age, &p::int(60)))?;
                let sec = cx.at(l, p::rem(&age, &p::int(60)))?;
                cx.at(l, p::pct("%dm%02ds", &[m, sec]))?
            };
            // --expect: the run time K2 expects of each job ahead.
            let est = if p::is_none(&est) {
                String::new()
            } else {
                let x = cx.at(l, p::add(&est, &p::int(59)))?;
                let x = cx.at(l, p::floordiv(&x, &p::int(60)))?;
                let m = cx.at(l, p::max2(&p::int(1), &x))?;
                cx.at(l, p::pct(" ~%dm", &[m]))?
            };
            let argv = p::or(cx.at(l, p::get(&a, "argv"))?, s(""));
            let reason = if p::truthy(&cx.at(l, p::get(&a, "reason"))?) {
                let r = cx.at(l, p::idx(&a, "reason"))?;
                let x = cx.at(l, p::add(&s("  ("), &r))?;
                p::str_of(&cx.at(l, p::add(&x, &s(")")))?)
            } else {
                String::new()
            };
            let t = cx.at(
                l,
                p::pct(
                    "  ahead: %-9s %-9s %-14s %s%s  %s%s",
                    &[short, state, ws, s(&el), s(&est), argv, s(&reason)],
                ),
            )?;
            cx.print(&t);
        }
    }
    // The wait.
    let wt = or_dict(g(cx, ln!("wt = j.get(\"wait\") or {}"), "wait")?);
    let l = ln!("if wt and not j.get(\"queue\") and wt.get(\"words\"):");
    if p::truthy(&wt)
        && !p::truthy(&g(cx, l, "queue")?)
        && p::truthy(&cx.at(l, p::get(&wt, "words"))?)
    {
        let l = ln!("print(\"  %s%s\" % (wt[\"words\"], (\", ~%s left (%s)\"");
        let words = cx.at(l, p::idx(&wt, "words"))?;
        let eta = if !p::is_none(&cx.at(l, p::get(&wt, "etaSecs"))?) {
            let e = cx.at(l, p::idx(&wt, "etaSecs"))?;
            let d = dur(cx, l, &e)?;
            let basis = cx.at(l, p::get(&wt, "etaBasis"))?;
            cx.at(l, p::pct(", ~%s left (%s)", &[s(&d), basis]))?
        } else {
            String::new()
        };
        let t = cx.at(l, p::pct("  %s%s", &[words, s(&eta)]))?;
        cx.print(&t);
    } else {
        let l = ln!("elif wt.get(\"etaSecs\") is not None and j.get(\"queue\") and not j[\"queue\"].get(\"onNode\"):");
        if !p::is_none(&cx.at(l, p::get(&wt, "etaSecs"))?) && p::truthy(&g(cx, l, "queue")?) {
            let q = cx.at(l, p::idx(j, "queue"))?;
            if !p::truthy(&cx.at(l, p::get(&q, "onNode"))?) {
                let l = ln!("print(\"  ETA ~%s (estimate from earlier runs)\"");
                let e = cx.at(l, p::idx(&wt, "etaSecs"))?;
                let d = dur(cx, l, &e)?;
                cx.print(&format!("  ETA ~{d} (estimate from earlier runs)"));
            }
        }
    }
    Ok(())
}

fn rest(cx: &mut Cx, j: &Value) -> Result<(), Raise> {
    let g = |cx: &Cx, line: u32, k: &str| cx.at(line, p::get(j, k));
    // --expect: how long K2 expects an unfinished job to run.
    let ex = g(cx, ln!("ex = j.get(\"expected\")"), "expected")?;
    let l = ln!("if isinstance(ex, dict) and ex.get(\"words\") and j.get(\"state\") not in");
    if p::is_dict(&ex) && p::truthy(&cx.at(l, p::get(&ex, "words"))?) {
        let st = g(cx, l, "state")?;
        let ended = ["done", "failed", "cancelled", "timeout", "interrupted", "unknown"]
            .iter()
            .any(|x| p::eq_s(&st, x));
        if !ended {
            let l = ln!("print(\"  expected: %s\" % ex[\"words\"])");
            let w = cx.at(l, p::idx(&ex, "words"))?;
            let t = cx.at(l, p::pct("  expected: %s", &[w]))?;
            cx.print(&t);
        }
    }
    let stl = or_dict(g(cx, ln!("stl = j.get(\"stall\") or {}"), "stall")?);
    let l = ln!("if \"stallSecs\" in stl:");
    if cx.at(l, p::has(&stl, "stallSecs"))? {
        let l = ln!("print(\"  stall watch: %s\"");
        let ss = cx.at(l, p::get(&stl, "stallSecs"))?;
        let what = if p::truthy(&ss) {
            let x = cx.at(l, p::idx(&stl, "stallSecs"))?;
            let d = dur(cx, l, &x)?;
            let warned = if p::truthy(&cx.at(l, p::get(&stl, "warned"))?) {
                "; warned"
            } else {
                ""
            };
            format!("ends after {d} of silence (--stall-timeout){warned}")
        } else {
            "off".to_string()
        };
        cx.print(&format!("  stall watch: {what}"));
    }
    let rl = or_dict(g(
        cx,
        ln!("rl = j.get(\"reportedLate\") or {}"),
        "reportedLate",
    )?);
    let l = ln!("if rl.get(\"secs\"):");
    if p::truthy(&cx.at(l, p::get(&rl, "secs"))?) {
        let hm = |cx: &mut Cx, line: u32, t: &Value| -> Result<String, Raise> {
            cx.call(line, "<lambda>", |cx| {
                if !p::truthy(t) {
                    return Ok("?".to_string());
                }
                let now = cx.now();
                cx.at(
                    ln!("hm = lambda t: time.strftime(\"%H:%M\""),
                    p::strftime("%H:%M", t, now),
                )
            })
        };
        let l = ln!("print(\"  the node finished at %s; this server heard at %s");
        let a = cx.at(l, p::get(&rl, "nodeEndedAt"))?;
        let a = hm(cx, l, &a)?;
        let b = cx.at(l, p::get(&rl, "heardAt"))?;
        let b = hm(cx, l, &b)?;
        let secs = cx.at(l, p::idx(&rl, "secs"))?;
        let late = if cx.at(l, p::cmp(">=", &secs, &p::int(3600)))? {
            let h = cx.at(l, p::floordiv(&secs, &p::int(3600)))?;
            let m = cx.at(l, p::rem(&secs, &p::int(3600)))?;
            let m = cx.at(l, p::floordiv(&m, &p::int(60)))?;
            cx.at(l, p::pct("%dh%02dm", &[h, m]))?
        } else {
            let m = cx.at(l, p::floordiv(&secs, &p::int(60)))?;
            cx.at(l, p::pct("%dm", &[m]))?
        };
        cx.print(&format!("  the node finished at {a}; this server heard at {b} ({late} late: it was asleep, or the node was cut off). Durations use the times of the node."));
    }
    if p::truthy(&g(cx, ln!("if j.get(\"hint\"):"), "hint")?) {
        let l = ln!("print(\"  hint: %s\" % j[\"hint\"])");
        let h = cx.at(l, p::idx(j, "hint"))?;
        let t = cx.at(l, p::pct("  hint: %s", &[h]))?;
        cx.print(&t);
    }
    let cb = g(cx, ln!("cb = j.get(\"cancelledBy\")"), "cancelledBy")?;
    if p::is_dict(&cb) {
        let l = ln!("print(\"  cancel asked by %s at %s\"");
        let by = cx.at(l, p::get(&cb, "by"))?;
        let at = p::or(cx.at(l, p::get(&cb, "at"))?, p::int(0));
        let now = cx.now();
        let at = cx.at(l, p::strftime("%H:%M:%S", &at, now))?;
        let t = cx.at(l, p::pct("  cancel asked by %s at %s", &[by, s(&at)]))?;
        cx.print(&t);
    }
    let r = or_dict(g(cx, ln!("r = j.get(\"result\") or {}"), "result")?);
    let rg = |cx: &Cx, line: u32, k: &str| cx.at(line, p::get(&r, k));
    let l = ln!("if j.get(\"reason\") == \"tenant_disk_full\" and r.get(\"detail\"):");
    if p::eq_s(&g(cx, l, "reason")?, "tenant_disk_full") && p::truthy(&rg(cx, l, "detail")?) {
        let l = ln!("print(\"  \" + r[\"detail\"])");
        let d = cx.at(l, p::idx(&r, "detail"))?;
        let t = cx.at(l, p::add(&s("  "), &d))?;
        cx.print(&p::str_of(&t));
    }
    let l = ln!("for n in r.get(\"notes\") or []:");
    let notes = rg(cx, l, "notes")?;
    for n in cx.at(l, p::iter_or_empty(notes))? {
        let l = ln!("if n.startswith(\"evicted \")");
        let keep = cx.at(l, p::startswith(&n, "evicted "))?
            || cx.at(l, p::startswith(&n, "network: no-lan proxy: "))?
            || cx.at(l, p::startswith(&n, "killed "))?;
        if keep {
            let t = cx.at(ln!("            print(\"  \" + n)"), p::add(&s("  "), &n))?;
            cx.print(&p::str_of(&t));
        }
    }
    hosts(cx, j, &r)?;
    let l = ln!("t = r.get(\"tests\")\n    if t:");
    let t = rg(cx, l, "tests")?;
    if p::truthy(&t) {
        let l = ln!("print(\"  tests: %d passed, %d failed, %d errors, %d skipped\"");
        let a = [
            cx.at(l, p::idx(&t, "passed"))?,
            cx.at(l, p::idx(&t, "failed"))?,
            cx.at(l, p::idx(&t, "errors"))?,
            cx.at(l, p::idx(&t, "skipped"))?,
        ];
        let x = cx.at(
            l,
            p::pct("  tests: %d passed, %d failed, %d errors, %d skipped", &a),
        )?;
        cx.print(&x);
    }
    let l = ln!("for a in r.get(\"artifacts\") or []:");
    let arts = rg(cx, l, "artifacts")?;
    for a in cx.at(l, p::iter_or_empty(arts))? {
        let l = ln!("print(\"  artifact: %s  %d bytes  sha256 %s…\"");
        let name = cx.at(l, p::idx(&a, "name"))?;
        let bytes = cx.at(l, p::idx(&a, "bytes"))?;
        let sha = cx.at(l, p::idx(&a, "sha256"))?;
        let sha = cx.at(l, p::slice(&sha, None, Some(12)))?;
        let x = cx.at(
            l,
            p::pct("  artifact: %s  %d bytes  sha256 %s…", &[name, bytes, sha]),
        )?;
        cx.print(&x);
    }
    let l = ln!("if r.get(\"receipt\"):");
    if p::truthy(&rg(cx, l, "receipt")?) {
        let l = ln!("print(\"  receipt: %s (%s)\"");
        let rc = cx.at(l, p::idx(&r, "receipt"))?;
        let id = cx.at(l, p::get(&rc, "id"))?;
        let rc = cx.at(l, p::idx(&r, "receipt"))?;
        let ok = if p::truthy(&cx.at(l, p::get(&rc, "verified"))?) {
            "verified"
        } else {
            "NOT verified"
        };
        let x = cx.at(l, p::pct("  receipt: %s (%s)", &[id, s(ok)]))?;
        cx.print(&x);
    }
    let tl = or_dict(rg(cx, ln!("tl = r.get(\"tools\") or {}"), "tools")?);
    let l = ln!("if tl.get(\"versions\"):");
    if p::truthy(&cx.at(l, p::get(&tl, "versions"))?) {
        let l = ln!("print(\"  tools%s: %s\"");
        let probe = if p::eq_s(&cx.at(l, p::get(&tl, "basis"))?, "node-probe") {
            " (node-wide probe, older k2-node)"
        } else {
            ""
        };
        let vs = cx.at(l, p::idx(&tl, "versions"))?;
        let mut parts = Vec::new();
        for (k, v) in cx.at(l, p::sorted_items(&vs))? {
            parts.push(cx.at(l, p::pct("%s %s", &[k, v]))?);
        }
        cx.print(&format!("  tools{probe}: {}", parts.join(", ")));
    }
    resources(cx, &r)?;
    let hs = or_dict(rg(cx, ln!("hs = r.get(\"host\") or {}"), "host")?);
    if p::truthy(&hs) {
        let l = ln!("print(\"  node at start: %s · at end: %s\"");
        let a = cx.at(l, p::get(&hs, "start"))?;
        let a = cx.call(l, "stamp", |cx| stamp(cx, &a))?;
        let b = cx.at(l, p::get(&hs, "end"))?;
        let b = cx.call(l, "stamp", |cx| stamp(cx, &b))?;
        cx.print(&format!("  node at start: {a} · at end: {b}"));
    }
    let eff = rg(cx, ln!("eff = r.get(\"effective\")"), "effective")?;
    if p::is_dict(&eff) && p::truthy(&eff) {
        let l = ln!("print(\"  effective: %s\" % json.dumps(eff");
        let d = cx.at(l, p::dumps(&eff, true, None))?;
        let d = cx.at(l, p::slice(&s(&d), None, Some(300)))?;
        let x = cx.at(l, p::pct("  effective: %s", &[d]))?;
        cx.print(&x);
    }
    let l = ln!("for x in r.get(\"shards\") or []:");
    let shards = rg(cx, l, "shards")?;
    for x in cx.at(l, p::iter_or_empty(shards))? {
        let t = cx.at(ln!("        t = x.get(\"tests\")"), p::get(&x, "tests"))?;
        let l = ln!("print(\"  shard %s/%d: job %s on %s · %s%s%s\"");
        let index = cx.at(l, p::idx(&x, "index"))?;
        let all = cx.at(l, p::idx(&r, "shards"))?;
        let n = cx.at(l, p::len(&all))?;
        let short = cx.at(l, p::idx(&x, "short"))?;
        let node = p::or(cx.at(l, p::get(&x, "node"))?, s("?"));
        let state = cx.at(l, p::idx(&x, "state"))?;
        let exit = if !p::is_none(&cx.at(l, p::get(&x, "exit"))?) {
            let e = cx.at(l, p::idx(&x, "exit"))?;
            cx.at(l, p::pct(" exit %s", &[e]))?
        } else {
            String::new()
        };
        let tests = if p::truthy(&t) {
            let passed = cx.at(l, p::idx(&t, "passed"))?;
            let failed = cx.at(l, p::idx(&t, "failed"))?;
            let errors = cx.at(l, p::idx(&t, "errors"))?;
            let f = cx.at(l, p::add(&failed, &errors))?;
            cx.at(l, p::pct(" · %d passed, %d failed", &[passed, f]))?
        } else {
            String::new()
        };
        let line = cx.at(
            l,
            p::pct(
                "  shard %s/%d: job %s on %s · %s%s%s",
                &[
                    index,
                    p::int(n as i128),
                    short,
                    node,
                    state,
                    s(&exit),
                    s(&tests),
                ],
            ),
        )?;
        cx.print(&line);
    }
    Ok(())
}

fn hosts(cx: &mut Cx, j: &Value, r: &Value) -> Result<(), Raise> {
    let l = ln!("nh = r.get(\"networkHosts\") or []");
    let nh = p::or(cx.at(l, p::get(r, "networkHosts"))?, list());
    if !p::truthy(&nh) {
        return Ok(());
    }
    let l = ln!("print(\"  hosts through the no-lan proxy (%d allowed, %d refused%s):\"");
    let mut counts = [0i128; 3];
    for (k, verdict) in ["allowed", "refused", "failed"].iter().enumerate() {
        let items = cx.call(l, "<lambda>", |cx| {
            let lc = ln!("cnt = lambda v: sum(1 for h in nh if h.get(\"verdict\") == v)");
            cx.at(lc, p::iter(&nh))
        })?;
        for h in items {
            let v = cx.call(l, "<lambda>", |cx| {
                let lc = ln!("cnt = lambda v: sum(1 for h in nh if h.get(\"verdict\") == v)");
                cx.at(lc, p::get(&h, "verdict"))
            })?;
            if p::eq_s(&v, verdict) {
                counts[k] += 1;
            }
        }
    }
    let failed = if counts[2] > 0 {
        format!(", {} unreachable", counts[2])
    } else {
        String::new()
    };
    cx.print(&format!(
        "  hosts through the no-lan proxy ({} allowed, {} refused{failed}):",
        counts[0], counts[1]
    ));
    let l = ln!("for h in nh[:40]:");
    let first = cx.at(l, p::slice(&nh, None, Some(40)))?;
    for h in cx.at(l, p::iter(&first))? {
        let l = ln!("hp = (\"[%s]:%s\" if \":\" in (h.get(\"host\") or \"\")");
        let host = p::or(cx.at(l, p::get(&h, "host"))?, s(""));
        let fmt = if cx.at(l, p::has(&host, ":"))? {
            "[%s]:%s"
        } else {
            "%s:%s"
        };
        let hv = cx.at(l, p::get(&h, "host"))?;
        let port = cx.at(l, p::get(&h, "port"))?;
        let hp = cx.at(l, p::pct(fmt, &[hv, port]))?;
        let l = ln!("if h.get(\"verdict\") == \"allowed\":");
        if p::eq_s(&cx.at(l, p::get(&h, "verdict"))?, "allowed") {
            let l = ln!("print(\"    allowed  %-36s %4d conn(s)  in %-9s out %s\"");
            let conns = p::or(cx.at(l, p::get(&h, "conns"))?, p::int(0));
            let bi = cx.at(l, p::get(&h, "bytesIn"))?;
            let bi = cx.call(l, "hb", |cx| hb(cx, &bi))?;
            let bo = cx.at(l, p::get(&h, "bytesOut"))?;
            let bo = cx.call(l, "hb", |cx| hb(cx, &bo))?;
            let t = cx.at(
                l,
                p::pct(
                    "    allowed  %-36s %4d conn(s)  in %-9s out %s",
                    &[s(&hp), conns, s(&bi), s(&bo)],
                ),
            )?;
            cx.print(&t);
        } else {
            let l = ln!("why = \", \".join(x for x in [h.get(\"addr\")");
            let addr = cx.at(l, p::get(&h, "addr"))?;
            let host = cx.at(l, p::get(&h, "host"))?;
            let mut why = Vec::new();
            if !p::eq(&addr, &host) && p::truthy(&addr) {
                why.push(addr);
            }
            let reason = cx.at(l, p::get(&h, "reason"))?;
            if p::truthy(&reason) {
                why.push(reason);
            }
            let why = cx.call(l, "<genexpr>", |cx| {
                cx.at(l, p::join(", ", &Value::Array(why)))
            })?;
            let l = ln!("print(\"    %-7s  %-36s %4d try(s)   %s\"");
            let verdict = cx.at(l, p::get(&h, "verdict"))?;
            let conns = p::or(cx.at(l, p::get(&h, "conns"))?, p::int(0));
            let t = cx.at(
                l,
                p::pct(
                    "    %-7s  %-36s %4d try(s)   %s",
                    &[verdict, s(&hp), conns, s(&why)],
                ),
            )?;
            cx.print(&t);
        }
    }
    let n = cx.at(ln!("if len(nh) > 40:"), p::len(&nh))?;
    if n > 40 {
        let l = ln!("print(\"    and %d more (");
        let short = cx.at(l, p::idx(j, "short"))?;
        let t = cx.at(
            l,
            p::pct(
                concat!("    and %d more (", cli_name!(), " compute get %s --json)"),
                &[p::int(n as i128 - 40), short],
            ),
        )?;
        cx.print(&t);
    }
    Ok(())
}

fn hb(cx: &mut Cx, n: &Value) -> Result<String, Raise> {
    let n = p::or(n.clone(), p::int(0));
    let l2 = ln!("return \"%d B\" % n if n < 1024");
    if cx.at(l2, p::cmp("<", &n, &p::int(1024)))? {
        return cx.at(l2, p::pct("%d B", &[n]));
    }
    if cx.at(l2, p::cmp("<", &n, &p::int(1 << 20)))? {
        let x = cx.at(l2, p::div(&n, &p::int(1024)))?;
        return cx.at(l2, p::pct("%.0f KB", &[x]));
    }
    if cx.at(l2, p::cmp("<", &n, &p::int(1 << 30)))? {
        let x = cx.at(l2, p::div(&n, &p::int(1 << 20)))?;
        return cx.at(l2, p::pct("%.1f MB", &[x]));
    }
    let x = cx.at(l2, p::div(&n, &p::int(1 << 30)))?;
    cx.at(l2, p::pct("%.1f GB", &[x]))
}

fn resources(cx: &mut Cx, r: &Value) -> Result<(), Raise> {
    let rs = or_dict(cx.at(
        ln!("rs = r.get(\"resources\") or {}"),
        p::get(r, "resources"),
    )?);
    if !p::truthy(&rs) {
        return Ok(());
    }
    let l = ln!("pk, lim = rs.get(\"peakMemBytes\"), rs.get(\"memLimitBytes\")");
    let pk = cx.at(l, p::get(&rs, "peakMemBytes"))?;
    let lim = cx.at(l, p::get(&rs, "memLimitBytes"))?;
    let l = ln!("print(\"  resources: peak memory %s%s (%s), CPU time %s (%s)\"");
    let gib = |cx: &Cx, x: &Value| -> Result<String, Raise> {
        let d = cx.at(l, p::div(x, &p::int(1 << 30)))?;
        cx.at(l, p::pct("%.1fG", &[d]))
    };
    let peak = if p::is_none(&pk) {
        "-".to_string()
    } else {
        gib(cx, &pk)?
    };
    let of = if p::is_none(&lim) {
        String::new()
    } else {
        format!(" of {}", gib(cx, &lim)?)
    };
    let ms = p::or(cx.at(l, p::get(&rs, "memSource"))?, s("-"));
    let cpu = if p::is_none(&cx.at(l, p::get(&rs, "cpuSecs"))?) {
        s("-")
    } else {
        let c = cx.at(l, p::idx(&rs, "cpuSecs"))?;
        let c = cx.at(l, p::round0(&c))?;
        s(&cx.at(l, p::pct("%ss", &[c]))?)
    };
    let cs = p::or(cx.at(l, p::get(&rs, "cpuSource"))?, s("-"));
    let t = cx.at(
        l,
        p::pct(
            "  resources: peak memory %s%s (%s), CPU time %s (%s)",
            &[s(&peak), s(&of), ms, cpu, cs],
        ),
    )?;
    cx.print(&t);
    let gv = cx.at(ln!("g = rs.get(\"gpu\")"), p::get(&rs, "gpu"))?;
    if !p::is_dict(&gv) {
        return Ok(());
    }
    let gg = |cx: &Cx, line: u32, k: &str| cx.at(line, p::get(&gv, k));
    let gbl = |cx: &mut Cx, line: u32, b: &Value| -> Result<String, Raise> {
        cx.call(line, "<lambda>", |cx| {
            let l = ln!("gb = lambda b: \"%.1fG\" % (b / (1 << 30))");
            let d = cx.at(l, p::div(b, &p::int(1 << 30)))?;
            cx.at(l, p::pct("%.1fG", &[d]))
        })
    };
    let l = ln!("bits = [\"busy avg %s%% (max %s%%)\"");
    let mut bits = vec![if !p::is_none(&gg(cx, l, "busyAvgPct")?) {
        let a = gg(cx, l, "busyAvgPct")?;
        let m = gg(cx, l, "busyMaxPct")?;
        cx.at(l, p::pct("busy avg %s%% (max %s%%)", &[a, m]))?
    } else {
        "busy % not reported".to_string()
    }];
    let l = ln!("if g.get(\"vramPeakBytes\") is not None:");
    if !p::is_none(&gg(cx, l, "vramPeakBytes")?) {
        let x = cx.at(l, p::idx(&gv, "vramPeakBytes"))?;
        bits.push(format!("VRAM +{}", gbl(cx, l, &x)?));
    }
    let l = ln!("if g.get(\"gttPeakBytes\") is not None:");
    if !p::is_none(&gg(cx, l, "gttPeakBytes")?) {
        let x = cx.at(l, p::idx(&gv, "gttPeakBytes"))?;
        bits.push(format!("GTT +{}", gbl(cx, l, &x)?));
    }
    let l = ln!("print(\"  gpu: %s: %s (whole device, %s)\"");
    let mut model = gg(cx, l, "model")?;
    if !p::truthy(&model) {
        model = gg(cx, l, "pci")?;
    }
    let src = p::or(gg(cx, l, "source")?, s("-"));
    let t = cx.at(
        l,
        p::pct(
            "  gpu: %s: %s (whole device, %s)",
            &[model, s(&bits.join(", ")), src],
        ),
    )?;
    cx.print(&t);
    let l = ln!("if g.get(\"memLimitBytes\") is not None:");
    if !p::is_none(&gg(cx, l, "memLimitBytes")?) {
        let l = ln!("own = \"peak \" + gb(g[\"jobMemPeakBytes\"])");
        let own = if !p::is_none(&gg(cx, l, "jobMemPeakBytes")?) {
            let x = cx.at(l, p::idx(&gv, "jobMemPeakBytes"))?;
            format!("peak {}", gbl(cx, l, &x)?)
        } else {
            "not read".to_string()
        };
        let l = ln!("print(\"  gpu memory (the job itself): %s of --gpu-mem %s\"");
        let x = cx.at(l, p::idx(&gv, "memLimitBytes"))?;
        let lim = gbl(cx, l, &x)?;
        cx.print(&format!(
            "  gpu memory (the job itself): {own} of --gpu-mem {lim}"
        ));
    }
    Ok(())
}

fn stamp(cx: &mut Cx, sv: &Value) -> Result<String, Raise> {
    if !p::truthy(sv) {
        return Ok("-".into());
    }
    let l = ln!("return \"load %s of %s CPUs, %s other job(s)%s%s%s\"");
    let g = |cx: &Cx, k: &str| cx.at(l, p::get(sv, k));
    let load = g(cx, "load1")?;
    let cpus = g(cx, "cpus")?;
    let jobs = g(cx, "jobs")?;
    let steal = if !p::is_none(&g(cx, "steal")?) {
        let x = cx.at(l, p::idx(sv, "steal"))?;
        cx.at(l, p::pct(", steal %s%%", &[x]))?
    } else {
        String::new()
    };
    let gpu = if !p::is_none(&g(cx, "gpuBusy")?) {
        let x = cx.at(l, p::idx(sv, "gpuBusy"))?;
        cx.at(l, p::pct(", GPU %s%% busy", &[x]))?
    } else {
        String::new()
    };
    let quiet = if !p::is_none(&g(cx, "quiet")?) {
        if p::truthy(&cx.at(l, p::idx(sv, "quiet"))?) {
            ", quiet".to_string()
        } else if p::truthy(&g(cx, "quietWhy")?) {
            let w = cx.at(l, p::idx(sv, "quietWhy"))?;
            cx.at(l, p::pct(", not quiet (%s)", &[w]))?
        } else {
            ", not quiet".to_string()
        }
    } else {
        String::new()
    };
    cx.at(
        l,
        p::pct(
            "load %s of %s CPUs, %s other job(s)%s%s%s",
            &[load, cpus, jobs, s(&steal), s(&gpu), s(&quiet)],
        ),
    )
}
