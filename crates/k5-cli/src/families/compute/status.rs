//! `cmd_compute_status`'s Python (`cli/src/compute-jobs.sh`): every node at
//! a glance, for people.

use super::lines::{py_line, JOBS};
use super::pyrt::{self as p, s};
use super::render::{load, lookup, Cx};
use crate::families::{raised, Raise, Shown};
use crate::out::json::Value;
use k2_client::{cli_name, product_name};

macro_rules! ln {
    ($m:literal) => {
        const { py_line(JOBS, "\ncmd_compute_status() {", $m) }
    };
}

fn dict() -> Value {
    Value::Object(Vec::new())
}

fn list() -> Value {
    Value::Array(Vec::new())
}

/// The Python on `body`. `now`: `int(time.time())` (unused here; the
/// renderer's state wants one).
pub fn render(body: &[u8], now: i64) -> Shown {
    let mut cx = Cx::new(now);
    match run(&mut cx, body) {
        Ok(()) => Shown::out(cx.out),
        Err(r) => raised(std::mem::take(&mut cx.out), String::new(), &r),
    }
}

fn gb(cx: &mut Cx, line: u32, b: &Value) -> Result<String, Raise> {
    cx.call(line, "gb", |cx| {
        if p::is_none(b) {
            return Ok("-".into());
        }
        let l = ln!("return \"-\" if b is None else \"%.0f GB\"");
        let r = cx.at(l, p::div(b, &p::int(1 << 30)))?;
        cx.at(l, p::pct("%.0f GB", &[r]))
    })
}

fn cores(cx: &mut Cx, line: u32, m: &Value) -> Result<String, Raise> {
    cx.call(line, "cores", |cx| {
        let l = ln!("return \"%g\" % round((m or 0) / 1000, 1)");
        let x = cx.at(l, p::div(&p::or(m.clone(), p::int(0)), &p::int(1000)))?;
        let x = cx.at(l, p::round_n(&x, 1))?;
        cx.at(l, p::pct("%g", &[x]))
    })
}

fn mmss(cx: &mut Cx, line: u32, sv: &Value) -> Result<String, Raise> {
    cx.call(line, "mmss", |cx| {
        if p::is_none(sv) {
            return Ok("-".into());
        }
        let l = ln!("return \"-\" if s is None else (\"%dm%02ds\"");
        if cx.at(l, p::cmp("<", sv, &p::int(3600)))? {
            let m = cx.at(l, p::floordiv(sv, &p::int(60)))?;
            let s2 = cx.at(l, p::rem(sv, &p::int(60)))?;
            cx.at(l, p::pct("%dm%02ds", &[m, s2]))
        } else {
            let h = cx.at(l, p::floordiv(sv, &p::int(3600)))?;
            let m = cx.at(l, p::rem(sv, &p::int(3600)))?;
            let m = cx.at(l, p::floordiv(&m, &p::int(60)))?;
            cx.at(l, p::pct("%dh%02dm", &[h, m]))
        }
    })
}

fn run(cx: &mut Cx, body: &[u8]) -> Result<(), Raise> {
    let v = load(cx, body, ln!("v = json.load(sys.stdin)"))?;
    let l = ln!("rows = v.get(\"nodes\") or []");
    let rows = p::or(cx.at(l, p::get(&v, "nodes"))?, list());
    if !p::truthy(&rows) {
        cx.print("(no compute nodes)");
    }
    let l = ln!("for n in rows:");
    for n in cx.at(l, p::iter(&rows))? {
        node(cx, &n)?;
    }
    let l = ln!("m = v.get(\"controllerMachine\") or {}");
    let m = p::or(cx.at(l, p::get(&v, "controllerMachine"))?, dict());
    if p::truthy(&m) {
        let l = ln!("s = \"controller %s: disk %s free of %s\"");
        let name = cx.at(l, p::get(&m, "name"))?;
        let a = cx.at(l, p::get(&m, "diskFreeBytes"))?;
        let a = gb(cx, l, &a)?;
        let b = cx.at(l, p::get(&m, "diskTotalBytes"))?;
        let b = gb(cx, l, &b)?;
        let mut line = cx.at(
            l,
            p::pct("controller %s: disk %s free of %s", &[name, s(&a), s(&b)]),
        )?;
        let l = ln!("if m.get(\"tmLocalSnapshots\") is not None:");
        if !p::is_none(&cx.at(l, p::get(&m, "tmLocalSnapshots"))?) {
            let l = ln!("s += \" · Time Machine local snapshots: %d\"");
            let x = cx.at(l, p::idx(&m, "tmLocalSnapshots"))?;
            line += &cx.at(l, p::pct(" · Time Machine local snapshots: %d", &[x]))?;
        } else if p::truthy(&cx.at(
            ln!("elif m.get(\"tmLocalSnapshotsError\"):"),
            p::get(&m, "tmLocalSnapshotsError"),
        )?) {
            let l = ln!("s += \" · Time Machine local snapshots: unknown (%s)\"");
            let x = cx.at(l, p::idx(&m, "tmLocalSnapshotsError"))?;
            line += &cx.at(
                l,
                p::pct(" · Time Machine local snapshots: unknown (%s)", &[x]),
            )?;
        }
        cx.print(&line);
    }
    let l = ln!("if v.get(\"notYet\"):");
    if p::truthy(&cx.at(l, p::get(&v, "notYet"))?) {
        let l = ln!("print(\"not yet: \" + v[\"notYet\"])");
        let x = cx.at(l, p::idx(&v, "notYet"))?;
        let t = cx.at(l, p::add(&s("not yet: "), &x))?;
        cx.print(&p::str_of(&t));
    }
    Ok(())
}

fn node(cx: &mut Cx, n: &Value) -> Result<(), Raise> {
    let st = p::or(
        cx.at(ln!("st = n.get(\"status\") or {}"), p::get(n, "status"))?,
        dict(),
    );
    let g = |cx: &Cx, line: u32, k: &str| cx.at(line, p::get(&st, k));
    let l = ln!("parts = [\"%-14s\" % n.get(\"name\")");
    let name = cx.at(l, p::get(n, "name"))?;
    let os = p::or(cx.at(l, p::get(n, "os"))?, s("?"));
    let arch = p::or(cx.at(l, p::get(n, "arch"))?, s("?"));
    let state = p::or(g(cx, l, "state")?, s("?"));
    let mut parts: Vec<Value> = vec![
        s(&cx.at(l, p::pct("%-14s", &[name]))?),
        s(&cx.at(l, p::pct("%s-%s", &[os, arch]))?),
        state,
    ];
    let l = ln!("if st.get(\"controllerPause\"):");
    if p::truthy(&g(cx, l, "controllerPause")?) {
        let l = ln!("parts.append(\"(server: %s)\" % st[\"controllerPause\"])");
        let x = cx.at(l, p::idx(&st, "controllerPause"))?;
        parts.push(s(&cx.at(l, p::pct("(server: %s)", &[x]))?));
    }
    let w = g(cx, ln!("w = st.get(\"waiting\")"), "waiting")?;
    if p::truthy(&w) {
        let l = ln!("parts.append(\"held: %s (%s)\"");
        let code = cx.at(l, p::get(&w, "code"))?;
        let msg = cx.at(l, p::get(&w, "message"))?;
        parts.push(s(&cx.at(l, p::pct("held: %s (%s)", &[code, msg]))?));
    }
    let l = ln!("if st.get(\"unavailable\"):");
    if p::truthy(&g(cx, l, "unavailable")?) {
        let l = ln!("parts.append(\"unavailable: \" + \",\".join(st[\"unavailable\"]))");
        let x = cx.at(l, p::idx(&st, "unavailable"))?;
        parts.push(s(&format!("unavailable: {}", cx.at(l, p::join(",", &x))?)));
    }
    let l = ln!("mine = n.get(\"mine\") if isinstance(n.get(\"mine\"), dict) else None");
    let mine = cx.at(l, p::get(n, "mine"))?;
    let mine = p::is_dict(&mine).then_some(mine);
    let yours = |cx: &mut Cx, line: u32, k: &str| -> Result<String, Raise> {
        cx.call(line, "<lambda>", |cx| match &mine {
            Some(m) if p::truthy(m) => {
                let l =
                    ln!("yours = lambda k: (\", yours %d\" % mine.get(k, 0)) if mine else \"\"");
                let x = cx.at(l, p::get_or(m, k, p::int(0)))?;
                cx.at(l, p::pct(", yours %d", &[x]))
            }
            _ => Ok(String::new()),
        })
    };
    let l = ln!("room = n.get(\"room\") if isinstance(n.get(\"room\"), dict) else None");
    let room = cx.at(l, p::get(n, "room"))?;
    let room = p::is_dict(&room).then_some(room);
    let l = ln!("busy = {\"other_servers\"");
    let room_or = room.clone().unwrap_or_else(dict);
    let key = p::or(cx.at(l, p::get(&room_or, "busy"))?, s(""));
    let busy = cx.at(
        l,
        lookup(
            &[
                ("other_servers", "busy (other servers)"),
                ("servers", "busy (this and other servers)"),
                ("this_server", "busy (this server)"),
            ],
            &key,
        ),
    )?;
    if let Some(b) = busy {
        parts.push(s(&b));
    }
    let l = ln!("par = st.get(\"parallelMax\") if");
    let par = g(cx, l, "parallelMax")?;
    let par = if p::is_none(&par) { s("?") } else { par };
    let l = ln!("parts.append(\"runs %d/%s (this server %d%s%s)\"");
    let nrun = |cx: &Cx, line: u32| -> Result<Value, Raise> {
        let r = p::or(g(cx, line, "running")?, list());
        Ok(p::int(cx.at(line, p::len(&r))? as i128))
    };
    if let Some(room) = room.as_ref().filter(|r| p::truthy(r)) {
        let lo = ln!("others = (\", other servers %d\" % room[\"otherServers\"])");
        let others = if p::truthy(&cx.at(lo, p::get(room, "otherServers"))?) {
            let x = cx.at(lo, p::idx(room, "otherServers"))?;
            cx.at(lo, p::pct(", other servers %d", &[x]))?
        } else {
            String::new()
        };
        let running = cx.at(l, p::get_or(room, "running", p::int(0)))?;
        let mine_n = nrun(cx, l)?;
        let y = yours(cx, l, "running")?;
        parts.push(s(&cx.at(
            l,
            p::pct(
                "runs %d/%s (this server %d%s%s)",
                &[running, par, mine_n, s(&y), s(&others)],
            ),
        )?));
    } else {
        let l = ln!("parts.append(\"this server runs %d (of %s slots%s)\"");
        let mine_n = nrun(cx, l)?;
        let y = yours(cx, l, "running")?;
        parts.push(s(&cx.at(
            l,
            p::pct("this server runs %d (of %s slots%s)", &[mine_n, par, s(&y)]),
        )?));
    }
    let l = ln!("parts.append(\"queued %d (this server%s)\"");
    let q = p::or(g(cx, l, "queued")?, list());
    let nq = p::int(cx.at(l, p::len(&q))? as i128);
    let y = yours(cx, l, "queued")?;
    parts.push(s(
        &cx.at(l, p::pct("queued %d (this server%s)", &[nq, s(&y)]))?
    ));
    let l = ln!("f, c = st.get(\"free\") or {}, st.get(\"caps\") or {}");
    let f = p::or(g(cx, l, "free")?, dict());
    let c = p::or(g(cx, l, "caps")?, dict());
    if p::truthy(&f) {
        let l = ln!("idle = (\" · idle ~%s\" % cores(st[\"idleCpuMillis\"]))");
        let idle = if !p::is_none(&g(cx, l, "idleCpuMillis")?) {
            let x = cx.at(l, p::idx(&st, "idleCpuMillis"))?;
            format!(" · idle ~{}", cores(cx, l, &x)?)
        } else {
            String::new()
        };
        let l = ln!("parts.append(\"all servers: cpu %s/%s unreserved%s\"");
        let a = cx.at(l, p::get(&f, "cpuMillis"))?;
        let a = cores(cx, l, &a)?;
        let b = cx.at(l, p::get(&c, "cpuMillis"))?;
        let b = cores(cx, l, &b)?;
        parts.push(s(&format!("all servers: cpu {a}/{b} unreserved{idle}")));
        let l = ln!("parts.append(\"mem %.1f/%s free\"");
        let m = p::or(cx.at(l, p::get(&f, "memBytes"))?, p::int(0));
        let m = cx.at(l, p::div(&m, &p::int(1 << 30)))?;
        let cm = cx.at(l, p::get(&c, "memBytes"))?;
        let cm = gb(cx, l, &cm)?;
        parts.push(s(&cx.at(l, p::pct("mem %.1f/%s free", &[m, s(&cm)]))?));
    }
    let l = ln!("if st.get(\"load\"):");
    if p::truthy(&g(cx, l, "load")?) {
        let l = ln!("parts.append(\"load \" + \" \".join(\"%.1f\" % x for x in st[\"load\"]))");
        let ld = cx.at(l, p::idx(&st, "load"))?;
        let mut xs = Vec::new();
        for x in cx.at(l, p::iter(&ld))? {
            xs.push(cx.at(l, p::pct("%.1f", &[x]))?);
        }
        parts.push(s(&format!("load {}", xs.join(" "))));
    }
    let l = ln!("d = st.get(\"disk\") or {}");
    let d = p::or(g(cx, l, "disk")?, dict());
    let l = ln!("if d.get(\"freeBytes\") is not None:");
    if !p::is_none(&cx.at(l, p::get(&d, "freeBytes"))?) {
        let l = ln!("s = \"disk %s free\" % gb(d[\"freeBytes\"])");
        let fb = cx.at(l, p::idx(&d, "freeBytes"))?;
        let mut line = format!("disk {} free", gb(cx, l, &fb)?);
        let l = ln!("if d.get(\"floorBytes\") is not None:");
        if !p::is_none(&cx.at(l, p::get(&d, "floorBytes"))?) {
            let l = ln!("s += \" (floor %s%s)\"");
            let x = cx.at(l, p::idx(&d, "floorBytes"))?;
            let x = gb(cx, l, &x)?;
            let def = if p::eq_s(&cx.at(l, p::get(&d, "floorFrom"))?, "default") {
                ", default"
            } else {
                ""
            };
            line += &format!(" (floor {x}{def})");
        }
        if p::truthy(&cx.at(ln!("if d.get(\"belowFloor\"):"), p::get(&d, "belowFloor"))?) {
            line += " BELOW FLOOR";
        }
        parts.push(s(&line));
    }
    let l = ln!("print(\"  \".join(parts))");
    let joined = cx.at(l, p::join("  ", &Value::Array(parts)))?;
    cx.print(&joined);
    let l = ln!("if (n.get(\"share\") or {}).get(\"line\"):");
    let share = p::or(cx.at(l, p::get(n, "share"))?, dict());
    if p::truthy(&cx.at(l, p::get(&share, "line"))?) {
        let l =
            ln!("print(\"    your share (jobs of this server only): \" + n[\"share\"][\"line\"])");
        let sh = cx.at(l, p::idx(n, "share"))?;
        let x = cx.at(l, p::idx(&sh, "line"))?;
        let t = cx.at(
            l,
            p::add(&s("    your share (jobs of this server only): "), &x),
        )?;
        cx.print(&p::str_of(&t));
    }
    let l = ln!("capc, parm = (st.get(\"caps\") or {}).get(\"cpuMillis\") or 0");
    let caps = p::or(g(cx, l, "caps")?, dict());
    let capc = p::or(cx.at(l, p::get(&caps, "cpuMillis"))?, p::int(0));
    let parm = p::or(g(cx, l, "parallelMax")?, p::int(0));
    let l = ln!("for j in st.get(\"running\") or []:");
    let running = p::or(g(cx, l, "running")?, list());
    for j in cx.at(l, p::iter(&running))? {
        let l = ln!("print(\"    running %s%-9s %-8s %-12s %s%s%s%s\"");
        let t = tag(cx, l, &j)?;
        let short = p::or(cx.at(l, p::get(&j, "short"))?, s("-"));
        let el = cx.at(l, p::get(&j, "elapsedSecs"))?;
        let el = mmss(cx, l, &el)?;
        let ws = p::or(cx.at(l, p::get(&j, "workspace"))?, s("(other)"));
        let argv = p::or(cx.at(l, p::get(&j, "argv"))?, s(""));
        let q = waited(cx, l, &j)?;
        let h = held(cx, l, &j, &capc, &parm)?;
        let w = pwhy(cx, l, &j)?;
        let line = cx.at(
            l,
            p::pct(
                "    running %s%-9s %-8s %-12s %s%s%s%s",
                &[s(&t), short, s(&el), ws, argv, s(&q), s(&h), s(&w)],
            ),
        )?;
        cx.print(&line);
    }
    let l = ln!("for j in st.get(\"queued\") or []:");
    let queued = p::or(g(cx, l, "queued")?, list());
    for j in cx.at(l, p::iter(&queued))? {
        let l = ln!("print(\"    queued  %s%-9s #%-3s %-8s %-6s %-12s %s%s%s\"");
        let t = tag(cx, l, &j)?;
        let short = p::or(cx.at(l, p::get(&j, "short"))?, s("-"));
        let pos = p::or(cx.at(l, p::get(&j, "position"))?, s("?"));
        let age = cx.at(l, p::get(&j, "ageSecs"))?;
        let age = mmss(cx, l, &age)?;
        let pin = cx.call(l, "<lambda>", |cx| {
            let lp = ln!("pin = lambda j: (\"pinned\" if j[\"pinned\"] else \"auto\") if \"pinned\" in j else \"\"");
            if cx.at(lp, p::has(&j, "pinned"))? {
                let x = cx.at(lp, p::idx(&j, "pinned"))?;
                Ok(if p::truthy(&x) { "pinned" } else { "auto" }.to_string())
            } else {
                Ok(String::new())
            }
        })?;
        let ws = p::or(cx.at(l, p::get(&j, "workspace"))?, s("(other)"));
        let argv = p::or(cx.at(l, p::get(&j, "argv"))?, s(""));
        let reason = if p::truthy(&cx.at(l, p::get(&j, "reason"))?) {
            let r = cx.at(l, p::idx(&j, "reason"))?;
            let x = cx.at(l, p::add(&s("  ("), &r))?;
            p::str_of(&cx.at(l, p::add(&x, &s(")")))?)
        } else {
            String::new()
        };
        let w = pwhy(cx, l, &j)?;
        let line = cx.at(
            l,
            p::pct(
                "    queued  %s%-9s #%-3s %-8s %-6s %-12s %s%s%s",
                &[
                    s(&t),
                    short,
                    pos,
                    s(&age),
                    s(&pin),
                    ws,
                    argv,
                    s(&reason),
                    s(&w),
                ],
            ),
        )?;
        cx.print(&line);
    }
    let l = ln!("if any(j.get(\"pinned\") for j in st.get(\"queued\") or []):");
    let queued = p::or(g(cx, l, "queued")?, list());
    let mut any = false;
    for j in cx.at(l, p::iter(&queued))? {
        if p::truthy(&cx.at(l, p::get(&j, "pinned"))?) {
            any = true;
            break;
        }
    }
    if any {
        cx.print(concat!(
            "      pinned jobs wait for this node only: ",
            cli_name!(),
            " compute move <id> --auto lets ",
            product_name!(),
            " move them"
        ));
    }
    Ok(())
}

fn tag(cx: &mut Cx, line: u32, j: &Value) -> Result<String, Raise> {
    cx.call(line, "<lambda>", |cx| {
        let l = ln!("tag = lambda j: ({\"ci\": \"CI \"");
        let t = cx.at(l, p::get(j, "tier"))?;
        let r = cx.at(
            l,
            lookup(
                &[("ci", "CI "), ("priority", "P  "), ("batch", "B  ")],
                &p::or(t, s("")),
            ),
        )?;
        Ok(r.unwrap_or_default())
    })
}

fn pwhy(cx: &mut Cx, line: u32, j: &Value) -> Result<String, Raise> {
    cx.call(line, "<lambda>", |cx| {
        let l = ln!("pwhy = lambda j:");
        if !p::truthy(&cx.at(l, p::get(j, "tierWhy"))?) {
            return Ok(String::new());
        }
        let tw = cx.at(l, p::idx(j, "tierWhy"))?;
        let sub = if p::truthy(&cx.at(l, p::get(j, "submitter"))?) {
            let x = cx.at(l, p::idx(j, "submitter"))?;
            cx.at(l, p::pct(" (%s)", &[x]))?
        } else {
            String::new()
        };
        cx.at(l, p::pct("\n      why: %s%s", &[tw, s(&sub)]))
    })
}

/// `waited`: the wait before a running job started, apart from its
/// running time, once it is a minute or more.
fn waited(cx: &mut Cx, line: u32, j: &Value) -> Result<String, Raise> {
    cx.call(line, "<lambda>", |cx| {
        let l = ln!("waited = lambda j:");
        let q = p::or(cx.at(l, p::get(j, "queuedSecs"))?, p::int(0));
        if !cx.at(l, p::cmp(">=", &q, &p::int(60)))? {
            return Ok(String::new());
        }
        let x = cx.at(l, p::idx(j, "queuedSecs"))?;
        let m = mmss(cx, l, &x)?;
        cx.at(l, p::pct("  (queued %s first)", &[s(&m)]))
    })
}

fn held(cx: &mut Cx, line: u32, j: &Value, capc: &Value, parm: &Value) -> Result<String, Raise> {
    cx.call(line, "held", |cx| {
        let l = ln!("if \"cpuMillis\" not in j or j[\"cpuMillis\"] is not None");
        let skip = !cx.at(l, p::has(j, "cpuMillis"))?
            || !p::is_none(&cx.at(l, p::idx(j, "cpuMillis"))?)
            || !p::truthy(capc)
            || cx.at(l, p::cmp("<", parm, &p::int(2)))?;
        if skip {
            return Ok(String::new());
        }
        let l = ln!("return \"  [no --cpus: reserves %s of %s CPUs]\"");
        let per = cx.at(l, p::floordiv(capc, parm))?;
        let a = cores(cx, l, &per)?;
        let b = cores(cx, l, capc)?;
        Ok(format!("  [no --cpus: reserves {a} of {b} CPUs]"))
    })
}
