//! `mpc`: a small reference host for the error surface, used by the blind-agent test
//! (`docs/evidence/error-surface-blind.md`). Not a product: it shows how a host prints the
//! kit's refusals (plain text with the id appended, or `--json`), and gives an agent with no
//! context a CLI to recover with, using only `--help`, `explain` and `capabilities`.
//!
//! One daemon holds the sessions (undo history and claims live in memory, as in a real
//! host); each `mpc` command is a client that talks to it over a Unix socket in `.mpc/`.
//!
//! ```text
//! mpc daemon <dir> <scenario>        # set a scenario up in <dir> and serve it
//! mpc [--json] <path> <verb> [args]  # a verb on a doc (from inside <dir>)
//! mpc [--json] explain [ID] | capabilities
//! ```

#[cfg(unix)]
fn main() {
    std::process::exit(host::main());
}

#[cfg(not(unix))]
fn main() {
    eprintln!("mpc: Unix only");
    std::process::exit(2);
}

#[cfg(unix)]
mod host {
    use std::collections::HashMap;
    use std::io::{IsTerminal, Read, Write};
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex};
    use std::time::{SystemTime, UNIX_EPOCH};

    use k2_mpc_base::errors::{run_meta, ErrorReport, Refusal};
    use k2_mpc_base::parity::RUN_TOOL_VERBS;
    use k2_mpc_base::parity::{Registry, Tier};
    use k2_mpc_base::session::{
        ApprovalState, AuditEvent, ClaimPolicy, Config, ConnId, Host, LineRange, LogItem, LogQuery,
        Participant, ParticipantKind, Rights, Session, SessionError, SessionManager, Stay,
        TextFilePack, UndoMode, UndoRequest,
    };
    use k2_mpc_base::store::{DocKey, DocStore};
    use k2_mpc_base::tools::{exit, ToolError, ToolOptions};

    const ROOT: &str = TextFilePack::ROOT;
    /// Kit verbs this reference host doesn't route.
    const UNROUTED: &[&str] = &[
        "open",
        "leave",
        "follow",
        "cursor",
        "watch",
        "tail -f",
        "restore",
        "version --name",
        "mv",
        "claim --renew",
    ];
    const ME: &str = "agent";

    const HELP: &str = "\
mpc: live, shared docs for people and agents (a reference host)

usage:
  mpc [--json] <path> <verb> [args...]   run a verb on a doc (paths are relative to the
                                         workspace); text for insert/append/replace/patch/
                                         write comes on stdin
  mpc [--json] explain [ID]              what an error id means and what to do next
                                         (no ID: list every id)
  mpc [--json] capabilities              every verb, its flags, exit codes, the error ids
                                         it can return, and the limits in effect
  mpc --help                             this text

Others (people and agents) may be editing the same docs at the same time. Every refusal
ends with its error id, e.g. `(MPC-E110)`; with --json it is one JSON object on stdout:
{\"id\", \"exit\", \"message\", \"next\", \"rule\", \"details\"}.

Common verbs: cat, grep, view --lines a,b, insert, append, replace, delete, patch,
write --base V, undo, redo, log, approvals, who, status, claim. See capabilities.
";

    pub fn main() -> i32 {
        let args: Vec<String> = std::env::args().skip(1).collect();
        match args.first().map(String::as_str) {
            None | Some("--help" | "-h" | "help") => {
                print!("{HELP}");
                0
            }
            Some("daemon") if args.len() == 3 => daemon(Path::new(&args[1]), &args[2]),
            _ => client(args),
        }
    }

    // ------------------------------------------------------------------ client

    fn find_mpc_dir() -> Option<PathBuf> {
        let mut d = std::env::current_dir().ok()?;
        loop {
            if d.join(".mpc/sock").exists() {
                return Some(d.join(".mpc"));
            }
            if !d.pop() {
                return None;
            }
        }
    }

    fn put(out: &mut Vec<u8>, b: &[u8]) {
        out.extend_from_slice(&(b.len() as u32).to_le_bytes());
        out.extend_from_slice(b);
    }

    fn take<'a>(b: &mut &'a [u8]) -> Option<&'a [u8]> {
        if b.len() < 4 {
            return None;
        }
        let n = u32::from_le_bytes(b[..4].try_into().ok()?) as usize;
        let v = b.get(4..4 + n)?;
        *b = &b[4 + n..];
        Some(v)
    }

    /// Whether the verb reads text from stdin.
    fn wants_stdin(args: &[String]) -> bool {
        let verb = args
            .iter()
            .skip_while(|a| a.starts_with("--"))
            .nth(1)
            .map(String::as_str);
        match verb {
            Some("insert" | "append" | "patch" | "apply" | "write" | "diff") => true,
            Some("replace") => {
                !(args.iter().any(|a| a == "--old") && args.iter().any(|a| a == "--new"))
                    || args.iter().any(|a| a == "-")
            }
            _ => false,
        }
    }

    fn client(args: Vec<String>) -> i32 {
        let Some(dir) = find_mpc_dir() else {
            eprintln!("mpc: no workspace here (run mpc from inside the workspace folder)");
            return 2;
        };
        let mut stdin = Vec::new();
        if wants_stdin(&args) && !std::io::stdin().is_terminal() {
            let _ = std::io::stdin().read_to_end(&mut stdin);
        }
        let cwd = std::env::current_dir().unwrap_or_default();
        let mut req = Vec::new();
        put(&mut req, cwd.to_string_lossy().as_bytes());
        put(&mut req, &stdin);
        for a in &args {
            put(&mut req, a.as_bytes());
        }
        // a relative socket path: Unix socket paths are short
        if std::env::set_current_dir(&dir).is_err() {
            eprintln!("mpc: can't reach the workspace daemon");
            return 2;
        }
        let Ok(mut s) = UnixStream::connect("sock") else {
            eprintln!("mpc: the workspace daemon isn't running");
            return 2;
        };
        let mut framed = Vec::new();
        put(&mut framed, &req);
        if s.write_all(&framed).is_err() {
            return 2;
        }
        let _ = s.shutdown(std::net::Shutdown::Write);
        let mut resp = Vec::new();
        let _ = s.read_to_end(&mut resp);
        if resp.is_empty() {
            eprintln!("mpc: no answer from the workspace daemon");
            return 2;
        }
        let code = resp[0];
        let mut b = &resp[1..];
        let out = take(&mut b).unwrap_or_default();
        let err = take(&mut b).unwrap_or_default();
        let _ = std::io::stdout().write_all(out);
        let _ = std::io::stderr().write_all(err);
        i32::from(code)
    }

    // ------------------------------------------------------------------ daemon

    struct ClockHost {
        rights: Mutex<HashMap<String, Rights>>,
    }

    impl Host for ClockHost {
        fn rights(&self, who: &Participant, _key: &DocKey) -> Rights {
            self.rights
                .lock()
                .unwrap()
                .get(&who.pid)
                .copied()
                .unwrap_or(Rights::none())
        }
        fn now_ms(&self) -> u64 {
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |d| d.as_millis() as u64)
        }
        fn on_event(&self, _ev: &AuditEvent) {}
    }

    struct World {
        ws: PathBuf,
        mgr: SessionManager,
        config: Config,
        /// (doc path, pid) → connection
        conns: HashMap<(String, String), ConnId>,
        me: Participant,
        policy: ClaimPolicy,
        log: std::fs::File,
    }

    fn pack_for(path: &str) -> TextFilePack {
        if path.ends_with(".csv") {
            TextFilePack::new("csv", 4 << 20)
        } else if path.ends_with(".md") {
            TextFilePack::new("markdown", 4 << 20)
        } else {
            TextFilePack::new("text", 4 << 20)
        }
    }

    fn who(pid: &str, kind: ParticipantKind) -> Participant {
        let name = format!("{}{}", pid[..1].to_uppercase(), &pid[1..]);
        Participant::new(pid, name, "#3366cc", kind, "cli")
    }

    impl World {
        fn doc(&mut self, path: &str) -> Result<Arc<Session>, SessionError> {
            let rel = path.trim_start_matches("./");
            let key = DocKey {
                space: "ws".into(),
                path: rel.into(),
            };
            if let Some(s) = self.mgr.get(&key) {
                return Ok(s);
            }
            let file = self.ws.join(rel);
            self.mgr
                .open_file(Arc::new(pack_for(rel)), &key, &file, &self.ws)
        }

        fn conn_as(
            &mut self,
            path: &str,
            p: &Participant,
        ) -> Result<(Arc<Session>, ConnId), SessionError> {
            let s = self.doc(path)?;
            let k = (path.to_owned(), p.pid.clone());
            if let Some(c) = self.conns.get(&k) {
                if s.touch(*c).is_ok() {
                    return Ok((s, *c));
                }
            }
            let c = s.join(p.clone(), Stay::Held)?.conn;
            self.conns.insert(k, c);
            Ok((s, c))
        }

        fn conn(&mut self, path: &str) -> Result<(Arc<Session>, ConnId), SessionError> {
            let me = self.me.clone();
            self.conn_as(path, &me)
        }
    }

    fn daemon(dir: &Path, scenario: &str) -> i32 {
        let ws = std::fs::canonicalize(dir).expect("the workspace folder exists");
        let mpc = ws.join(".mpc");
        let _ = std::fs::remove_dir_all(&mpc);
        std::fs::create_dir_all(&mpc).unwrap();
        let host = Arc::new(ClockHost {
            rights: Mutex::new(HashMap::new()),
        });
        let mut config = Config::default();
        match scenario {
            "held" => config.pending_max_records = 2,
            "rate" => {
                config.edit_rate_per_s = 1;
                config.edit_burst = 3;
            }
            _ => {}
        }
        let store = Arc::new(DocStore::open(mpc.join("docs.redb")).unwrap());
        let mgr = SessionManager::new(store, host.clone(), config.clone());
        let me = if scenario == "held" {
            who(ME, ParticipantKind::Agent).on_behalf_of("sam")
        } else {
            who(ME, ParticipantKind::Agent)
        };
        {
            let mut r = host.rights.lock().unwrap();
            r.insert(ME.into(), Rights::editor().with_history(true));
            r.insert("sam".into(), Rights::editor().with_history(true));
            r.insert("bot".into(), Rights::editor());
        }
        let log = std::fs::File::create(mpc.join("requests.log")).unwrap();
        let mut w = World {
            ws: ws.clone(),
            mgr,
            config,
            conns: HashMap::new(),
            me,
            policy: if scenario == "claimed" {
                ClaimPolicy::Respect
            } else {
                ClaimPolicy::Ignore
            },
            log,
        };
        if let Err(e) = setup(&mut w, scenario) {
            eprintln!("mpc daemon: scenario {scenario}: {}", e.report().plain());
            return 1;
        }
        std::env::set_current_dir(&mpc).unwrap();
        let listener = UnixListener::bind("sock").expect("bind .mpc/sock");
        eprintln!("mpc daemon: serving {} ({scenario})", ws.display());
        for stream in listener.incoming() {
            let Ok(mut s) = stream else { continue };
            let mut req = Vec::new();
            if s.read_to_end(&mut req).is_err() {
                continue;
            }
            let mut b = &req[..];
            let Some(mut body) = take(&mut b) else {
                continue;
            };
            let _cwd = take(&mut body).unwrap_or_default();
            let stdin = take(&mut body).unwrap_or_default().to_vec();
            let mut args = Vec::new();
            while let Some(a) = take(&mut body) {
                args.push(String::from_utf8_lossy(a).into_owned());
            }
            let (code, out, err, id) = serve(&mut w, &args, &stdin);
            let _ = writeln!(
                w.log,
                "{} exit={code}{} :: {}",
                ClockHost::now(),
                id.map(|i| format!(" id={i}")).unwrap_or_default(),
                args.join(" ")
            );
            let mut resp = vec![code];
            put(&mut resp, &out);
            put(&mut resp, err.as_bytes());
            let _ = s.write_all(&resp);
        }
        0
    }

    impl ClockHost {
        fn now() -> u64 {
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |d| d.as_millis() as u64)
        }
    }

    /// What the other participants did before the agent arrives.
    fn setup(w: &mut World, scenario: &str) -> Result<(), SessionError> {
        let sam = who("sam", ParticipantKind::Person);
        let write =
            |w: &World, name: &str, text: &[u8]| std::fs::write(w.ws.join(name), text).unwrap();
        match scenario {
            "undo-conflict" => {
                write(
                    w,
                    "notes.md",
                    b"# Launch notes\n\nStatus: draft\nOwner: tbd\nDate: 2026-11-02\n\nOpen questions:\n- pricing\n",
                );
                let (s, me) = w.conn("notes.md")?;
                s.edit_with(me, ROOT, |t| {
                    Ok::<_, ()>(
                        t.replace("Status: draft", "Status: in review")
                            .replace("Owner: tbd", "Owner: agent"),
                    )
                })
                .map_err(|e| match e {
                    k2_mpc_base::session::EditWithError::Session(e) => e,
                    _ => SessionError::BadEdit("setup".into()),
                })?;
                let (s, sam) = w.conn_as("notes.md", &sam)?;
                s.edit_with(sam, ROOT, |t| {
                    Ok::<_, ()>(t.replace(
                        "Status: in review",
                        "Status: approved by legal (Sam, 10-27)",
                    ))
                })
                .map_err(|_| SessionError::BadEdit("setup".into()))?;
            }
            "held" => {
                write(w, "plan.md", b"# Q4 plan\n\nGoals:\n- ship v1\n");
                let (s, me) = w.conn("plan.md")?;
                for line in ["- hire a designer\n", "- close the audit\n"] {
                    s.edit_with(me, ROOT, |t| Ok::<_, ()>(format!("{t}{line}")))
                        .map_err(|_| SessionError::BadEdit("setup".into()))?;
                }
            }
            "write-conflict" => {
                // the agent read version 1; Sam then rewrote the Status line (v0.9.7.1's
                // blind check of the write --base conflict)
                write(
                    w,
                    "notes.md",
                    b"# Launch notes\n\nStatus: draft\nOwner: tbd\nDate: 2026-11-02\n",
                );
                let (s, sam) = w.conn_as("notes.md", &sam)?;
                s.edit_with(sam, ROOT, |t| {
                    Ok::<_, ()>(t.replace("Status: draft", "Status: in review (Sam)"))
                })
                .map_err(|_| SessionError::BadEdit("setup".into()))?;
            }
            "rate" => write(w, "todo.md", b"# TODO\n"),
            "declined" => write(w, "data.csv", b"name,age\r\nalice,31\r\nbob,27\n"),
            "claimed" => {
                write(
                    w,
                    "spec.md",
                    b"# Spec\n\n## Sync\nClients recieve updates in order.\nEach update has a version.\nVersions only grow.\n\n## Storage\nThe log is append-only.\n",
                );
                let bot = who("bot", ParticipantKind::Agent);
                let (s, b) = w.conn_as("spec.md", &bot)?;
                s.claim(
                    b,
                    LineRange {
                        root: ROOT.into(),
                        first: 3,
                        last: 6,
                    },
                    Some(90_000),
                    Some("rewriting the Sync section".into()),
                    false,
                )?;
            }
            other => {
                return Err(SessionError::BadEdit(format!("no scenario {other}")));
            }
        }
        let _ = &w.config;
        Ok(())
    }

    type Answer = (u8, Vec<u8>, String, Option<&'static str>);

    fn refuse(r: ErrorReport, json: bool) -> Answer {
        let id = r.id();
        if json {
            (
                r.exit(),
                format!("{}\n", r.to_json()).into_bytes(),
                String::new(),
                Some(id),
            )
        } else {
            (
                r.exit(),
                Vec::new(),
                format!("mpc: {}\n", r.plain()),
                Some(id),
            )
        }
    }

    fn ok(out: impl Into<Vec<u8>>, err: String) -> Answer {
        (exit::OK, out.into(), err, None)
    }

    fn serve(w: &mut World, args: &[String], stdin: &[u8]) -> Answer {
        let mut json = false;
        let mut rest: Vec<String> = Vec::new();
        let mut it = args.iter();
        for a in it.by_ref() {
            if a == "--json" {
                json = true;
            } else {
                rest.push(a.clone());
                break;
            }
        }
        rest.extend(it.cloned());
        // capabilities lists what this host routes: the kit's text verbs minus the ones mpc
        // doesn't serve (a host passes only the verbs it routes)
        let mut kit = Registry::kit();
        for v in UNROUTED {
            kit = kit.without(v);
        }
        let regs = [kit];
        let reg_refs: Vec<&Registry> = regs.iter().collect();
        let _ = Tier::Text;
        if let Some(out) = run_meta(&rest, json, &reg_refs, &w.config) {
            return match out.refusal {
                Some(r) => refuse(r, json),
                None => ok(out.stdout, String::new()),
            };
        }
        let [path, verb, flags @ ..] = rest.as_slice() else {
            return refuse(
                ToolError::usage("usage: mpc <path> <verb> [args] (mpc --help)").report(),
                json,
            );
        };
        match run(w, path, verb, flags, stdin, json) {
            Ok(a) => a,
            Err(e) => refuse(e.report(), json),
        }
    }

    fn flag<'a>(flags: &'a [String], name: &str) -> Option<&'a str> {
        flags
            .iter()
            .position(|f| f == name)
            .and_then(|i| flags.get(i + 1))
            .map(String::as_str)
    }

    fn num(v: Option<&str>, what: &str) -> Result<u64, ErrorReport> {
        let v = v.ok_or_else(|| ToolError::usage(format!("{what} needs a number")).report())?;
        v.trim_start_matches('v')
            .parse()
            .map_err(|_| ToolError::usage(format!("invalid {what}: '{v}'")).report())
    }

    fn run(
        w: &mut World,
        path: &str,
        verb: &str,
        flags: &[String],
        stdin: &[u8],
        json: bool,
    ) -> Result<Answer, SessionError> {
        let (s, me) = w.conn(path)?;
        let mut argv = vec![verb.to_owned()];
        argv.extend(flags.iter().cloned());
        let has = |f: &str| flags.iter().any(|x| x == f);
        let history_form = (verb == "diff" || verb == "show") && has("--version");
        if !history_form
            && RUN_TOOL_VERBS
                .iter()
                .any(|v| v.split(' ').next() == Some(verb))
        {
            let opts = ToolOptions::new().policy(w.policy);
            return Ok(match s.run_tool_with(me, ROOT, &argv, stdin, &opts) {
                Err(e) => refuse(e.report(), json),
                Ok(out) => {
                    let mut err = String::new();
                    if !out.notes.is_empty() {
                        err.push_str(&out.notes.to_string());
                        if !err.ends_with('\n') {
                            err.push('\n');
                        }
                    }
                    match out.refusal {
                        Some(r) if json => (
                            out.exit,
                            format!("{}\n", r.to_json()).into_bytes(),
                            err,
                            Some(r.id()),
                        ),
                        Some(r) => {
                            err.push_str(&format!("{}\n", out.stderr_summary));
                            (out.exit, out.stdout, err, Some(r.id()))
                        }
                        None => {
                            err.push_str(&format!("{}\n", out.stderr_summary));
                            (out.exit, out.stdout, err, None)
                        }
                    }
                }
            });
        }
        let num_or = |v: Option<&str>, what: &str| num(v, what);
        Ok(match verb {
            "write" => {
                let Some(b) = flag(flags, "--base") else {
                    return Ok(refuse(
                        ToolError::usage("write needs --base V").report(),
                        json,
                    ));
                };
                let base = match num_or(Some(b), "--base") {
                    Ok(n) => n,
                    Err(r) => return Ok(refuse(r, json)),
                };
                let text = String::from_utf8_lossy(stdin);
                let o = s.write_base(me, ROOT, base, &text)?;
                match o.refusal_at(base, s.status()?.version) {
                    Some(r) => refuse(r, json),
                    None => ok(
                        Vec::new(),
                        format!("wrote {path} · v{}\n", s.status()?.version),
                    ),
                }
            }
            "undo" | "redo" => {
                let mode = if has("--partial") {
                    UndoMode::Partial
                } else if has("--drop") {
                    UndoMode::Drop
                } else {
                    UndoMode::All
                };
                let steps = match flag(flags, "--steps").map(|v| num_or(Some(v), "--steps")) {
                    Some(Ok(n)) => n as usize,
                    Some(Err(r)) => return Ok(refuse(r, json)),
                    None => 1,
                };
                let mut req = UndoRequest::steps(steps).mode(mode);
                if let Some(p) = flag(flags, "--for") {
                    req = req.for_person(p);
                }
                let o = if verb == "undo" {
                    s.undo_with(me, &req)?
                } else {
                    s.redo_with(me, &req)?
                };
                let v = s.status()?.version;
                match o.refusal() {
                    Some(r) if r.exit() != exit::OK && !json => {
                        // a partial undo applied some: say so, then the refusal
                        (
                            r.exit(),
                            Vec::new(),
                            format!("mpc: {} · v{v}\n", r.plain()),
                            Some(r.id()),
                        )
                    }
                    Some(r) => refuse(r, json),
                    None => ok(Vec::new(), format!("{verb}: {o:?} · v{v}\n")),
                }
            }
            "revert" => {
                let Some(n) = flags.iter().find(|f| !f.starts_with("--")) else {
                    return Ok(refuse(ToolError::usage("revert <change>").report(), json));
                };
                let n = match num_or(Some(n), "change") {
                    Ok(n) => n,
                    Err(r) => return Ok(refuse(r, json)),
                };
                let o = s.revert(me, n, has("--partial"))?;
                match o.refusal() {
                    Some(r) => refuse(r, json),
                    None => ok(
                        Vec::new(),
                        format!("reverted change {n} · v{}\n", s.status()?.version),
                    ),
                }
            }
            "log" | "approvals" => {
                let mut q = LogQuery::default();
                if has("--pending") || verb == "approvals" {
                    q = q.pending();
                }
                let mut out = String::new();
                for item in s.log(me, &q)? {
                    match item {
                        LogItem::Change { info, approval } => out.push_str(&format!(
                            "change {} {:?} by {}{}{} (+{} -{})\n",
                            info.seq,
                            info.kind,
                            info.author,
                            info.on_behalf_of
                                .as_deref()
                                .map(|p| format!(" for {p}"))
                                .unwrap_or_default(),
                            approval.map(|a| format!(" [{a:?}]")).unwrap_or_default(),
                            info.lines_added,
                            info.lines_removed
                        )),
                        LogItem::Version(v) => {
                            out.push_str(&format!("version {} at change {}\n", v.id, v.seq))
                        }
                        _ => {}
                    }
                }
                ok(out, String::new())
            }
            "approve" | "decline" => {
                let Some(n) = flags.first() else {
                    return Ok(refuse(
                        ToolError::usage(format!("{verb} <change>")).report(),
                        json,
                    ));
                };
                let n = match num_or(Some(n), "change") {
                    Ok(n) => n,
                    Err(r) => return Ok(refuse(r, json)),
                };
                let state = if verb == "approve" {
                    ApprovalState::Approved
                } else {
                    ApprovalState::Declined
                };
                s.set_approval(me, n, state)?;
                ok(Vec::new(), format!("{verb}d change {n}\n"))
            }
            "diff" | "show" => {
                let v = match num_or(flag(flags, "--version"), "--version") {
                    Ok(n) => n,
                    Err(r) => return Ok(refuse(r, json)),
                };
                if verb == "show" {
                    let mut out = String::new();
                    for (_, text) in s.show(me, v)? {
                        out.push_str(&text);
                    }
                    ok(out, String::new())
                } else {
                    let to = match flag(flags, "--to").map(|t| num_or(Some(t), "--to")) {
                        Some(Ok(n)) => Some(n),
                        Some(Err(r)) => return Ok(refuse(r, json)),
                        None => None,
                    };
                    let d = s.diff_version(me, ROOT, v, to, 3)?;
                    (d.exit(), d.text.into_bytes(), String::new(), None)
                }
            }
            "blame" => {
                let b = s.blame(me, ROOT, None)?;
                ok(b.render(|_| None), String::new())
            }
            "activity" => {
                let mut out = String::new();
                for a in s.activity(me, 20)? {
                    out.push_str(&format!(
                        "{} {} {}{}\n",
                        a.pid,
                        a.verb,
                        a.lines
                            .map(|l| format!("lines {}-{}", l.first, l.last))
                            .unwrap_or_default(),
                        a.claim.map(|c| format!(" ({c})")).unwrap_or_default()
                    ));
                }
                ok(out, String::new())
            }
            "status" => {
                let st = s.status()?;
                ok(format!("{path} v{}\n", st.version), String::new())
            }
            "who" => {
                let mut out = String::new();
                for r in s.roster(me)? {
                    out.push_str(&format!("{} ({:?})\n", r.participant.pid, r.role));
                }
                for c in s.claims(me)? {
                    out.push_str(&format!(
                        "claim {} by {}: lines {}-{}\n",
                        c.id, c.pid, c.lines.first, c.lines.last
                    ));
                }
                ok(out, String::new())
            }
            "claim" => {
                if has("--release") {
                    let ended = s.release_claim(me, None)?;
                    ok(Vec::new(), format!("released {} claim(s)\n", ended.len()))
                } else {
                    let Some(l) = flag(flags, "--lines") else {
                        return Ok(refuse(ToolError::usage("claim --lines a,b").report(), json));
                    };
                    let (a, b) = l.split_once(',').unwrap_or((l, l));
                    let (Ok(a), Ok(b)) = (a.parse(), b.parse()) else {
                        return Ok(refuse(
                            ToolError::usage(format!("invalid --lines '{l}'")).report(),
                            json,
                        ));
                    };
                    let ttl = flag(flags, "--ttl").and_then(|t| t.parse().ok());
                    let c = s.claim(
                        me,
                        LineRange {
                            root: ROOT.into(),
                            first: a,
                            last: b,
                        },
                        ttl,
                        None,
                        false,
                    )?;
                    ok(Vec::new(), format!("{} on lines {a}-{b}\n", c.id))
                }
            }
            other => refuse(
                ToolError::unsupported(other, "see mpc capabilities for the verbs").report(),
                json,
            ),
        })
    }

    #[allow(dead_code)]
    fn _refusal_is_object_safe(r: &dyn Refusal) -> ErrorReport {
        r.report()
    }
}
