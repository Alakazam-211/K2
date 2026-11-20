//! `compute run --plan` (`--dry-run`): the reader of the plan the daemon
//! answers (`cmd_compute_run`'s first Python, `cli/src/compute-run.sh`):
//! the plan's lines on stderr (where it would run, when, the source, the
//! tools, the GPU, the network, the limits, the settings a preset or
//! `.k2/compute.toml` filled in, the CPU hints, the runners-up), then the
//! answer, indented, on stdout. A raise: the lines so far, its traceback,
//! exit 1.

use super::super::more::py_line_q;
use super::super::pyrt::{self as p, s};
use super::super::render::{self, Cx};
use super::run::{plus, source_line};
use super::SRC;
use crate::families::{raised, Raise, Shown};
use crate::out::json::Value;
use k2_client::brand::CLI_NAME;

macro_rules! line {
    ($m:literal) => {
        const { py_line_q(SRC, "    if [ \"$plan\" = \"1\" ]; then", $m) }
    };
}

/// The reader on the answer.
pub fn reader(body: &[u8]) -> Shown {
    let mut cx = Cx::new(0);
    let mut err = String::new();
    match read(&mut cx, body, &mut err) {
        Ok(()) => Shown {
            stdout: std::mem::take(&mut cx.out),
            stderr: err,
            exit: crate::exit::Exit::Ok,
        },
        Err(e) => raised(std::mem::take(&mut cx.out), err, &e),
    }
}

fn empty() -> Value {
    Value::Object(vec![])
}

fn tag(text: &str) -> String {
    format!("[{CLI_NAME} compute] {text}")
}

/// `def d(x): return "%dm%02ds" % (x // 60, x % 60)`.
fn d(cx: &mut Cx, from: u32, x: &Value) -> Result<String, Raise> {
    let l = line!("return \"%dm%02ds\"");
    cx.call(from, "d", |cx| {
        let m = cx.at(l, p::floordiv(x, &p::int(60)))?;
        let r = cx.at(l, p::rem(x, &p::int(60)))?;
        cx.at(l, p::pct("%dm%02ds", &[m, r]))
    })
}

/// `def dur(s)`: whole hours, whole minutes, or seconds.
fn dur(cx: &mut Cx, from: u32, x: &Value) -> Result<Value, Raise> {
    cx.call(from, "dur", |cx| {
        if p::is_none(x) {
            return Ok(Value::Null);
        }
        let n = cx.at(line!("    s = int(s)"), p::to_int(x))?;
        let l = line!("return \"%dh\"");
        let text = if n >= 3600 && n % 3600 == 0 {
            cx.at(l, p::pct("%dh", &[p::int(n.div_euclid(3600))]))?
        } else if n >= 60 && n % 60 == 0 {
            cx.at(l, p::pct("%dm", &[p::int(n.div_euclid(60))]))?
        } else {
            cx.at(l, p::pct("%ds", &[p::int(n)]))?
        };
        Ok(s(&text))
    })
}

fn read(cx: &mut Cx, body: &[u8], err: &mut String) -> Result<(), Raise> {
    let v = render::load(cx, body, line!("v = json.load"))?;
    let l = line!("e = v.get(\"estimate\")");
    let e = cx.at(l, p::get(&v, "estimate"))?;
    let l = line!("line = \"[k2 compute] plan:");
    let starts = cx.at(l, p::get(&v, "startsNow"))?;
    let first = if p::truthy(&starts) {
        s("starts now")
    } else {
        let q = cx.at(l, p::idx(&v, "queue"))?;
        let ahead = cx.at(l, p::idx(&q, "ahead"))?;
        let q = cx.at(l, p::idx(&v, "queue"))?;
        let blocked = cx.at(l, p::get(&q, "blockedBy"))?;
        let why = if p::truthy(&blocked) {
            let q = cx.at(l, p::idx(&v, "queue"))?;
            let b = cx.at(l, p::idx(&q, "blockedBy"))?;
            let code = cx.at(l, p::idx(&b, "code"))?;
            s(&cx.at(l, plus(": ", &code))?)
        } else {
            s("")
        };
        s(&cx.at(l, p::pct("waits (%s ahead%s)", &[ahead, why]))?)
    };
    let node = cx.at(l, p::idx(&v, "node"))?;
    let mut line = cx.at(l, p::pct(&tag("plan: %s on %s"), &[first, node]))?;
    let l = line!("if not v.get(\"online\")");
    if !p::truthy(&cx.at(l, p::get(&v, "online"))?) {
        line.push_str(" (offline now)");
    }
    let l = line!("if v.get(\"warmSlot\")");
    if p::truthy(&cx.at(l, p::get(&v, "warmSlot"))?) {
        let w = cx.at(l, p::idx(&v, "warmSlot"))?;
        let first = match &w {
            Value::Str(_) => p::str_of(&w).split(' ').next().unwrap_or("").to_string(),
            other => {
                return Err(cx.raise(
                    l,
                    format!(
                        "AttributeError: '{}' object has no attribute 'split'",
                        p::tname(other)
                    ),
                ))
            }
        };
        line.push_str(" \u{b7} ");
        line.push_str(&first);
    }
    let l = line!("line += (\" · ~%s");
    if p::truthy(&e) {
        let med = cx.at(l, p::idx(&e, "medianSecs"))?;
        let med = d(cx, l, &med)?;
        let p90 = cx.at(l, p::idx(&e, "p90Secs"))?;
        let p90 = d(cx, l, &p90)?;
        let samples = cx.at(l, p::idx(&e, "samples"))?;
        let basis = cx.at(l, p::idx(&e, "basis"))?;
        line.push_str(&cx.at(
            l,
            p::pct(
                " \u{b7} ~%s (p90 %s, %d runs: %s)",
                &[s(&med), s(&p90), samples, basis],
            ),
        )?);
    } else {
        line.push_str(" \u{b7} no estimate yet");
    }
    let l = line!("for w in v.get(\"warnings\")");
    let ws = p::or(cx.at(l, p::get(&v, "warnings"))?, Value::Array(vec![]));
    for w in cx.at(l, p::iter(&ws))? {
        let l = line!("    sys.stderr.write(\"[k2 compute] note: %s\\n\" % w)");
        err.push_str(&cx.at(l, p::pct(&tag("note: %s\n"), &[w]))?);
    }
    let l = line!("for n in v.get(\"notices\")");
    let ns = p::or(cx.at(l, p::get(&v, "notices"))?, Value::Array(vec![]));
    for n in cx.at(l, p::iter(&ns))? {
        let l = line!("(n.get(\"message\") or n.get(\"code\"))");
        let m = cx.at(l, p::get(&n, "message"))?;
        let m = if p::truthy(&m) {
            m
        } else {
            cx.at(l, p::get(&n, "code"))?
        };
        err.push_str(&cx.at(l, p::pct(&tag("note: %s\n"), &[m]))?);
    }
    err.push_str(&line);
    err.push('\n');
    let l = line!("p = v.get(\"placement\") or {}");
    let pl = p::or(cx.at(l, p::get(&v, "placement"))?, empty());
    let l = line!("if p.get(\"mode\") == \"auto\":");
    if p::eq_s(&cx.at(l, p::get(&pl, "mode"))?, "auto") {
        let l = line!("would place on");
        let node = cx.at(l, p::get(&pl, "node"))?;
        let why = cx.at(l, p::get(&pl, "why"))?;
        let fits = if p::truthy(&cx.at(l, p::get(&pl, "fits"))?) {
            let f = cx.at(l, p::idx(&pl, "fits"))?;
            s(&cx.at(l, plus(" \u{b7} ", &f))?)
        } else {
            s("")
        };
        err.push_str(&cx.at(
            l,
            p::pct(&tag("would place on %s (auto): %s%s\n"), &[node, why, fits]),
        )?);
    } else {
        let l = line!("elif p.get(\"why\") and");
        let why = cx.at(l, p::get(&pl, "why"))?;
        if p::truthy(&why) && !p::eq_s(&cx.at(l, p::get(&pl, "why"))?, "you named this node") {
            let l = line!("pinned to %s");
            let node = cx.at(l, p::get(&pl, "node"))?;
            let why = cx.at(l, p::get(&pl, "why"))?;
            err.push_str(&cx.at(l, p::pct(&tag("pinned to %s: %s\n"), &[node, why]))?);
        }
    }
    let l = line!("src = v.get(\"source\")");
    let src = cx.at(l, p::get(&v, "source"))?;
    if p::is_dict(&src) {
        err.push_str(&source_line(cx, &src, |m| match m {
            "where" => line!("where = src.get(\"checkout\")"),
            "repo" => line!("if src.get(\"repo\") and"),
            "path" => line!("if src.get(\"path\"): where"),
            "s" => line!("s = \"[k2 compute] source:"),
            "d" => line!("if isinstance(src.get(\"dirty\"), dict)"),
            _ => line!("s += \" +dirty"),
        })?);
    }
    let l = line!("if v.get(\"toolsLine\")");
    if p::truthy(&cx.at(l, p::get(&v, "toolsLine"))?) {
        let l = line!("tools on %s");
        let node = cx.at(l, p::idx(&v, "node"))?;
        let tools = cx.at(l, p::idx(&v, "toolsLine"))?;
        err.push_str(&cx.at(l, p::pct(&tag("tools on %s: %s\n"), &[node, tools]))?);
    }
    let l = line!("lim = v.get(\"limits\")");
    let lim = p::or(cx.at(l, p::get(&v, "limits"))?, empty());
    let mut ends: Vec<String> = vec![];
    for (key, fmt, marker_if, marker_append) in [
        (
            "stallSecs",
            "ends after %s with no output (--stall-timeout)",
            line!("if lim.get(\"stallSecs\")"),
            line!("ends.append(\"ends after"),
        ),
        (
            "queueTimeoutSecs",
            "fails if not started within %s (--queue-timeout)",
            line!("if lim.get(\"queueTimeoutSecs\")"),
            line!("ends.append(\"fails if"),
        ),
        (
            "timeoutSecs",
            "stops after %s running (--timeout)",
            line!("if lim.get(\"timeoutSecs\")"),
            line!("ends.append(\"stops after"),
        ),
    ] {
        if p::truthy(&cx.at(marker_if, p::get(&lim, key))?) {
            let x = cx.at(marker_append, p::idx(&lim, key))?;
            let t = dur(cx, marker_append, &x)?;
            ends.push(cx.at(marker_append, p::pct(fmt, &[t]))?);
        }
    }
    let l = line!("g = v.get(\"gpu\")");
    let g = cx.at(l, p::get(&v, "gpu"))?;
    if p::is_dict(&g) {
        gpu(cx, &v, &g, err)?;
    }
    let l = line!("nw = v.get(\"network\")");
    let nw = cx.at(l, p::get(&v, "network"))?;
    if p::is_dict(&nw) {
        let l = line!("network: %s (this job only");
        let nline = cx.at(l, p::get(&nw, "line"))?;
        let dummies = if p::truthy(&cx.at(l, p::get(&nw, "dummies"))?) {
            s(" and its dummy interfaces")
        } else {
            s("")
        };
        err.push_str(&cx.at(
            l,
            p::pct(
                &tag("network: %s (this job only: loopback%s, no internet, no LAN)\n"),
                &[nline, dummies],
            ),
        )?);
    }
    if !ends.is_empty() {
        err.push_str(&tag(&format!("limits: {}\n", ends.join(" \u{b7} "))));
    }
    effective(cx, &v, err)?;
    ranking(cx, &v, err)?;
    let l = line!("print(json.dumps(v, indent=2))");
    let text = cx.at(l, p::dumps(&v, false, Some(2)))?;
    cx.print(&text);
    Ok(())
}

fn gpu(cx: &mut Cx, v: &Value, g: &Value, err: &mut String) -> Result<(), Raise> {
    let l = line!("gs = g.get(\"gpus\")");
    let gs = p::or(cx.at(l, p::get(g, "gpus"))?, Value::Array(vec![]));
    if p::truthy(&gs) {
        let l = line!("gpu: %s, %s free now");
        let mut names = vec![];
        for x in cx.at(l, p::iter(&gs))? {
            let m = cx.call(l, "<genexpr>", |cx| {
                let m = cx.at(l, p::get(&x, "model"))?;
                Ok(if p::truthy(&m) {
                    m
                } else {
                    cx.at(l, p::get(&x, "pci"))?
                })
            })?;
            names.push(m);
        }
        let joined = cx.at(l, p::join(" \u{b7} ", &Value::Array(names)))?;
        let free = cx.at(l, p::get(g, "free"))?;
        err.push_str(&cx.at(
            l,
            p::pct(
                &tag("gpu: %s, %s free now (one GPU, this job alone)\n"),
                &[s(&joined), free],
            ),
        )?);
    } else {
        let l = line!("gpu: none on %s");
        let node = cx.at(l, p::idx(v, "node"))?;
        err.push_str(&cx.at(
            l,
            p::pct(
                &tag("gpu: none on %s for this server (it would fail there)\n"),
                &[node],
            ),
        )?);
    }
    let l = line!("m = g.get(\"mem\")");
    let m = cx.at(l, p::get(g, "mem"))?;
    if !p::is_dict(&m) {
        return Ok(());
    }
    let l = line!("if m.get(\"budgetBytes\") is None:");
    let text = if p::is_none(&cx.at(l, p::get(&m, "budgetBytes"))?) {
        "gpu memory: not watched (a discrete GPU; --gpu-mem sets a budget)".to_string()
    } else {
        let l = line!("s = \"gpu memory: budget %s%s\"");
        let b = cx.at(l, p::idx(&m, "budgetBytes"))?;
        let gib = cx.call(l, "<lambda>", |cx| {
            let gl = line!("gib = lambda b:");
            let q = cx.at(gl, p::div(&b, &p::int(1 << 30)))?;
            cx.at(gl, p::pct("%.1fG", &[q]))
        })?;
        let watched = if p::truthy(&cx.at(l, p::get(&m, "watched"))?) {
            s("")
        } else {
            s(" (NOT watched: an older k2-node there)")
        };
        let mut text = cx.at(l, p::pct("gpu memory: budget %s%s", &[s(&gib), watched]))?;
        let l = line!("if m.get(\"countedBytes\")");
        if p::truthy(&cx.at(l, p::get(&m, "countedBytes"))?) {
            text.push_str(", counted with --mem (unified GPU: its memory is system RAM)");
        }
        text
    };
    err.push_str(&tag(&format!("{text}\n")));
    Ok(())
}

fn effective(cx: &mut Cx, v: &Value, err: &mut String) -> Result<(), Raise> {
    let l = line!("eff = v.get(\"effective\")");
    let eff = p::or(cx.at(l, p::get(v, "effective"))?, empty());
    let mut parts: Vec<String> = vec![];
    for k in [
        "os", "arch", "mem", "cpus", "needs", "env", "timeout", "stall", "tier", "cache",
    ] {
        let l = line!("    e = eff.get(k)");
        let e = cx.at(l, p::get(&eff, k))?;
        let l = line!("if not isinstance(e, dict) or");
        if !p::is_dict(&e) {
            continue;
        }
        let from = cx.at(l, p::get(&e, "from"))?;
        if p::is_none(&from) || p::eq_s(&from, "flag") {
            continue;
        }
        let l = line!("if k == \"mem\" and str(");
        if k == "mem" && p::str_of(&cx.at(l, p::get(&e, "from"))?).starts_with("auto") {
            continue;
        }
        let l = line!("val = e.get(\"value\")");
        let mut val = cx.at(l, p::get(&e, "value"))?;
        if p::is_list(&val) {
            let items: Vec<String> = cx.at(l + 1, p::iter(&val))?.iter().map(p::str_of).collect();
            val = s(&items.join(", "));
        }
        let l = line!("parts.append(\"%s %s (%s)\"");
        let from = cx.at(l, p::idx(&e, "from"))?;
        parts.push(cx.at(l, p::pct("%s %s (%s)", &[s(k), val, from]))?);
    }
    if !parts.is_empty() {
        let l = line!("    src = eff.get(\"source\") or \"\"");
        let mut src = p::or(cx.at(l, p::get(&eff, "source"))?, s(""));
        let l = line!("if eff.get(\"preset\"): src +=");
        if p::truthy(&cx.at(l, p::get(&eff, "preset"))?) {
            let sep = if p::truthy(&src) { ", " } else { "" };
            let preset = cx.at(l, p::idx(&eff, "preset"))?;
            let rhs = cx.at(l, plus(&format!("{sep}preset "), &preset))?;
            src = cx.at(l, p::iadd(&src, &s(&rhs)))?;
        }
        let l = line!("from %s: %s");
        err.push_str(&cx.at(
            l,
            p::pct(&tag("from %s: %s\n"), &[src, s(&parts.join("; "))]),
        )?);
    }
    let l = line!("hints = ((eff.get(\"env\")");
    let env = p::or(cx.at(l, p::get(&eff, "env"))?, empty());
    let hints = p::or(cx.at(l, p::get(&env, "cpuHints"))?, empty());
    if p::truthy(&hints) {
        let l = line!("CPU hints: %s");
        let Value::Object(pairs) = &hints else {
            return Err(cx.raise(
                l,
                format!(
                    "AttributeError: '{}' object has no attribute 'items'",
                    p::tname(&hints)
                ),
            ));
        };
        let mut kvs = vec![];
        for (k, x) in pairs {
            kvs.push(cx.at(l, p::pct("%s=%s", &[Value::Str(k.clone()), x.clone()]))?);
        }
        err.push_str(&tag(&format!(
            "CPU hints: {} (the job's CPUs; --env NAME=\u{2026} overrides)\n",
            kvs.join(" ")
        )));
    }
    Ok(())
}

fn ranking(cx: &mut Cx, v: &Value, err: &mut String) -> Result<(), Raise> {
    let l = line!("rk = v.get(\"ranking\")");
    let rk = p::or(cx.at(l, p::get(v, "ranking"))?, Value::Array(vec![]));
    let l = line!("if len(rk) > 1:");
    if cx.at(l, p::len(&rk))? <= 1 {
        return Ok(());
    }
    let l = line!("ranking: %s");
    let mut items = vec![];
    for r in cx.at(l, p::iter(&rk))? {
        items.push(cx.call(l, "<genexpr>", |cx| {
            cx.call(l, "one", |cx| {
                let l1 = line!("s = r[\"node\"] + \" (\" + r[\"state\"]");
                let node = cx.at(l1, p::idx(&r, "node"))?;
                let a = cx.at(l1, p::add(&node, &s(" (")))?;
                let state = cx.at(l1, p::idx(&r, "state"))?;
                let mut out = cx.at(l1, p::add(&a, &state))?;
                let l2 = line!("if r.get(\"etaSecs\") is not None");
                if !p::is_none(&cx.at(l2, p::get(&r, "etaSecs"))?) {
                    let eta = cx.at(l2, p::idx(&r, "etaSecs"))?;
                    let t = dur(cx, l2, &eta)?;
                    let basis = cx.at(l2, p::get(&r, "etaBasis"))?;
                    let more = cx.at(l2, p::pct(", ~%s %s", &[t, basis]))?;
                    out = cx.at(l2, p::iadd(&out, &s(&more)))?;
                }
                let l3 = line!("return s + \")\"");
                cx.at(l3, p::add(&out, &s(")")))
            })
        })?);
    }
    let joined = cx.at(l, p::join(" \u{b7} ", &Value::Array(items)))?;
    err.push_str(&tag(&format!("ranking: {joined}\n")));
    Ok(())
}
