//! `_compute_render <verb>` (`cli/src/compute-lib.sh`): the shell's
//! Python that prints a compute answer for people, one branch per verb,
//! ported line for line ([`super::pyrt`] does each operation as Python
//! does). Where the shell's Python raises, this prints what it printed
//! before and a Python 3.12 traceback naming the same line
//! ([`super::lines`]), exit 1.

use super::lines::{py_line, LIB};
use super::pyrt::{self as p, s, Exc, R};
use crate::families::{raised, Raise, Shown};
use crate::out::json::{self, Value};
use k2_client::cli_name;

macro_rules! ln {
    ($m:literal) => {
        const { py_line(LIB, "\n_compute_render() {", $m) }
    };
}

/// The renderer's state: what it printed, and the frames above the code
/// running now.
pub struct Cx {
    pub out: String,
    stack: Vec<(u32, &'static str)>,
    name: &'static str,
    now: i64,
}

impl Cx {
    pub fn new(now: i64) -> Cx {
        Cx {
            out: String::new(),
            stack: Vec::new(),
            name: "<module>",
            now,
        }
    }

    /// `r`, or its exception raised at `line` of the code running now.
    pub fn at<T>(&self, line: u32, r: R<T>) -> Result<T, Raise> {
        r.map_err(|e| self.raise(line, e))
    }

    pub fn raise(&self, line: u32, e: Exc) -> Raise {
        let mut frames = self.stack.clone();
        frames.push((line, self.name));
        Raise {
            frames,
            exception: e,
        }
    }

    /// `print(text)`.
    pub fn print(&mut self, text: &str) {
        self.out.push_str(text);
        self.out.push('\n');
    }

    /// Calls `name` from `line` of the code running now.
    pub fn call<T>(
        &mut self,
        line: u32,
        name: &'static str,
        f: impl FnOnce(&mut Cx) -> Result<T, Raise>,
    ) -> Result<T, Raise> {
        self.stack.push((line, self.name));
        let caller = std::mem::replace(&mut self.name, name);
        let r = f(self)?;
        self.name = caller;
        self.stack.pop();
        Ok(r)
    }

    pub fn now(&self) -> i64 {
        self.now
    }
}

/// `json.load(sys.stdin)` on the body (each byte that is not UTF-8 read
/// as U+FFFD), or its exception at `line`.
pub fn load(cx: &Cx, body: &[u8], line: u32) -> Result<Value, Raise> {
    let raw = String::from_utf8_lossy(body);
    json::parse(&raw).map_err(|e| {
        let why = if raw.trim_matches(crate::families::py_space).is_empty() {
            "Expecting value: line 1 column 1 (char 0)".to_string()
        } else {
            e.to_string()
        };
        cx.raise(line, format!("json.decoder.JSONDecodeError: {why}"))
    })
}

/// `_compute_render <verb>` on `body`. `full`: `K2_CP_FULL=1` (get's
/// `--full`). `now`: `int(time.time())`.
pub fn render(verb: &str, body: &[u8], full: bool, now: i64) -> Shown {
    let mut cx = Cx::new(now);
    match run(&mut cx, verb, body, full) {
        Ok(()) => Shown::out(cx.out),
        Err(r) => raised(std::mem::take(&mut cx.out), String::new(), &r),
    }
}

fn run(cx: &mut Cx, verb: &str, body: &[u8], full: bool) -> Result<(), Raise> {
    let v = load(cx, body, ln!("v = json.load(sys.stdin)"))?;
    match verb {
        "nodes" => nodes(cx, &v),
        "info" => info(cx, &v),
        "jobs" => jobs(cx, &v),
        "get" => get(cx, &v, full),
        "receipt" => receipt(cx, &v),
        "grants" => grants(cx, &v),
        "usage" => usage(cx, &v),
        "events" => events(cx, &v),
        "queue" => queue(cx, &v),
        "servers" => servers(cx, &v),
        other => panic!("_compute_render has no {other} here"),
    }
}

// ---- the helpers the branches share ----------------------------------------

/// `{…}.get(key, default)` on a table of strings: a key that can't be
/// hashed (a list, a dict) raises.
pub fn lookup(table: &[(&str, &str)], key: &Value) -> R<Option<String>> {
    match key {
        Value::Array(_) | Value::Object(_) => {
            Err(format!("TypeError: unhashable type: '{}'", p::tname(key)))
        }
        Value::Str(_) => Ok(table
            .iter()
            .find(|(k, _)| p::eq_s(key, k))
            .map(|(_, v)| v.to_string())),
        _ => Ok(None),
    }
}

fn gb(cx: &mut Cx, line: u32, n: &Value) -> Result<String, Raise> {
    cx.call(line, "gb", |cx| {
        let l = ln!("return \"%d GB\" % round((n or 0)");
        let x = cx.at(l, p::div(&p::or(n.clone(), p::int(0)), &p::int(1 << 30)))?;
        let r = cx.at(l, p::round0(&x))?;
        cx.at(l, p::pct("%d GB", &[r]))
    })
}

fn cores(cx: &mut Cx, line: u32, m: &Value) -> Result<String, Raise> {
    cx.call(line, "cores", |cx| {
        let l = ln!("return \"%g\" % ((m or 0) / 1000)");
        let x = cx.at(l, p::div(&p::or(m.clone(), p::int(0)), &p::int(1000)))?;
        cx.at(l, p::pct("%g", &[x]))
    })
}

fn ago(cx: &mut Cx, line: u32, t: &Value) -> Result<String, Raise> {
    cx.call(line, "ago", |cx| {
        if !p::truthy(t) {
            return Ok("-".into());
        }
        let l = ln!("d = int(time.time()) - int(t)");
        let d = i128::from(cx.now()) - cx.at(l, p::to_int(t))?;
        Ok(if d < 60 {
            "now".into()
        } else if d < 3600 {
            format!("{}m ago", d.div_euclid(60))
        } else if d < 86400 {
            format!("{}h ago", d.div_euclid(3600))
        } else {
            format!("{}d ago", d.div_euclid(86400))
        })
    })
}

fn dur(cx: &mut Cx, line: u32, sv: &Value) -> Result<String, Raise> {
    cx.call(line, "dur", |cx| {
        let l = ln!("s = int(s or 0)");
        let n = cx.at(l, p::to_int(&p::or(sv.clone(), p::int(0))))?;
        Ok(if n >= 3600 && n.rem_euclid(3600) == 0 {
            format!("{}h", n.div_euclid(3600))
        } else if n >= 60 && n.rem_euclid(60) == 0 {
            format!("{}m", n.div_euclid(60))
        } else {
            format!("{n}s")
        })
    })
}

fn tier_counts(cx: &mut Cx, line: u32, t: &Value, bare: bool) -> Result<String, Raise> {
    cx.call(line, "tier_counts", |cx| {
        if !p::is_dict(t) {
            return Ok(String::new());
        }
        let mut any = false;
        for k in ["ci", "priority", "normal", "batch"] {
            if p::truthy(&p::get(t, k).expect("a dict")) {
                any = true;
                break;
            }
        }
        if !any {
            return Ok(String::new());
        }
        let g = |k: &str| p::or(p::get(t, k).expect("a dict"), p::int(0));
        let l = ln!("line = \"CI %d · P %d · S %d\"");
        let mut out = cx.at(
            l,
            p::pct(
                "CI %d · P %d · S %d",
                &[g("ci"), g("priority"), g("normal")],
            ),
        )?;
        let batch = p::get(t, "batch").expect("a dict");
        if p::truthy(&batch) {
            out += &cx.at(
                ln!("line += \" · B %d\" % t[\"batch\"]"),
                p::pct(" · B %d", &[batch]),
            )?;
        }
        Ok(if bare { out } else { format!(" ({out})") })
    })
}

fn tier_tag(cx: &mut Cx, line: u32, t: &Value) -> Result<String, Raise> {
    cx.call(line, "tier_tag", |cx| {
        let key = p::or(t.clone(), s(""));
        let r = cx.at(
            ln!("return {\"ci\": \"CI\", \"priority\": \"P\", \"batch\": \"B\"}"),
            lookup(&[("ci", "CI"), ("priority", "P"), ("batch", "B")], &key),
        )?;
        Ok(r.unwrap_or_default())
    })
}

fn node_line(cx: &mut Cx, line: u32, n: &Value) -> Result<String, Raise> {
    cx.call(line, "node_line", |cx| node_line_body(cx, n))
}

fn node_line_body(cx: &mut Cx, n: &Value) -> Result<String, Raise> {
    let o = p::or(
        cx.at(ln!("o = n.get(\"offer\") or {}"), p::get(n, "offer"))?,
        Value::Object(Vec::new()),
    );
    let st = cx.at(ln!("st = n.get(\"state\")"), p::get(n, "state"))?;
    let mut status: String;
    if p::eq_s(&st, "pending") {
        let l = ln!("status = \"pending (code %s)\"");
        let sas = p::or(cx.at(l, p::get(n, "sas"))?, s("?"));
        status = cx.at(l, p::pct("pending (code %s)", &[sas]))?;
    } else if !p::truthy(&cx.at(ln!("elif not n.get(\"online\"):"), p::get(n, "online"))?) {
        status = "offline".into();
    } else {
        let l = ln!("status = \"online/%s\"");
        let route = p::or(cx.at(l, p::get(n, "liveRoute"))?, s("?"));
        status = cx.at(l, p::pct("online/%s", &[route]))?;
        let lc = ln!("if o.get(\"control\") and o[\"control\"] != \"active\":");
        let control = cx.at(lc, p::get(&o, "control"))?;
        if p::truthy(&control) && !p::eq_s(&cx.at(lc, p::idx(&o, "control"))?, "active") {
            let l = ln!("status = \"draining for update\" if");
            let by = p::or(cx.at(l, p::get(&o, "controlBy"))?, s(""));
            if cx.at(l, p::startswith(&by, "k2-node node.update"))? {
                status = "draining for update".into();
            } else {
                let c = cx.at(l, p::idx(&o, "control"))?;
                let tail = if p::truthy(&cx.at(l, p::get(&o, "controlBy"))?) {
                    let b = cx.at(l, p::idx(&o, "controlBy"))?;
                    cx.at(l, p::add(&s(" by "), &b))?
                } else {
                    s("")
                };
                status = p::str_of(&cx.at(l, p::add(&c, &tail))?);
            }
        } else if p::truthy(&cx.at(ln!("elif o.get(\"refusal\"):"), p::get(&o, "refusal"))?) {
            let l = ln!("status = \"waiting: \" + o[\"refusal\"]");
            let r = cx.at(l, p::idx(&o, "refusal"))?;
            let code = cx.at(l, p::get_or(&r, "code", s("")))?;
            status = p::str_of(&cx.at(l, p::add(&s("waiting: "), &code))?);
        } else if matches!(
            cx.at(
                ln!("elif o.get(\"available\") is False:"),
                p::get(&o, "available")
            )?,
            Value::Bool(false)
        ) {
            let l = ln!("status = \"unavailable: \" + \",\".join");
            let u = p::or(
                cx.at(l, p::get(&o, "unavailable"))?,
                Value::Array(Vec::new()),
            );
            status = format!("unavailable: {}", cx.at(l, p::join(",", &u))?);
        }
    }
    let l = ln!("if n.get(\"controllerPause\"):");
    if p::truthy(&cx.at(l, p::get(n, "controllerPause"))?) {
        let l = ln!("status += \" (server: %s)\" % n[\"controllerPause\"]");
        let cp = cx.at(l, p::idx(n, "controllerPause"))?;
        status += &cx.at(l, p::pct(" (server: %s)", &[cp]))?;
    }
    if p::truthy(&cx.at(ln!("if n.get(\"pinnedOnly\"):"), p::get(n, "pinnedOnly"))?) {
        status += " (pinned only)";
    }
    let l = ln!("os_arch = n.get(\"platform\") or");
    let mut os_arch = cx.at(l, p::get(n, "platform"))?;
    if !p::truthy(&os_arch) {
        let a = p::or(cx.at(l, p::get(n, "os"))?, s("?"));
        let b = p::or(cx.at(l, p::get(n, "arch"))?, s("?"));
        os_arch = s(&cx.at(l, p::pct("%s-%s", &[a, b]))?);
    }
    let caps = p::or(
        cx.at(ln!("caps = o.get(\"caps\") or {}"), p::get(&o, "caps"))?,
        Value::Object(Vec::new()),
    );
    let free = p::or(
        cx.at(ln!("free = o.get(\"free\") or {}"), p::get(&o, "free"))?,
        Value::Object(Vec::new()),
    );
    let l = ln!("idle = (\" · idle ~%s\" % cores(n[\"idleCpuMillis\"]))");
    let idle = if !p::is_none(&cx.at(l, p::get(n, "idleCpuMillis"))?) {
        let m = cx.at(l, p::idx(n, "idleCpuMillis"))?;
        format!(" · idle ~{}", cores(cx, l, &m)?)
    } else {
        String::new()
    };
    let l = ln!("room = n.get(\"room\") if isinstance(n.get(\"room\"), dict) else None");
    let room = cx.at(l, p::get(n, "room"))?;
    let room = p::is_dict(&room).then_some(room);
    let l = ln!("busy = {\"other_servers\"");
    let room_or = room.clone().unwrap_or(Value::Object(Vec::new()));
    let key = p::or(cx.at(l, p::get(&room_or, "busy"))?, s(""));
    let busy = cx
        .at(
            l,
            lookup(
                &[
                    ("other_servers", "busy (other servers) · "),
                    ("servers", "busy (this and other servers) · "),
                    ("this_server", "busy (this server) · "),
                ],
                &key,
            ),
        )?
        .unwrap_or_default();
    let res = if p::truthy(&o) {
        let l = ln!("res = (\"%sall servers: cpu %s/%s unreserved%s");
        let a = cx.at(l, p::get(&free, "cpu_millis"))?;
        let c1 = cores(cx, l, &a)?;
        let b = cx.at(l, p::get(&caps, "cpu_millis"))?;
        let c2 = cores(cx, l, &b)?;
        let mem = p::or(cx.at(l, p::get(&free, "mem_bytes"))?, p::int(0));
        let mem = cx.at(l, p::div(&mem, &p::int(1 << 30)))?;
        let capm = cx.at(l, p::get(&caps, "mem_bytes"))?;
        let g1 = gb(cx, l, &capm)?;
        let disk = cx.at(l, p::get(&free, "disk_free_bytes"))?;
        let g2 = gb(cx, l, &disk)?;
        cx.at(
            l,
            p::pct(
                "%sall servers: cpu %s/%s unreserved%s  mem %.1f/%s free  disk %s",
                &[s(&busy), s(&c1), s(&c2), s(&idle), mem, s(&g1), s(&g2)],
            ),
        )?
    } else {
        "-".into()
    };
    let par = cx.at(
        ln!("par = o.get(\"parallelMax\")"),
        p::get(&o, "parallelMax"),
    )?;
    let l = ln!("mine = n.get(\"mine\") if isinstance(n.get(\"mine\"), dict) else None");
    let mine = cx.at(l, p::get(n, "mine"))?;
    let mine = p::is_dict(&mine).then_some(mine);
    let par_or = if p::is_none(&par) {
        s("?")
    } else {
        par.clone()
    };
    let run = if let Some(room) = room.as_ref().filter(|r| p::truthy(r)) {
        let l = ln!("others = (\", other servers %d\" % room[\"otherServers\"])");
        let others = if p::truthy(&cx.at(l, p::get(room, "otherServers"))?) {
            let x = cx.at(l, p::idx(room, "otherServers"))?;
            cx.at(l, p::pct(", other servers %d", &[x]))?
        } else {
            String::new()
        };
        let l = ln!("run = \"runs %d/%s (this server %d%s%s)\"");
        let running = cx.at(l, p::get_or(room, "running", p::int(0)))?;
        let n_running = cx.at(l, p::get_or(n, "running", p::int(0)))?;
        let y = yours(cx, l, mine.as_ref(), "running")?;
        cx.at(
            l,
            p::pct(
                "runs %d/%s (this server %d%s%s)",
                &[running, par_or, n_running, s(&y), s(&others)],
            ),
        )?
    } else {
        let l = ln!("run = \"this server runs %d (of %s slots%s)\"");
        let n_running = cx.at(l, p::get_or(n, "running", p::int(0)))?;
        let y = yours(cx, l, mine.as_ref(), "running")?;
        cx.at(
            l,
            p::pct(
                "this server runs %d (of %s slots%s)",
                &[n_running, par_or, s(&y)],
            ),
        )?
    };
    let l = ln!("queued = \"queued %d (this server%s)\"");
    let nq = cx.at(l, p::get_or(n, "queued", p::int(0)))?;
    let y = yours(cx, l, mine.as_ref(), "queued")?;
    let queued = cx.at(l, p::pct("queued %d (this server%s)", &[nq, s(&y)]))?;
    let l = ln!("g = n.get(\"grant\")");
    let g = cx.at(l, p::get(n, "grant"))?;
    let grant = if p::is_dict(&g) {
        let l = ln!("grant = (\"  grant %s/job %s parallel %s queued%s\"");
        let mj = cx.at(l, p::get(&g, "maxJobSecs"))?;
        let d = dur(cx, l, &mj)?;
        let mp = cx.at(l, p::get(&g, "maxParallel"))?;
        let mq = cx.at(l, p::get(&g, "maxQueued"))?;
        let ci = if p::truthy(&cx.at(l, p::get(&g, "allowCi"))?) {
            " ci"
        } else {
            ""
        };
        cx.at(
            l,
            p::pct(
                "  grant %s/job %s parallel %s queued%s",
                &[s(&d), mp, mq, s(ci)],
            ),
        )?
    } else {
        String::new()
    };
    let reboot = if p::truthy(&cx.at(
        ln!("reboot = \"  reboot pending\""),
        p::get(n, "rebootPending"),
    )?) {
        "  reboot pending"
    } else {
        ""
    };
    let l = ln!("return \"%-14s %-19s %-24s %-64s  %s · %s%s%s  seen %s%s\"");
    let name = cx.at(l, p::idx(n, "name"))?;
    let st24 = s(&status.chars().take(24).collect::<String>());
    let qt = cx.at(l, p::get(n, "queueTiers"))?;
    let tiers = tier_counts(cx, l, &qt, false)?;
    let seen = cx.at(l, p::get(n, "lastSeen"))?;
    let seen = ago(cx, l, &seen)?;
    cx.at(
        l,
        p::pct(
            "%-14s %-19s %-24s %-64s  %s · %s%s%s  seen %s%s",
            &[
                name,
                os_arch,
                st24,
                s(&res),
                s(&run),
                s(&queued),
                s(&tiers),
                s(&grant),
                s(&seen),
                s(reboot),
            ],
        ),
    )
}

/// `yours(k)`: `(", yours %d" % mine.get(k, 0)) if mine else ""`.
fn yours(cx: &mut Cx, line: u32, mine: Option<&Value>, k: &str) -> Result<String, Raise> {
    cx.call(line, "<lambda>", |cx| match mine {
        Some(m) if p::truthy(m) => {
            let l = ln!("yours = lambda k: (\", yours %d\" % mine.get(k, 0)) if mine else \"\"");
            let x = cx.at(l, p::get_or(m, k, p::int(0)))?;
            cx.at(l, p::pct(", yours %d", &[x]))
        }
        _ => Ok(String::new()),
    })
}

fn update_lines(cx: &mut Cx, line: u32, n: &Value) -> Result<Vec<String>, Raise> {
    cx.call(line, "update_lines", |cx| {
        let l = ln!("u = n.get(\"update\") if isinstance(n.get(\"update\"), dict) else {}");
        let u = cx.at(l, p::get(n, "update"))?;
        let u = if p::is_dict(&u) { u } else { Value::Object(Vec::new()) };
        let l = ln!("r = n.get(\"rollout\") if isinstance(n.get(\"rollout\"), dict) else {}");
        let r = cx.at(l, p::get(n, "rollout"))?;
        let r = if p::is_dict(&r) { r } else { Value::Object(Vec::new()) };
        let have = p::or(cx.at(ln!("have = n.get(\"nodeVersion\") or \"?\""), p::get(n, "nodeVersion"))?, s("?"));
        let g = |v: &Value, k: &str| p::get(v, k).expect("a dict");
        let d = if p::eq_s(&g(&u, "state"), "staged") && p::is_dict(&g(&u, "drain")) {
            Some(g(&u, "drain"))
        } else {
            None
        };
        let end = match &d {
            Some(dv) if p::truthy(dv) && p::truthy(&g(dv, "jobsEndBy")) => {
                let l = ln!("end = (\"longest ends by \" + time.strftime");
                let t = cx.at(l, p::idx(dv, "jobsEndBy"))?;
                let now = cx.now();
                format!("longest ends by {}", cx.at(l, p::strftime("%b %d %H:%M", &t, now))?)
            }
            _ => "end time unknown".into(),
        };
        let phase: Value = match &d {
            Some(dv) if p::truthy(dv) => {
                let l = ln!("phase = (\": draining for update (%s jobs running, %s)\"");
                s(&cx.at(
                    l,
                    p::pct(
                        ": draining for update (%s jobs running, %s)",
                        &[g(dv, "running"), s(&end)],
                    ),
                )?)
            }
            _ => {
                let key = p::or(g(&u, "state"), s(""));
                let l = ln!("phase = (\": draining for update (%s jobs running, %s)\"");
                cx.at(
                    l,
                    lookup(
                        &[
                            ("offered", " (sent)"),
                            ("staged", " (staged)"),
                            ("swapped", " (health check)"),
                        ],
                        &key,
                    ),
                )?
                .map_or(Value::Null, |t| s(&t))
            }
        };
        let version = g(&u, "version");
        if p::truthy(&phase) && p::truthy(&version) && !p::eq(&version, &have) {
            let l = ln!("return [\"%-14s k2-node v%s -> %s%s\"");
            let uv = cx.at(l, p::idx(&u, "version"))?;
            return Ok(vec![cx.at(
                l,
                p::pct("%-14s k2-node v%s -> %s%s", &[s(""), have, uv, phase]),
            )?]);
        }
        let code = g(&r, "code");
        if (p::eq_s(&code, "waiting_turn") || p::eq_s(&code, "waiting_idle"))
            && p::eq_s(&g(&r, "node"), "pending")
            && p::eq_s(&g(&r, "state"), "running")
        {
            let l = ln!("return [\"%-14s k2-node v%s -> %s (update staged, %s)\"");
            let to = p::or(g(&r, "toVersion"), s("?"));
            let why = if p::eq_s(&code, "waiting_turn") {
                "waiting its turn"
            } else {
                "waiting for its running jobs to end"
            };
            return Ok(vec![cx.at(
                l,
                p::pct(
                    "%-14s k2-node v%s -> %s (update staged, %s)",
                    &[s(""), have, to, s(why)],
                ),
            )?]);
        }
        if p::eq_s(&code, "node_needs_manual_install") {
            let l = ln!("return [\"%-14s k2-node v%s: needs one manual install");
            return Ok(vec![cx.at(
                l,
                p::pct(
                    "%-14s k2-node v%s: needs one manual install (the updater fix is in 0.46.4): sudo k2-node setup at the machine",
                    &[s(""), have],
                ),
            )?]);
        }
        if p::eq_s(&g(&r, "node"), "rolled_back") {
            let l = ln!("return [\"%-14s k2-node %s rolled back here;");
            let to = p::or(g(&r, "toVersion"), s(""));
            return Ok(vec![cx.at(
                l,
                p::pct(
                    concat!("%-14s k2-node %s rolled back here; the rollout paused (", cli_name!(), " compute node rollout)"),
                    &[s(""), to],
                ),
            )?]);
        }
        Ok(Vec::new())
    })
}

fn reserve_lines(cx: &mut Cx, line: u32, n: &Value) -> Result<Vec<String>, Raise> {
    cx.call(line, "reserve_lines", |cx| {
        let l = ln!("rows = n.get(\"reservedBy\") if isinstance");
        let rows = cx.at(l, p::get(n, "reservedBy"))?;
        let rows = match rows {
            Value::Array(items) => items,
            _ => Vec::new(),
        };
        let l = ln!("room = n.get(\"room\") if isinstance(n.get(\"room\"), dict) else {}");
        let room = cx.at(l, p::get(n, "room"))?;
        let room = if p::is_dict(&room) {
            room
        } else {
            Value::Object(Vec::new())
        };
        let others = p::or(p::get(&room, "otherServers").expect("a dict"), p::int(0));
        if rows.is_empty() && !p::truthy(&others) {
            return Ok(Vec::new());
        }
        let mut shown = Vec::new();
        let lshown = ln!("shown = [one(r) for r in rows[:4]]");
        for r in rows.iter().take(4) {
            shown.push(cx.call(lshown, "one", |cx| reserve_one(cx, r))?);
        }
        if rows.len() > 4 {
            let l = ln!("shown.append(\"%d more (");
            let name = cx.at(l, p::get(n, "name"))?;
            shown.push(cx.at(
                l,
                p::pct(
                    concat!("%d more (", cli_name!(), " compute jobs --node %s)"),
                    &[p::int(rows.len() as i128 - 4), name],
                ),
            )?);
        }
        if p::truthy(&others) {
            let l = ln!("shown.append(\"%d job(s) of other servers\" % others)");
            shown.push(cx.at(l, p::pct("%d job(s) of other servers", &[others]))?);
        }
        Ok(vec![format!("{:14} reserved by {}", "", shown.join(", "))])
    })
}

fn reserve_one(cx: &mut Cx, r: &Value) -> Result<String, Raise> {
    let l = ln!("bits = [r.get(\"state\") or \"?\"]");
    let mut bits = vec![p::or(cx.at(l, p::get(r, "state"))?, s("?"))];
    let l = ln!("if r.get(\"startedAt\"):");
    if p::truthy(&cx.at(l, p::get(r, "startedAt"))?) {
        let l = ln!("bits.append(\"started \" + ago(r[\"startedAt\"]))");
        let t = cx.at(l, p::idx(r, "startedAt"))?;
        bits.push(s(&format!("started {}", ago(cx, l, &t)?)));
    } else if p::truthy(&cx.at(ln!("elif r.get(\"createdAt\"):"), p::get(r, "createdAt"))?) {
        let l = ln!("bits.append(\"queued \" + ago(r[\"createdAt\"]))");
        let t = cx.at(l, p::idx(r, "createdAt"))?;
        bits.push(s(&format!("queued {}", ago(cx, l, &t)?)));
    }
    if p::truthy(&cx.at(ln!("if r.get(\"cpuMillis\"):"), p::get(r, "cpuMillis"))?) {
        let l = ln!("bits.append(\"%s CPUs\" % cores(r[\"cpuMillis\"]))");
        let m = cx.at(l, p::idx(r, "cpuMillis"))?;
        bits.push(s(&format!("{} CPUs", cores(cx, l, &m)?)));
    }
    if p::truthy(&cx.at(ln!("if r.get(\"memBytes\"):"), p::get(r, "memBytes"))?) {
        let l = ln!("bits.append(\"%.1f GB\" % (r[\"memBytes\"] / (1 << 30)))");
        let m = cx.at(l, p::idx(r, "memBytes"))?;
        let x = cx.at(l, p::div(&m, &p::int(1 << 30)))?;
        bits.push(s(&cx.at(l, p::pct("%.1f GB", &[x]))?));
    }
    let l = ln!("return \"%s (%s)\" % (r.get(\"short\") or \"?\", \", \".join(bits))");
    let short = p::or(cx.at(l, p::get(r, "short"))?, s("?"));
    let joined = cx.at(l, p::join(", ", &Value::Array(bits)))?;
    cx.at(l, p::pct("%s (%s)", &[short, s(&joined)]))
}

fn leftover_lines(cx: &mut Cx, line: u32, n: &Value) -> Result<Vec<String>, Raise> {
    cx.call(line, "leftover_lines", |cx| {
        let l = ln!("l = n.get(\"leftovers\") if isinstance");
        let lv = cx.at(l, p::get(n, "leftovers"))?;
        if !p::is_dict(&lv) || !p::truthy(&lv) || !p::truthy(&p::get(&lv, "count").expect("a dict")) {
            return Ok(Vec::new());
        }
        let l = ln!("out = [\"%-14s %d leftover process(es)");
        let count = cx.at(l, p::idx(&lv, "count"))?;
        let mut out = vec![cx.at(
            l,
            p::pct(
                "%-14s %d leftover process(es) of the job user with no live job (not killed; the owner ends them at the machine)",
                &[s(""), count],
            ),
        )?];
        let l = ln!("for p in (l.get(\"procs\") or [])[:3]:");
        let procs = p::or(p::get(&lv, "procs").expect("a dict"), Value::Array(Vec::new()));
        let procs = cx.at(l, p::slice(&procs, None, Some(3)))?;
        for pr in cx.at(l, p::iter(&procs))? {
            let l = ln!("out.append(\"%-14s   pid %s, %s old%s: %s\"");
            let pid = cx.at(l, p::get(&pr, "pid"))?;
            let a = cx.at(l, p::get(&pr, "age_secs"))?;
            let age = cx.call(l, "age", |cx| leftover_age(cx, &a))?;
            let job = if p::truthy(&cx.at(l, p::get(&pr, "job_id"))?) {
                let j = cx.at(l, p::idx(&pr, "job_id"))?;
                let j = cx.at(l, p::slice(&j, None, Some(8)))?;
                cx.at(l, p::pct(", from job %s", &[j]))?
            } else {
                String::new()
            };
            let cmd = p::or(cx.at(l, p::get(&pr, "cmd"))?, s(""));
            let cmd = cx.at(l, p::slice(&cmd, None, Some(90)))?;
            out.push(cx.at(
                l,
                p::pct(
                    "%-14s   pid %s, %s old%s: %s",
                    &[s(""), pid, s(&age), s(&job), cmd],
                ),
            )?);
        }
        Ok(out)
    })
}

fn leftover_age(cx: &mut Cx, sv: &Value) -> Result<String, Raise> {
    if p::is_none(sv) {
        return Ok("?".into());
    }
    let l = ln!("return \"%dd\" % (s // 86400) if s >= 86400");
    let r = if cx.at(l, p::cmp(">=", sv, &p::int(86400)))? {
        ("%dd", 86400)
    } else if cx.at(l, p::cmp(">=", sv, &p::int(3600)))? {
        ("%dh", 3600)
    } else {
        ("%dm", 60)
    };
    let q = cx.at(l, p::floordiv(sv, &p::int(r.1)))?;
    cx.at(l, p::pct(r.0, &[q]))
}

fn when_col(cx: &mut Cx, line: u32, j: &Value) -> Result<String, Raise> {
    cx.call(line, "when_col", |cx| {
        if p::truthy(&cx.at(ln!("if j.get(\"startedAt\"):"), p::get(j, "startedAt"))?) {
            let l = ln!("w = \"started %s\" % ago(j[\"startedAt\"])");
            let t = cx.at(l, p::idx(j, "startedAt"))?;
            let mut w = format!("started {}", ago(cx, l, &t)?);
            if p::truthy(&cx.at(
                ln!("        if j.get(\"createdAt\"):"),
                p::get(j, "createdAt"),
            )?) {
                let l = ln!("w += \" (queued %s)\" % ago(j[\"createdAt\"])");
                let t = cx.at(l, p::idx(j, "createdAt"))?;
                w += &format!(" (queued {})", ago(cx, l, &t)?);
            }
            return Ok(w);
        }
        let l = ln!(
            "return \"queued %s\" % ago(j.get(\"createdAt\")) if j.get(\"createdAt\") else \"-\""
        );
        let c = cx.at(l, p::get(j, "createdAt"))?;
        if p::truthy(&c) {
            Ok(format!("queued {}", ago(cx, l, &c)?))
        } else {
            Ok("-".into())
        }
    })
}

fn share_lines(cx: &mut Cx, line: u32, n: &Value) -> Result<Vec<String>, Raise> {
    cx.call(line, "share_lines", |cx| {
        let sh = cx.at(ln!("    s = n.get(\"share\")"), p::get(n, "share"))?;
        if !p::is_dict(&sh) {
            return Ok(Vec::new());
        }
        let g = |k: &str| p::get(&sh, k).expect("a dict");
        let mut until = String::new();
        if p::truthy(&g("expiresAt")) {
            let l = ln!("until = \"   until \" + time.strftime(\"%b %d\"");
            let now = cx.now();
            until = format!("   until {}", cx.at(l, p::strftime("%b %d", &g("expiresAt"), now))?);
        }
        let mut head = if p::eq_s(&g("role"), "tenant") {
            let l = ln!("head = \"shared by %s (%s servers)%s\"");
            cx.at(
                l,
                p::pct(
                    "shared by %s (%s servers)%s",
                    &[p::or(g("ownerLabel"), s("?")), p::or(g("servers"), s("?")), s(&until)],
                ),
            )?
        } else {
            let l = ln!("head = \"shared with %d other server(s)\"");
            let x = cx.at(l, p::sub(&p::or(g("servers"), p::int(1)), &p::int(1)))?;
            let m = cx.at(l, p::max2(&x, &p::int(0)))?;
            cx.at(l, p::pct("shared with %d other server(s)", &[m]))?
        };
        if p::truthy(&g("state")) && !p::eq_s(&g("state"), "active") {
            let l = ln!("head += \"   your share: \" + s[\"state\"]");
            let st = cx.at(l, p::add(&s("   your share: "), &g("state")))?;
            head += &p::str_of(&st);
        }
        if p::truthy(&g("skewSecs")) {
            let l = ln!("head += \"   clock skew %+ds");
            head += &cx.at(l, p::pct("   clock skew %+ds (node vs this server)", &[g("skewSecs")]))?;
        }
        let mut out = vec![format!("{:14} {head}", "")];
        if p::truthy(&g("line")) {
            let l = ln!("out.append(\"%-14s your share (jobs of this server only): %s\"");
            out.push(cx.at(
                l,
                p::pct(
                    "%-14s your share (jobs of this server only): %s",
                    &[s(""), g("line")],
                ),
            )?);
        }
        if matches!(g("priority"), Value::Bool(false)) {
            out.push(format!("{:14} your --tier priority runs as Normal here (its owner has not set priority=true for this server)", ""));
        }
        Ok(out)
    })
}

// ---- the branches ---------------------------------------------------------

fn nodes(cx: &mut Cx, v: &Value) -> Result<(), Raise> {
    let rows = cx.at(
        ln!("rows = v.get(\"nodes\", [])"),
        p::get_or(v, "nodes", Value::Array(Vec::new())),
    )?;
    let l = ln!("for c in v.get(\"criticalPath\") or []:");
    let crit = cx.at(l, p::get(v, "criticalPath"))?;
    for c in cx.at(l, p::iter_or_empty(crit))? {
        let l = ln!("print(\"critical path (Priority): %s (%s jobs)%s\"");
        let who = p::or(cx.at(l, p::get(&c, "submitter"))?, s("?"));
        let jobs = cx.at(l, p::get(&c, "jobs"))?;
        let ws = if p::truthy(&cx.at(l, p::get(&c, "workspace"))?) {
            let w = cx.at(l, p::idx(&c, "workspace"))?;
            cx.at(l, p::pct(" in %s", &[w]))?
        } else {
            String::new()
        };
        let t = cx.at(
            l,
            p::pct(
                "critical path (Priority): %s (%s jobs)%s",
                &[who, jobs, s(&ws)],
            ),
        )?;
        cx.print(&t);
    }
    if !p::truthy(&rows) {
        let l = ln!("print(\"(no compute nodes%s)\"");
        let wc = cx.at(l, p::get(v, "waitingCodes"))?;
        let tail = if p::is_none(&wc) {
            ""
        } else {
            concat!("; add one with: ", cli_name!(), " compute node add <name>")
        };
        cx.print(&format!("(no compute nodes{tail})"));
    }
    let l = ln!("    for n in rows:");
    for n in cx.at(l, p::iter(&rows))? {
        let l = ln!("print(node_line(n) + (\"  [off the node list]\"");
        let line = node_line(cx, l, &n)?;
        let off = matches!(cx.at(l, p::get(&n, "allowed"))?, Value::Bool(false));
        cx.print(&format!(
            "{line}{}",
            if off { "  [off the node list]" } else { "" }
        ));
        let l = ln!("for l in reserve_lines(n) + leftover_lines(n):");
        let mut lines = reserve_lines(cx, l, &n)?;
        lines.extend(leftover_lines(cx, l, &n)?);
        for x in lines {
            cx.print(&x);
        }
        let l = ln!("        for l in share_lines(n):");
        for x in share_lines(cx, l, &n)? {
            cx.print(&x);
        }
        let l = ln!("        for l in update_lines(n):");
        for x in update_lines(cx, l, &n)? {
            cx.print(&x);
        }
    }
    let l = ln!("for c in v.get(\"waitingCodes\") or []:");
    let wc = cx.at(l, p::get(v, "waitingCodes"))?;
    for c in cx.at(l, p::iter_or_empty(wc))? {
        let l = ln!("print(\"code waiting for %s (expires %s)\"");
        let name = cx.at(l, p::idx(&c, "name"))?;
        let t = cx.at(l, p::idx(&c, "expiresAt"))?;
        let now = cx.now();
        let at = cx.at(l, p::strftime("%H:%M", &t, now))?;
        let t = cx.at(
            l,
            p::pct("code waiting for %s (expires %s)", &[name, s(&at)]),
        )?;
        cx.print(&t);
    }
    let l = ln!("if isinstance(v.get(\"allowNodes\"), list):");
    let an = cx.at(l, p::get(v, "allowNodes"))?;
    if p::is_list(&an) {
        let l = ln!("print(\"node list (%s): jobs go only to %s");
        let ws = p::or(
            cx.at(l, p::get(v, "allowNodesWorkspace"))?,
            s("this workspace"),
        );
        let joined = cx.at(l, p::join(", ", &an))?;
        let to = if joined.is_empty() {
            "nothing".to_string()
        } else {
            joined
        };
        let t = cx.at(
            l,
            p::pct(
                concat!(
                    "node list (%s): jobs go only to %s (",
                    cli_name!(),
                    " compute allow-nodes)"
                ),
                &[ws, s(&to)],
            ),
        )?;
        cx.print(&t);
    }
    Ok(())
}

fn info(cx: &mut Cx, v: &Value) -> Result<(), Raise> {
    let line = node_line(cx, ln!("    print(node_line(v))"), v)?;
    cx.print(&line);
    let l = ln!("    for l in reserve_lines(v) + leftover_lines(v):");
    let mut lines = reserve_lines(cx, l, v)?;
    lines.extend(leftover_lines(cx, l, v)?);
    for x in lines {
        cx.print(&x);
    }
    for x in share_lines(cx, ln!("    for l in share_lines(v):"), v)? {
        cx.print(&x);
    }
    let h = cx.at(ln!("h = v.get(\"host\")"), p::get(v, "host"))?;
    if p::is_dict(&h) && p::truthy(&p::get(&h, "line").expect("a dict")) {
        let l = ln!("print(\"host: \" + h[\"line\"])");
        let x = cx.at(l, p::idx(&h, "line"))?;
        let x = cx.at(l, p::add(&s("host: "), &x))?;
        cx.print(&p::str_of(&x));
    }
    let pk = cx.at(ln!("pk = v.get(\"packages\")"), p::get(v, "packages"))?;
    if p::is_dict(&pk) {
        let g = |k: &str| p::get(&pk, k).expect("a dict");
        let n_open = p::or(g("openRequests"), p::int(0));
        let mut bits = vec![p::or(g("manager"), s("?"))];
        if p::truthy(&g("policy")) {
            let l = ln!("bits.append(\"policy for this server: \" + pk[\"policy\"])");
            bits.push(cx.at(l, p::add(&s("policy for this server: "), &g("policy")))?);
        }
        let l = ln!("bits.append(\"%d open request%s\"");
        let plural = if p::eq(&n_open, &p::int(1)) { "" } else { "s" };
        bits.push(s(
            &cx.at(l, p::pct("%d open request%s", &[n_open, s(plural)]))?
        ));
        let l = ln!("print(\"packages: %s (");
        let joined = cx.at(l, p::join(" · ", &Value::Array(bits)))?;
        let name = cx.at(l, p::get(v, "name"))?;
        let t = cx.at(
            l,
            p::pct(
                concat!("packages: %s (", cli_name!(), " compute node packages %s)"),
                &[s(&joined), name],
            ),
        )?;
        cx.print(&t);
    }
    let o = p::or(
        cx.at(ln!("    o = v.get(\"offer\") or {}"), p::get(v, "offer"))?,
        Value::Object(Vec::new()),
    );
    let l = ln!("nv = v.get(\"nodeVersion\") or o.get(\"node_version\")");
    let mut nv = cx.at(l, p::get(v, "nodeVersion"))?;
    if !p::truthy(&nv) {
        nv = cx.at(l, p::get(&o, "node_version"))?;
    }
    let l = ln!("if nv or o.get(\"features\") is not None:");
    if p::truthy(&nv) || !p::is_none(&cx.at(l, p::get(&o, "features"))?) {
        let l = ln!("feats = o.get(\"features\") or []");
        let feats = p::or(cx.at(l, p::get(&o, "features"))?, Value::Array(Vec::new()));
        let what = if !p::truthy(&cx.at(ln!("if not v.get(\"shared\"):"), p::get(v, "shared"))?) {
            "this server orders its queue: every line works"
        } else if cx.at(ln!("elif \"tiers2\" in feats:"), contains(&feats, "tiers2"))? {
            "shared; the node orders: lines, turns, Batch"
        } else if cx.at(ln!("elif \"tiers\" in feats:"), contains(&feats, "tiers"))? {
            "shared; the node orders: Priority only (Batch waits for an idle node; no turns until k2-node 0.46.6)"
        } else {
            "shared; the node orders without tiers (Priority has no effect there)"
        };
        let l = ln!("print(\"k2-node %s (protocol %s): %s\"");
        let proto = cx.at(l, p::get_or(&o, "protocol", s("?")))?;
        let t = cx.at(
            l,
            p::pct(
                "k2-node %s (protocol %s): %s",
                &[p::or(nv, s("?")), proto, s(what)],
            ),
        )?;
        cx.print(&t);
    }
    let l = ln!("if o.get(\"tools\"):");
    if p::truthy(&cx.at(l, p::get(&o, "tools"))?) {
        let l = ln!("print(\"tools: \" + \"  \".join(");
        let tools = cx.at(l, p::idx(&o, "tools"))?;
        let mut parts = Vec::new();
        for (k, val) in cx.at(l, p::sorted_items(&tools))? {
            parts.push(cx.at(l, p::pct("%s %s", &[k, val]))?);
        }
        cx.print(&format!("tools: {}", parts.join("  ")));
    }
    let l = ln!("if v.get(\"routes\"):");
    if p::truthy(&cx.at(l, p::get(v, "routes"))?) {
        let l = ln!("print(\"routes: \" + \" \".join(v[\"routes\"]))");
        let r = cx.at(l, p::idx(v, "routes"))?;
        let j = cx.at(l, p::join(" ", &r))?;
        cx.print(&format!("routes: {j}"));
    }
    let l = ln!("for g in v.get(\"grants\") or []:");
    let gs = cx.at(l, p::get(v, "grants"))?;
    for g in cx.at(l, p::iter_or_empty(gs))? {
        let l = ln!("print(\"grant: %s  %s/job  %s parallel%s");
        let ws = cx.at(l, p::get(&g, "workspace"))?;
        let mj = cx.at(l, p::idx(&g, "maxJobSecs"))?;
        let d = dur(cx, l, &mj)?;
        let mp = cx.at(l, p::idx(&g, "maxParallel"))?;
        let per = if p::truthy(&cx.at(l, p::get(&g, "submitterMaxRunning"))?) {
            let x = cx.at(l, p::idx(&g, "submitterMaxRunning"))?;
            cx.at(l, p::pct("  %s per submitter", &[x]))?
        } else {
            String::new()
        };
        let mq = cx.at(l, p::idx(&g, "maxQueued"))?;
        let md = cx.at(l, p::idx(&g, "maxDiskGb"))?;
        let ci = if p::truthy(&cx.at(l, p::get(&g, "allowCi"))?) {
            "  CI allowed"
        } else {
            ""
        };
        let t = cx.at(
            l,
            p::pct(
                "grant: %s  %s/job  %s parallel%s  %s queued  %s GB disk%s",
                &[ws, s(&d), mp, s(&per), mq, md, s(ci)],
            ),
        )?;
        cx.print(&t);
    }
    let l = ln!("for j in v.get(\"recentJobs\") or []:");
    let rj = cx.at(l, p::get(v, "recentJobs"))?;
    for j in cx.at(l, p::iter_or_empty(rj))? {
        let l = ln!("print(\"job %s  %-9s exit %-4s %s\"");
        let short = cx.at(l, p::idx(&j, "short"))?;
        let state = cx.at(l, p::idx(&j, "state"))?;
        let exit = cx.at(l, p::get(&j, "exitCode"))?;
        let exit = if p::is_none(&exit) {
            s("-")
        } else {
            cx.at(l, p::idx(&j, "exitCode"))?
        };
        let argv = p::or(cx.at(l, p::get(&j, "argv"))?, Value::Array(Vec::new()));
        let argv = cx.at(l, p::join(" ", &argv))?;
        let argv = cx.at(l, p::slice(&s(&argv), None, Some(60)))?;
        let t = cx.at(
            l,
            p::pct("job %s  %-9s exit %-4s %s", &[short, state, exit, argv]),
        )?;
        cx.print(&t);
    }
    Ok(())
}

/// `x in v` for a string `x`.
fn contains(v: &Value, x: &str) -> R<bool> {
    p::has(v, x)
}

fn jobs(cx: &mut Cx, v: &Value) -> Result<(), Raise> {
    let rows = cx.at(
        ln!("rows = v.get(\"jobs\", [])"),
        p::get_or(v, "jobs", Value::Array(Vec::new())),
    )?;
    if !p::truthy(&rows) {
        cx.print("(no jobs)");
    }
    let l = ln!("    for j in rows:\n        # 0.46.1");
    for j in cx.at(l, p::iter(&rows))? {
        let l = ln!("who = j.get(\"submitter\") or j.get(\"submittedBy\") or \"-\"");
        let mut who = cx.at(l, p::get(&j, "submitter"))?;
        if !p::truthy(&who) {
            who = cx.at(l, p::get(&j, "submittedBy"))?;
        }
        let mut who = p::or(who, s("-"));
        if p::eq_s(
            &cx.at(
                ln!("if j.get(\"via\") == \"owner_token\":\n            who +="),
                p::get(&j, "via"),
            )?,
            "owner_token",
        ) {
            let l = ln!("who += \" (owner token)\"");
            who = cx.at(l, p::iadd(&who, &s(" (owner token)")))?;
        }
        let l = ln!("state = j[\"state\"]");
        let mut state = cx.at(l, p::idx(&j, "state"))?;
        let l = ln!("node, reason = j.get(\"node\") or \"?\", j.get(\"reason\") or \"\"");
        let mut node = p::or(cx.at(l, p::get(&j, "node"))?, s("?"));
        let mut reason = p::or(cx.at(l, p::get(&j, "reason"))?, s(""));
        let l =
            ln!("if state == \"queued\" and (j.get(\"wait\") or {}).get(\"reason\") == \"held\":");
        let held = p::eq_s(&state, "queued") && {
            let w = p::or(cx.at(l, p::get(&j, "wait"))?, Value::Object(Vec::new()));
            p::eq_s(&cx.at(l, p::get(&w, "reason"))?, "held")
        };
        let l = ln!("elif state == \"queued\" and \"pinned\" in j:");
        if held {
            // 0.46.17: an auto job held on this server, on no node yet.
            state = s("held (auto)");
            node = s("-");
            reason = s("waits for a free slot");
        } else if p::eq_s(&state, "queued") && cx.at(l, p::has(&j, "pinned"))? {
            let l = ln!("state += \" (pin)\" if j[\"pinned\"] else \" (auto)\"");
            let pinned = cx.at(l, p::idx(&j, "pinned"))?;
            let tail = if p::truthy(&pinned) {
                " (pin)"
            } else {
                " (auto)"
            };
            state = cx.at(l, p::iadd(&state, &s(tail)))?;
        }
        let l = ln!("print(\"%s  %-2s %-12s %-15s exit %-4s %-22s %-34s %-16s %-24s %s\"");
        let short = cx.at(l, p::idx(&j, "short"))?;
        let tier = cx.at(l, p::get(&j, "tier"))?;
        let tag = tier_tag(cx, l, &tier)?;
        let node = cx.at(l, p::slice(&node, None, Some(12)))?;
        let exit = cx.at(l, p::get(&j, "exitCode"))?;
        let exit = if p::is_none(&exit) {
            s("-")
        } else {
            cx.at(l, p::idx(&j, "exitCode"))?
        };
        let when = when_col(cx, l, &j)?;
        let jtag = p::or(cx.at(l, p::get(&j, "tag"))?, s("-"));
        let jtag = cx.at(l, p::slice(&jtag, None, Some(16)))?;
        let who = cx.at(l, p::slice(&who, None, Some(24)))?;
        let argv = p::or(cx.at(l, p::get(&j, "argv"))?, Value::Array(Vec::new()));
        let argv = cx.at(l, p::join(" ", &argv))?;
        let argv = cx.at(l, p::slice(&s(&argv), None, Some(60)))?;
        let t = cx.at(
            l,
            p::pct(
                "%s  %-2s %-12s %-15s exit %-4s %-22s %-34s %-16s %-24s %s",
                &[
                    short,
                    s(&tag),
                    node,
                    state,
                    exit,
                    reason,
                    s(&when),
                    jtag,
                    who,
                    argv,
                ],
            ),
        )?;
        cx.print(&t);
    }
    Ok(())
}

fn receipt(cx: &mut Cx, v: &Value) -> Result<(), Raise> {
    let l = ln!("print(json.dumps(v.get(\"receipt\"), indent=2))");
    let r = cx.at(l, p::get(v, "receipt"))?;
    let t = cx.at(l, p::dumps(&r, false, Some(2)))?;
    cx.print(&t);
    let l = ln!("print(\"verified: %s  (%s)\"");
    let ok = if p::truthy(&cx.at(l, p::get(v, "verified"))?) {
        "yes"
    } else {
        "NO"
    };
    let proves = cx.at(l, p::get(v, "proves"))?;
    let t = cx.at(l, p::pct("verified: %s  (%s)", &[s(ok), proves]))?;
    cx.print(&t);
    Ok(())
}

fn grants(cx: &mut Cx, v: &Value) -> Result<(), Raise> {
    let rows = cx.at(
        ln!("rows = v.get(\"grants\", [])"),
        p::get_or(v, "grants", Value::Array(Vec::new())),
    )?;
    if !p::truthy(&rows) {
        cx.print("(no grants)");
    }
    let l = ln!("    for g in rows:\n        print(\"%-14s %-20s max");
    for g in cx.at(l, p::iter(&rows))? {
        let l = ln!("print(\"%-14s %-20s max %ss/job");
        let node = cx.at(l, p::get(&g, "node"))?;
        let ws = cx.at(l, p::get(&g, "workspace"))?;
        let mj = cx.at(l, p::idx(&g, "maxJobSecs"))?;
        let mp = cx.at(l, p::idx(&g, "maxParallel"))?;
        let mq = cx.at(l, p::idx(&g, "maxQueued"))?;
        let md = cx.at(l, p::idx(&g, "maxDiskGb"))?;
        let ci = if p::truthy(&cx.at(l, p::get(&g, "allowCi"))?) {
            "  ci"
        } else {
            ""
        };
        let by = cx.at(l, p::idx(&g, "grantedBy"))?;
        let t = cx.at(
            l,
            p::pct(
                "%-14s %-20s max %ss/job  %s parallel  %s queued  %s GB disk%s  by %s",
                &[node, ws, mj, mp, mq, md, s(ci), by],
            ),
        )?;
        cx.print(&t);
    }
    Ok(())
}

fn usage(cx: &mut Cx, v: &Value) -> Result<(), Raise> {
    let rows = cx.at(
        ln!("rows = v.get(\"usage\", [])"),
        p::get_or(v, "usage", Value::Array(Vec::new())),
    )?;
    if !p::truthy(&rows) {
        cx.print("(no traffic in this window)");
    }
    let l = ln!("    for u in rows:");
    for u in cx.at(l, p::iter(&rows))? {
        let l = ln!("print(\"%s  %-14s %-9s in %8.1f MB  out %8.1f MB\"");
        let day = cx.at(l, p::idx(&u, "day"))?;
        let mut node = cx.at(l, p::get(&u, "node"))?;
        if !p::truthy(&node) {
            let id = cx.at(l, p::idx(&u, "nodeId"))?;
            node = cx.at(l, p::slice(&id, None, Some(8)))?;
        }
        let route = cx.at(l, p::idx(&u, "route"))?;
        let bi = cx.at(l, p::idx(&u, "bytesIn"))?;
        let bi = cx.at(l, p::div(&bi, &Value::Float(1e6)))?;
        let bo = cx.at(l, p::idx(&u, "bytesOut"))?;
        let bo = cx.at(l, p::div(&bo, &Value::Float(1e6)))?;
        let t = cx.at(
            l,
            p::pct(
                "%s  %-14s %-9s in %8.1f MB  out %8.1f MB",
                &[day, node, route, bi, bo],
            ),
        )?;
        cx.print(&t);
    }
    let l = ln!("print(v.get(\"metered\", \"\"))");
    let m = cx.at(l, p::get_or(v, "metered", s("")))?;
    cx.print(&p::str_of(&m));
    Ok(())
}

fn events(cx: &mut Cx, v: &Value) -> Result<(), Raise> {
    let l = ln!("for e in v.get(\"events\", []):");
    let es = cx.at(l, p::get_or(v, "events", Value::Array(Vec::new())))?;
    for e in cx.at(l, p::iter(&es))? {
        let l = ln!("print(\"%s  %-16s %-18s %s %s\"");
        let at = cx.at(l, p::idx(&e, "at"))?;
        let now = cx.now();
        let when = cx.at(l, p::strftime("%m-%d %H:%M:%S", &at, now))?;
        let kind = cx.at(l, p::idx(&e, "kind"))?;
        let actor = cx.at(l, p::idx(&e, "actor"))?;
        let actor = cx.at(l, p::slice(&actor, None, Some(18)))?;
        let mut id = cx.at(l, p::get(&e, "jobId"))?;
        if !p::truthy(&id) {
            id = cx.at(l, p::get(&e, "nodeId"))?;
        }
        let id = p::or(id, s(""));
        let id = cx.at(l, p::slice(&id, None, Some(8)))?;
        let detail = cx.at(l, p::get(&e, "detail"))?;
        let detail = cx.at(l, p::dumps(&detail, false, None))?;
        let detail = cx.at(l, p::slice(&s(&detail), None, Some(80)))?;
        let t = cx.at(
            l,
            p::pct(
                "%s  %-16s %-18s %s %s",
                &[s(&when), kind, actor, id, detail],
            ),
        )?;
        cx.print(&t);
    }
    Ok(())
}

fn queue(cx: &mut Cx, v: &Value) -> Result<(), Raise> {
    let l = ln!("rows = v.get(\"entries\") or []");
    let rows = p::or(cx.at(l, p::get(v, "entries"))?, Value::Array(Vec::new()));
    let l = ln!("print(\"%s%s\" % (v.get(\"node\")");
    let node = cx.at(l, p::get(v, "node"))?;
    let from_node = p::eq_s(&cx.at(l, p::get(v, "from"))?, "node");
    let tail = if from_node {
        ""
    } else {
        "  (jobs from this server only: the node keeps no shared queue)"
    };
    let t = cx.at(l, p::pct("%s%s", &[node, s(tail)]))?;
    cx.print(&t);
    if !p::truthy(&rows) {
        cx.print("(nothing running or queued)");
    } else {
        let head = p::pct(
            "%-3s %-34s %-4s %-12s %-8s %-5s %-10s %s",
            &[
                s("#"),
                s("who"),
                s("tier"),
                s("class"),
                s("state"),
                s("cpus"),
                s("eta"),
                s("why"),
            ],
        )
        .expect("the header's strings");
        cx.print(&head);
    }
    let l = ln!("    for e in rows:\n        if e.get(\"mine\")");
    for e in cx.at(l, p::iter(&rows))? {
        let g = |cx: &Cx, line: u32, k: &str| cx.at(line, p::get(&e, k));
        let l = ln!("if e.get(\"mine\") and e.get(\"short\"):");
        let mine = g(cx, l, "mine")?;
        let who: Value = if p::truthy(&mine) && p::truthy(&g(cx, l, "short")?) {
            let mut pin = String::new();
            let l = ln!("if e.get(\"state\") == \"queued\" and \"pinned\" in e:");
            if p::eq_s(&g(cx, l, "state")?, "queued") && cx.at(l, p::has(&e, "pinned"))? {
                let l = ln!("pin = \"[pin] \" if e[\"pinned\"] else \"[auto] \"");
                pin = if p::truthy(&cx.at(l, p::idx(&e, "pinned"))?) {
                    "[pin] ".into()
                } else {
                    "[auto] ".into()
                };
            }
            let l = ln!("who = \"you: %s%s %s\"");
            let short = cx.at(l, p::idx(&e, "short"))?;
            let argv = p::or(g(cx, l, "argv")?, s(""));
            let mut w = cx.at(l, p::pct("you: %s%s %s", &[s(&pin), short, argv]))?;
            let l = ln!("if e.get(\"workspace\"):\n                who +=");
            if p::truthy(&g(cx, l, "workspace")?) {
                let l = ln!("who += \" (ws %s)\" % e[\"workspace\"]");
                let ws = cx.at(l, p::idx(&e, "workspace"))?;
                w += &cx.at(l, p::pct(" (ws %s)", &[ws]))?;
            }
            s(&w)
        } else if p::truthy(&mine) {
            s("this server (other workspace)")
        } else {
            let l = ln!("who = e.get(\"server\") or \"another server\"");
            p::or(g(cx, l, "server")?, s("another server"))
        };
        let l = ln!("if e.get(\"tierWhy\"):");
        let who_why = if p::truthy(&g(cx, l, "tierWhy")?) {
            let l = ln!("who_why = \"P: %s%s\"");
            let tw = cx.at(l, p::idx(&e, "tierWhy"))?;
            let sub = if p::truthy(&g(cx, l, "submitter")?) {
                let x = cx.at(l, p::idx(&e, "submitter"))?;
                cx.at(l, p::pct(" (%s)", &[x]))?
            } else {
                String::new()
            };
            cx.at(l, p::pct("P: %s%s", &[tw, s(&sub)]))?
        } else {
            String::new()
        };
        let eta = g(cx, ln!("eta = e.get(\"etaSecs\")"), "etaSecs")?;
        let etas = if p::is_none(&eta) {
            "-".to_string()
        } else {
            let running = p::eq_s(
                &g(cx, ln!("elif e.get(\"state\") == \"running\":"), "state")?,
                "running",
            );
            let l = if running {
                ln!("etas = \"~%dm left\" % max(1, (eta + 59) // 60)")
            } else {
                ln!("etas = \"~%dm\" % max(1, (eta + 59) // 60)")
            };
            let x = cx.at(l, p::add(&eta, &p::int(59)))?;
            let x = cx.at(l, p::floordiv(&x, &p::int(60)))?;
            let m = cx.at(l, p::max2(&p::int(1), &x))?;
            cx.at(l, p::pct(if running { "~%dm left" } else { "~%dm" }, &[m]))?
        };
        let l = ln!("why = e.get(\"why\") or e.get(\"blockedBy\") or \"\"");
        let mut why = g(cx, l, "why")?;
        if !p::truthy(&why) {
            why = g(cx, l, "blockedBy")?;
        }
        let why = p::or(why, s(""));
        let l =
            ln!("cpus = (\"%g\" % (e[\"cpuMillis\"] / 1000)) if e.get(\"cpuMillis\") else \"-\"");
        let cpus = if p::truthy(&g(cx, l, "cpuMillis")?) {
            let m = cx.at(l, p::idx(&e, "cpuMillis"))?;
            let x = cx.at(l, p::div(&m, &p::int(1000)))?;
            cx.at(l, p::pct("%g", &[x]))?
        } else {
            "-".to_string()
        };
        let l = ln!("print(\"%-3s %-34s %-4s %-12s %-8s %-5s %-10s %s\" % (e.get(\"pos\")");
        let pos = g(cx, l, "pos")?;
        let who = cx.at(l, p::slice(&who, None, Some(34)))?;
        let tier = g(cx, l, "tier")?;
        let tag = tier_tag(cx, l, &tier)?;
        let tag = if tag.is_empty() { "-".to_string() } else { tag };
        let class = p::or(g(cx, l, "class")?, s("-"));
        let state = g(cx, l, "state")?;
        let t = cx.at(
            l,
            p::pct(
                "%-3s %-34s %-4s %-12s %-8s %-5s %-10s %s",
                &[pos, who, s(&tag), class, state, s(&cpus), s(&etas), why],
            ),
        )?;
        cx.print(&t);
        if !who_why.is_empty() {
            cx.print(&format!("    {who_why}"));
        }
    }
    let l = ln!("if tier_counts(v.get(\"tiers\")):");
    let tiers = cx.at(l, p::get(v, "tiers"))?;
    if !tier_counts(cx, l, &tiers, false)?.is_empty() {
        let l = ln!("print(\"queued: \" + tier_counts(v.get(\"tiers\"), bare=True))");
        let tiers = cx.at(l, p::get(v, "tiers"))?;
        let t = tier_counts(cx, l, &tiers, true)?;
        cx.print(&format!("queued: {t}"));
    }
    let l = ln!("for t in v.get(\"totals\") or []:");
    let totals = cx.at(l, p::get(v, "totals"))?;
    for t in cx.at(l, p::iter_or_empty(totals))? {
        let l = ln!("print(\"server %-20s running %s  queued %s  today %.1f CPU-h\"");
        let label = cx.at(l, p::idx(&t, "label"))?;
        let running = cx.at(l, p::idx(&t, "running"))?;
        let queued = cx.at(l, p::idx(&t, "queued"))?;
        let cpu = p::or(cx.at(l, p::get(&t, "cpuSecsToday"))?, p::int(0));
        let cpu = cx.at(l, p::div(&cpu, &p::int(3600)))?;
        let line = cx.at(
            l,
            p::pct(
                "server %-20s running %s  queued %s  today %.1f CPU-h",
                &[label, running, queued, cpu],
            ),
        )?;
        cx.print(&line);
    }
    Ok(())
}

fn servers(cx: &mut Cx, v: &Value) -> Result<(), Raise> {
    let l = ln!("if v.get(\"role\") == \"tenant\":");
    if p::eq_s(&cx.at(l, p::get(v, "role"))?, "tenant") {
        let l = ln!("print(\"%s is shared with this server by %s\"");
        let node = cx.at(l, p::idx(v, "node"))?;
        let owner = p::or(cx.at(l, p::get(v, "owner"))?, s("its owner"));
        let t = cx.at(
            l,
            p::pct("%s is shared with this server by %s", &[node, owner]),
        )?;
        cx.print(&t);
        let l = ln!("print(\"  your share: %s\" % v[\"line\"])");
        if p::truthy(&cx.at(l, p::get(v, "line"))?) {
            let x = cx.at(l, p::idx(v, "line"))?;
            let t = cx.at(l, p::pct("  your share: %s", &[x]))?;
            cx.print(&t);
        }
        let l = ln!("s = v.get(\"share\") or {}\n        if s.get(\"expires_in_secs\"):");
        let sh = p::or(cx.at(l, p::get(v, "share"))?, Value::Object(Vec::new()));
        let l = ln!("print(\"  ends in %dh\" % (s[\"expires_in_secs\"] // 3600))");
        if p::truthy(&cx.at(l, p::get(&sh, "expires_in_secs"))?) {
            let x = cx.at(l, p::idx(&sh, "expires_in_secs"))?;
            let x = cx.at(l, p::floordiv(&x, &p::int(3600)))?;
            let t = cx.at(l, p::pct("  ends in %dh", &[x]))?;
            cx.print(&t);
        }
        return Ok(());
    }
    let head = p::pct(
        "%-4s %-24s %-9s %-5s %-6s %s",
        &[
            s("slot"),
            s("server"),
            s("state"),
            s("trust"),
            s("weight"),
            s("share"),
        ],
    )
    .expect("the header's strings");
    cx.print(&head);
    let l = ln!("for s in v.get(\"servers\") or []:");
    let list = cx.at(l, p::get(v, "servers"))?;
    for sv in cx.at(l, p::iter_or_empty(list))? {
        let g = |cx: &Cx, line: u32, k: &str| cx.at(line, p::get(&sv, k));
        let l = ln!("l = s.get(\"limits\") or {}");
        let lim = p::or(g(cx, l, "limits")?, Value::Object(Vec::new()));
        let share = if p::truthy(&lim) {
            let l = ln!("share = (\"run %s · queue %s · %s CPU-h/day");
            let lg = |cx: &Cx, k: &str| cx.at(l, p::get(&lim, k));
            let mr = lg(cx, "max_running")?;
            let mq = lg(cx, "max_queued")?;
            let ch = lg(cx, "cpu_hours_day")?;
            let mj = lg(cx, "max_job_secs")?;
            let d = dur(cx, l, &mj)?;
            let mc = lg(cx, "max_cpus")?;
            let mm = lg(cx, "max_mem_gb")?;
            let net = lg(cx, "network")?;
            cx.at(
                l,
                p::pct(
                    "run %s · queue %s · %s CPU-h/day · %s/job · %s CPUs · %s GB · net %s",
                    &[mr, mq, ch, s(&d), mc, mm, net],
                ),
            )?
        } else if p::truthy(&g(
            cx,
            ln!("if l else (\"(this server; no other servers yet)\""),
            "solo",
        )?) {
            "(this server; no other servers yet)".to_string()
        } else {
            "-".to_string()
        };
        let mut st = g(cx, ln!("st = s.get(\"state\")"), "state")?;
        let l = ln!("if s.get(\"sas\"):\n                st +=");
        if p::truthy(&g(cx, l, "sas")?) {
            let l = ln!("st += \" · SAS \" + s[\"sas\"]");
            let sas = cx.at(l, p::idx(&sv, "sas"))?;
            let x = cx.at(l, p::add(&s(" · SAS "), &sas))?;
            st = cx.at(l, p::iadd(&st, &x))?;
        }
        let u = p::or(
            g(cx, ln!("u = s.get(\"usage\") or {}"), "usage")?,
            Value::Object(Vec::new()),
        );
        let l = ln!("if u.get(\"refusal\"):");
        if p::truthy(&cx.at(l, p::get(&u, "refusal"))?) {
            let l = ln!("st = \"refused: %s\" % u[\"refusal\"]");
            let r = cx.at(l, p::idx(&u, "refusal"))?;
            st = s(&cx.at(l, p::pct("refused: %s", &[r]))?);
        }
        let l = ln!("print(\"%-4s %-24s %-9s %-5s %-6s %s\" % (s.get(\"slot\")");
        let slot = g(cx, l, "slot")?;
        let label = p::or(g(cx, l, "label")?, s("?"));
        let label = cx.at(l, p::slice(&label, None, Some(24)))?;
        let trust = g(cx, l, "trust")?;
        let weight = cx.at(l, p::get_or(&lim, "weight", s("-")))?;
        let t = cx.at(
            l,
            p::pct(
                "%-4s %-24s %-9s %-5s %-6s %s",
                &[slot, label, st, trust, weight, s(&share)],
            ),
        )?;
        cx.print(&t);
        let mut extra: Vec<String> = Vec::new();
        let l = ln!("if u.get(\"running\") is not None:");
        if !p::is_none(&cx.at(l, p::get(&u, "running"))?) {
            let l = ln!("extra.append(\"running %s, queued %s\"");
            let r = cx.at(l, p::get(&u, "running"))?;
            let q = cx.at(l, p::get(&u, "queued"))?;
            extra.push(cx.at(l, p::pct("running %s, queued %s", &[r, q]))?);
        }
        let l = ln!("if u.get(\"cpu_secs_today\") is not None:");
        if !p::is_none(&cx.at(l, p::get(&u, "cpu_secs_today"))?) {
            let l = ln!("extra.append(\"today %.1f CPU-h\"");
            let c = cx.at(l, p::idx(&u, "cpu_secs_today"))?;
            let c = cx.at(l, p::div(&c, &p::int(3600)))?;
            extra.push(cx.at(l, p::pct("today %.1f CPU-h", &[c]))?);
        }
        let l = ln!("if s.get(\"expiresAt\"):\n                extra.append");
        if p::truthy(&g(cx, l, "expiresAt")?) {
            let l = ln!("extra.append(\"until \" + time.strftime");
            let t = cx.at(l, p::idx(&sv, "expiresAt"))?;
            let now = cx.now();
            extra.push(format!(
                "until {}",
                cx.at(l, p::strftime("%b %d %H:%M", &t, now))?
            ));
        }
        let l = ln!("if s.get(\"priority\") is True:");
        let pr = g(cx, l, "priority")?;
        if matches!(pr, Value::Bool(true)) {
            extra.push("Priority counts".into());
        } else if matches!(pr, Value::Bool(false))
            && !p::eq(
                &g(
                    cx,
                    ln!("elif s.get(\"priority\") is False and s.get(\"slot\") != 0:"),
                    "slot",
                )?,
                &p::int(0),
            )
        {
            extra.push("Priority runs as Normal".into());
        }
        if !extra.is_empty() {
            cx.print(&format!("{:4} {}", "", extra.join(" · ")));
        }
    }
    let l = ln!("for r in v.get(\"requests\") or []:");
    let reqs = cx.at(l, p::get(v, "requests"))?;
    for r in cx.at(l, p::iter_or_empty(reqs))? {
        let l = ln!("print(\"waiting: %s  %s  %s\"");
        let id = cx.at(l, p::idx(&r, "id"))?;
        let st = cx.at(l, p::idx(&r, "state"))?;
        let sm = cx.at(l, p::idx(&r, "summary"))?;
        let t = cx.at(l, p::pct("waiting: %s  %s  %s", &[id, st, sm]))?;
        cx.print(&t);
    }
    let l = ln!("if v.get(\"note\"):");
    if p::truthy(&cx.at(l, p::get(v, "note"))?) {
        let l = ln!("print(\"note: %s\" % v[\"note\"])");
        let n = cx.at(l, p::idx(v, "note"))?;
        let t = cx.at(l, p::pct("note: %s", &[n]))?;
        cx.print(&t);
    }
    Ok(())
}

mod get;
use get::get;
