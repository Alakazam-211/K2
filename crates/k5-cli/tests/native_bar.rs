//! The native bar: which command lines the k5 binary still hands to the
//! shell CLI. It is the 0.46.17 ship bar (K2's ruling): every command
//! native.
//!
//! What it runs (each a probe):
//!
//! - every form of `tests/data/router_forms.tsv` (`scripts/router-forms.py`
//!   reads them from the router and each family's dispatch), with its
//!   `argv` when it has one;
//! - each form, an alias too, with `--help` after it (a help form);
//! - each form whose `--schema` entry takes `--json` with `--json` after it;
//! - the shapes of `tests/data/native_bar_shapes.tsv`: documented flags,
//!   and error and environment edges, each with its class;
//! - [`PROXIED`]: one form of each family the shell answers with its
//!   Python's urllib, with `K2_HOST=https://<name>` through the person's
//!   proxy (K2's ruling): see [`proxied_falls_through`].
//!
//! How: the built binary, with a scratch home and K2 home, a stub daemon
//! that answers every request with 200 and the owner token and port on
//! disk, stdin closed, and a `PATH` with no `python3` on it. Every
//! hand-off checks for `python3` before bash starts
//! ([`k5_cli::fallthrough::run`]), so a form that falls through exits 69
//! with its one line and runs nothing; anything else is native. A probe
//! still running after [`DEADLINE`] is native too (it is waiting in its
//! own code: a hand-off ends at once). Programs that would act on the
//! machine (`open`, `launchctl`, `systemctl`, `sudo` and the like) are
//! stand-ins first on `PATH`.
//!
//! The ceiling, `tests/data/native_bar_ceiling.tsv`, lists every probe
//! that falls through today, with its class:
//!
//! - V a verb (or its `--json` twin), H a help form, F a documented flag
//!   or shape, E an error or environment edge on a k5 code path: the bar
//!   counts these (K2's ruling);
//! - P an edge that exists only because python3 is missing (the
//!   native-without-python3 difference): listed, not counted.
//!
//! A [`PROXIED`] probe is class V, and it falls through when the binary
//! sends nothing at all and fails (it doesn't tunnel through the proxy as
//! the shell's Python does: it fails closed); it is native when its
//! request reached the daemon inside a CONNECT tunnel. Either way the
//! stub proxy and the stub daemons record every connection, and the test
//! fails if anything went to the daemon direct, in plain text, or to the
//! local plain-http daemon, or if the owner token reached the proxy.
//!
//! The test fails when a probe falls through that the ceiling doesn't
//! list, and when a ceiling row is native now or names no probe: the
//! ceiling can only fall. The ship bar is a ceiling with no counted row.
//! `NATIVE_BAR_WRITE=<file>` writes the ceiling as this run found it to
//! that file (the check still runs). `python3 scripts/router-forms.py
//! --summary` prints the count per family.

#![cfg(unix)]

mod common;

use common::stub::Stub;
use common::tunnel::{self, ProxyMode, ProxyStub, TlsStub};
use std::collections::BTreeSet;
use std::fs;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

const BIN: &str = env!("CARGO_BIN_EXE_k5-cli");
const FORMS: &str = "crates/k5-cli/tests/data/router_forms.tsv";
const SHAPES: &str = "crates/k5-cli/tests/data/native_bar_shapes.tsv";
const CEILING: &str = "crates/k5-cli/tests/data/native_bar_ceiling.tsv";
/// The hand-off's line when there is no `python3` ([`k5_cli::fallthrough::FrontError::NoPython`]).
const NO_PYTHON: &str = "is not built into k5 yet and needs Python 3";
/// How long a probe may run: past it, it is in its own code.
const DEADLINE: Duration = Duration::from_secs(10);
const WORKERS: usize = 8;
/// The owner token on the scratch machine's disk.
const OWNER: &str = "owner-tok-7171";
/// One form per family whose shell code is a Python urllib block, probed
/// with `K2_HOST=https://0.0.0.0` through `https_proxy` ([`proxied`]).
/// `0.0.0.0` is no loopback name, so it is proxied, yet a direct connect
/// reaches the stub daemon, so a direct try shows in its log.
const PROXIED: &[(&str, &str)] = &[
    ("db", "db list"),
    ("calendar", "calendar list"),
    ("project", "project list"),
    ("store", "store list"),
    ("workspace", "workspace list"),
];
/// A [`PROXIED`] probe's name after its form.
const PROXIED_AS: &str = "(K2_HOST=https://<name> through https_proxy)";
/// Stand-ins for programs that act on the machine.
const STAND_INS: &[&str] = &[
    "open",
    "xdg-open",
    "launchctl",
    "systemctl",
    "osascript",
    "security",
    "sudo",
    "pkill",
    "killall",
    "vi",
    "vim",
    "nano",
    "ssh",
    "scp",
    "docker",
    "podman",
];

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn read(rel: &str) -> String {
    let p = root().join(rel);
    fs::read_to_string(&p).unwrap_or_else(|e| panic!("read {}: {e}", p.display()))
}

/// The rows of a tab-separated data file under its header, which must be
/// `head`.
fn rows(rel: &str, head: &[&str]) -> Vec<Vec<String>> {
    let text = read(rel);
    let mut lines = text.lines().filter(|l| !l.starts_with('#'));
    assert_eq!(lines.next(), Some(head.join("\t").as_str()), "{rel}");
    lines
        .map(|l| {
            let cols: Vec<String> = l.split('\t').map(str::to_string).collect();
            assert_eq!(cols.len(), head.len(), "{rel}: {l}");
            cols
        })
        .collect()
}

#[derive(Debug, Clone)]
struct Probe {
    class: String,
    family: String,
    form: String,
    argv: Vec<String>,
    env: Vec<(String, String)>,
}

fn words(s: &str) -> Vec<String> {
    s.split(' ').map(str::to_string).collect()
}

fn probes() -> Vec<Probe> {
    let mut out = Vec::new();
    let mut firsts = BTreeSet::new();
    let head = [
        "form", "argv", "alias_of", "anchor", "entry", "json", "source",
    ];
    let forms = rows(FORMS, &head);
    // A family is its router arm's first word: `docs` is `doc`'s.
    let arm: std::collections::BTreeMap<String, String> = forms
        .iter()
        .filter(|r| !r[0].contains(' '))
        .map(|r| {
            let canon = if r[2] == "-" { &r[0] } else { &r[2] };
            (r[0].clone(), canon.clone())
        })
        .collect();
    let family_of = |word: &str| arm.get(word).cloned().unwrap_or_else(|| word.to_string());
    for r in &forms {
        let (form, json) = (&r[0], &r[5]);
        let argv = words(if r[1] == "-" { form } else { &r[1] });
        let family = family_of(&argv[0]);
        firsts.insert(argv[0].clone());
        let help = form
            .split(' ')
            .any(|w| matches!(w, "help" | "--help" | "-h"));
        let probe = |class: &str, form: String, argv: Vec<String>| Probe {
            class: class.to_string(),
            family: family.clone(),
            form,
            argv,
            env: Vec::new(),
        };
        out.push(probe(
            if help { "H" } else { "V" },
            form.clone(),
            argv.clone(),
        ));
        if !help {
            let mut a = argv.clone();
            a.push("--help".into());
            out.push(probe("H", format!("{form} --help"), a));
        }
        if json == "y" {
            let mut a = argv.clone();
            a.push("--json".into());
            out.push(probe("V", format!("{form} --json"), a));
        }
    }
    for r in rows(SHAPES, &["class", "form", "argv", "env", "reason"]) {
        assert!(
            matches!(r[0].as_str(), "F" | "E" | "P"),
            "{SHAPES}: {r:?}: the class is F, E or P"
        );
        assert!(!r[4].trim().is_empty(), "{SHAPES}: {r:?}: no reason");
        let argv = words(&r[2]);
        assert!(
            firsts.contains(&argv[0]),
            "{SHAPES}: {r:?}: its first word is no router form's"
        );
        let env = if r[3] == "-" {
            Vec::new()
        } else {
            r[3].split(',')
                .map(|kv| {
                    let (k, v) = kv
                        .split_once('=')
                        .unwrap_or_else(|| panic!("{SHAPES}: {r:?}: env is NAME=value"));
                    (k.to_string(), v.to_string())
                })
                .collect()
        };
        out.push(Probe {
            class: r[0].clone(),
            family: family_of(&argv[0]),
            form: r[1].clone(),
            argv,
            env,
        });
    }
    out.extend(proxied());
    let mut seen = BTreeSet::new();
    for p in &out {
        assert!(seen.insert(&p.form), "two probes are named {:?}", p.form);
    }
    out
}

/// The [`PROXIED`] probes: each names a router form.
fn proxied() -> Vec<Probe> {
    let forms: BTreeSet<String> = rows(
        FORMS,
        &[
            "form", "argv", "alias_of", "anchor", "entry", "json", "source",
        ],
    )
    .into_iter()
    .map(|r| r[0].clone())
    .collect();
    PROXIED
        .iter()
        .map(|(family, form)| {
            assert!(forms.contains(*form), "{form:?} is no router form");
            Probe {
                class: "V".into(),
                family: (*family).into(),
                form: format!("{form} {PROXIED_AS}"),
                argv: words(form),
                env: Vec::new(),
            }
        })
        .collect()
}

fn is_proxied(p: &Probe) -> bool {
    p.form.ends_with(PROXIED_AS)
}

/// A scratch machine: home, K2 home with the owner token and the stub's
/// port, a project, and the `PATH` the binary runs with.
struct Machine {
    dir: PathBuf,
    home: PathBuf,
    proj: PathBuf,
    tmp: PathBuf,
    path: String,
    stub: Stub,
}

fn scratch() -> Machine {
    let dir = std::env::temp_dir().join(format!("k5-native-bar-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    let home = dir.join("home");
    let k2home = k2_client::paths::k2_home_in(&home);
    let proj = dir.join("proj");
    let tmp = dir.join("tmp");
    let tools = dir.join("stand-ins");
    for d in [&k2home, &proj.join(".k2"), &tmp, &tools] {
        fs::create_dir_all(d).unwrap();
    }
    let stub = Stub::start(Path::new(&format!("/tmp/k5nb-{}.sock", std::process::id())));
    fs::write(k2home.join("heartbeat.token"), "owner-tok-7171\n").unwrap();
    fs::write(k2home.join("heartbeat.port"), format!("{}\n", stub.port)).unwrap();
    for name in STAND_INS {
        let p = tools.join(name);
        fs::write(&p, "#!/bin/sh\nexit 1\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let path = format!("{}:{}", tools.display(), common::path_without_python3(&dir));
    Machine {
        dir,
        home,
        proj,
        tmp,
        path,
        stub,
    }
}

/// Runs `p` with `env` on top of its own: its exit (`None`: still
/// running at [`DEADLINE`], killed) and its stderr.
fn spawn(b: &Machine, p: &Probe, env: &[(String, String)]) -> (Option<ExitStatus>, String) {
    let mut cmd = Command::new(BIN);
    cmd.env_clear()
        .env("PATH", &b.path)
        .env("HOME", &b.home)
        .env("TMPDIR", &b.tmp)
        .envs(p.env.iter().map(|(k, v)| (k.as_str(), v.as_str())))
        .envs(env.iter().map(|(k, v)| (k.as_str(), v.as_str())))
        .current_dir(&b.proj)
        .args(&p.argv)
        .process_group(0)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    let mut child = cmd
        .spawn()
        .unwrap_or_else(|e| panic!("run {:?}: {e}", p.argv));
    let mut pipe = child.stderr.take().expect("its stderr");
    let reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        std::io::Read::read_to_end(&mut pipe, &mut buf).expect("read its stderr");
        buf
    });
    let started = Instant::now();
    let status = loop {
        if let Some(s) = child
            .try_wait()
            .unwrap_or_else(|e| panic!("wait {:?}: {e}", p.argv))
        {
            break Some(s);
        }
        if started.elapsed() >= DEADLINE {
            let group = i32::try_from(child.id()).expect("a pid");
            // SAFETY: a plain kill of the group the child leads.
            unsafe {
                libc::killpg(group, libc::SIGKILL);
            }
            let _ = child.wait();
            break None;
        }
        std::thread::sleep(Duration::from_millis(5));
    };
    let stderr = String::from_utf8_lossy(&reader.join().expect("its stderr")).into_owned();
    (status, stderr)
}

/// Whether `p` falls through: the hand-off's exit and line.
fn falls_through(b: &Machine, p: &Probe) -> bool {
    let (status, stderr) = spawn(b, p, &[]);
    let Some(status) = status else {
        // Still in its own code at the deadline: native.
        return false;
    };
    let said = stderr.contains(NO_PYTHON);
    assert!(
        !said || status.code() == Some(69),
        "{:?}: the missing-Python line with exit {:?}:\n{stderr}",
        p.argv,
        status.code()
    );
    said
}

/// The probes that fall through, in probe order: the [`PROXIED`] ones
/// after the others, one at a time.
fn run(probes: &[Probe]) -> Vec<Probe> {
    let b = scratch();
    let (plain, proxy): (Vec<&Probe>, Vec<&Probe>) = probes.iter().partition(|p| !is_proxied(p));
    let next = AtomicUsize::new(0);
    let fell = Mutex::new(vec![false; plain.len()]);
    std::thread::scope(|s| {
        for _ in 0..WORKERS {
            s.spawn(|| loop {
                let i = next.fetch_add(1, Ordering::SeqCst);
                let Some(p) = plain.get(i) else { break };
                let f = falls_through(&b, p);
                fell.lock().expect("the results")[i] = f;
            });
        }
    });
    let fell = fell.into_inner().expect("the results");
    let pki = tunnel::Pki::for_names(&b.dir.join("pki"), &["0.0.0.0"]);
    let proxy_fell: Vec<bool> = proxy
        .iter()
        .map(|p| proxied_falls_through(&b, &pki, p))
        .collect();
    let _ = fs::remove_dir_all(&b.dir);
    plain
        .into_iter()
        .zip(fell)
        .chain(proxy.into_iter().zip(proxy_fell))
        .filter(|(_, f)| *f)
        .map(|(p, _)| p.clone())
        .collect()
}

/// A [`PROXIED`] probe, against a stub proxy and a stub `https://`
/// daemon of its own, both recording: whether it falls through (it sent
/// nothing and failed: closed). Fails if anything reached the local
/// plain-http daemon, went past the proxy other than in a CONNECT, put
/// the token in a CONNECT, or reached the daemon other than through a
/// tunnel; or if it answered 0 having sent nothing.
fn proxied_falls_through(b: &Machine, pki: &tunnel::Pki, p: &Probe) -> bool {
    let daemon = TlsStub::start(pki, 200, "{}");
    let proxy = ProxyStub::start(ProxyMode::Tunnel);
    b.stub.take_log();
    let env: Vec<(String, String)> = [
        ("K2_HOST", "https://0.0.0.0".to_string()),
        ("K2_PORT", daemon.port.to_string()),
        ("SSL_CERT_FILE", pki.ca_file.to_string_lossy().into_owned()),
        ("https_proxy", proxy.url(None)),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v))
    .collect();
    let (status, stderr) = spawn(b, p, &env);
    // The stubs log on their own threads: let a connection that ended
    // with the process land.
    std::thread::sleep(Duration::from_millis(300));
    let heads = tunnel::take(&proxy.heads);
    let sent = tunnel::take(&daemon.log);
    let broken = tunnel::take(&daemon.errors);
    let plain = b.stub.unread();
    let record = format!(
        "exit {:?}; CONNECTs {heads:?}; daemon requests {sent:?}; daemon connections with no request {broken:?}; local plain daemon {} requests; stderr {stderr:?}",
        status.map(|s| s.code()),
        plain.len()
    );
    let name = &p.form;
    let Some(status) = status else {
        panic!("{name}: still running at the deadline: {record}");
    };
    assert!(
        plain.is_empty(),
        "{name}: the local plain-http daemon got requests: {record}"
    );
    let target = format!("CONNECT 0.0.0.0:{} HTTP/1.1\r\n", daemon.port);
    for h in &heads {
        assert!(
            h.starts_with(&target),
            "{name}: the proxy got more than a CONNECT: {record}"
        );
        assert!(
            !h.contains(OWNER),
            "{name}: the token reached the proxy: {record}"
        );
    }
    assert!(
        sent.len() + broken.len() <= heads.len(),
        "{name}: the daemon was reached other than through the tunnel: {record}"
    );
    assert!(
        !stderr.contains(OWNER),
        "{name}: the token was printed: {record}"
    );
    let fell = sent.is_empty();
    if fell {
        assert!(
            !status.success(),
            "{name}: exit 0 with nothing sent: {record}"
        );
        assert!(
            heads.is_empty() && broken.is_empty(),
            "{name}: a tunnel with no request in it: {record}"
        );
    }
    println!(
        "{name}: {}: {record}",
        if fell {
            "sent nothing, failed closed"
        } else {
            "native: through the tunnel"
        }
    );
    fell
}

fn ceiling_text(fell: &[Probe], total: usize) -> String {
    let mut counts = std::collections::BTreeMap::new();
    for p in fell {
        *counts.entry(p.class.as_str()).or_insert(0) += 1;
    }
    let by: Vec<String> = ["V", "H", "F", "E", "P"]
        .iter()
        .map(|c| format!("{c} {}", counts.get(c).copied().unwrap_or(0)))
        .collect();
    let mut s = String::from(
        "# GENERATED by crates/k5-cli/tests/native_bar.rs (NATIVE_BAR_WRITE=<file>): the probes\n\
         # k5 still hands to bash. It may only fall; the ship bar is no row of a counted class\n\
         # (V verb, H help, F flag shape, E edge; P is listed, not counted).\n",
    );
    s.push_str(&format!(
        "# {} of {total} probes fall through: {}\n",
        fell.len(),
        by.join(", ")
    ));
    s.push_str("class\tfamily\tform\n");
    for p in fell {
        s.push_str(&format!("{}\t{}\t{}\n", p.class, p.family, p.form));
    }
    s
}

/// Ceiling rows for a failure message, one per line, the columns split
/// by ` | ` (a test runner's log may drop a tab).
fn listing(rows: &[&String]) -> String {
    rows.iter()
        .map(|r| r.replace('\t', " | "))
        .collect::<Vec<_>>()
        .join("\n  ")
}

#[test]
fn the_native_bar_ceiling_holds() {
    let probes = probes();
    let fell = run(&probes);
    let found = ceiling_text(&fell, probes.len());
    if let Ok(to) = std::env::var("NATIVE_BAR_WRITE") {
        fs::write(&to, &found).unwrap_or_else(|e| panic!("write {to}: {e}"));
    }
    let rows_of = |text: &str| -> BTreeSet<String> {
        text.lines()
            .filter(|l| !l.starts_with('#') && *l != "class\tfamily\tform")
            .map(str::to_string)
            .collect()
    };
    let now = rows_of(&found);
    let ceiling = rows_of(&read(CEILING));
    let names: BTreeSet<String> = probes
        .iter()
        .map(|p| format!("{}\t{}\t{}", p.class, p.family, p.form))
        .collect();
    let rose: Vec<&String> = now.difference(&ceiling).collect();
    let fell_off: Vec<&String> = ceiling
        .difference(&now)
        .filter(|r| names.contains(*r))
        .collect();
    let stray: Vec<&String> = ceiling.iter().filter(|r| !names.contains(*r)).collect();
    let mut problems = String::new();
    if !rose.is_empty() {
        problems += &format!(
            "these fall through and the ceiling doesn't list them ({}):\n  {}\n",
            rose.len(),
            listing(&rose)
        );
    }
    if !fell_off.is_empty() {
        problems += &format!(
            "these are native now: take them out of {CEILING} ({}):\n  {}\n",
            fell_off.len(),
            listing(&fell_off)
        );
    }
    if !stray.is_empty() {
        problems += &format!(
            "these ceiling rows name no probe ({}):\n  {}\n",
            stray.len(),
            listing(&stray)
        );
    }
    println!("{}", found.lines().nth(3).unwrap_or_default());
    assert!(problems.is_empty(), "the native bar:\n{problems}");
}

/// The rows the bar reads are current: `scripts/router-forms.py --check`.
#[test]
fn the_router_forms_are_current() {
    let o = Command::new("python3")
        .arg("-I")
        .arg(root().join("scripts/router-forms.py"))
        .arg("--check")
        .output()
        .expect("run python3 scripts/router-forms.py --check");
    assert!(
        o.status.success(),
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    );
}
