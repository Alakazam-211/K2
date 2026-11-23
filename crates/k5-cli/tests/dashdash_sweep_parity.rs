//! A bare `--` on every form the shell router takes, the binary against
//! the shell CLI's `cli/k2`: the same exit, stdout, stderr and requests.
//!
//! The forms are `tests/data/router_forms.tsv` (`scripts/router-forms.py`
//! reads them from the router and each family's dispatch), each with
//! its `argv` when it has one; a help form is left out. Each form runs
//! twice, `<form> -- x-k5-sweep` and `<form> -- -h`. A verb that takes no
//! `--` must refuse it and send nothing (the shell's `_k2_takes_dashdash`,
//! or the family's own rule, as `ci signer`'s); a verb that takes one
//! must read it as the shell does. Either way the binary must answer as
//! the shell does, byte for byte.
//!
//! How: one scratch machine per side (home, K2 home with the owner token
//! and its own stub daemon's port, a project), stdin closed, stand-ins
//! first on `PATH` for programs that act on the machine, and
//! `K2_APPROVE_BIN` a k2-approve stand-in that prints its words. The
//! stub answers every request with a plain 200. The scratch folder and the stub's port
//! are written `<dir>` and `<port>` before the two sides are compared; a
//! request is its request line and its body. A side still running at
//! [`DEADLINE`] is killed and counts as hung (both hung is the same).
//!
//! A line that differs has a class, worst first: A the binary sends a
//! write the shell doesn't (always a failure: fix it), B the reads differ,
//! C the shell sends a write the binary doesn't, D only the exit, stdout or
//! stderr differ. A B, C or D line passes only when
//! `tests/data/dashdash_sweep_named.tsv` names it with its class and why;
//! a named line that no longer differs so fails too. The counts per class
//! and family, and every A and B line, are printed first.
//! `DASHDASH_SWEEP_OUT=<folder>` writes every differing line in full
//! (`sweep-rows.txt`) and the named list as the run found it
//! (`sweep-named.tsv`).
//!
//! A line where either side prints a Python traceback fails unless its
//! family is one of [`TRACEBACK_FAMILIES`], the families still to fix: a
//! person or agent gets one plain line and an exit code, never a
//! traceback (whose words change with the Python version).
//!
//! It prints each form where the shell sends a write (anything but a GET)
//! after a `--`, marked when the request holds the `--` itself as a value:
//! that is the shell acting on a word it should not have read, a shell
//! bug to fix in the shell, never here.

#![cfg(unix)]

mod common;

use common::parity::{cli_k2, fold_query_escapes};
use common::stub::Stub;
use std::collections::BTreeSet;
use std::fs;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

const BIN: &str = env!("CARGO_BIN_EXE_k5-cli");
const FORMS: &str = "crates/k5-cli/tests/data/router_forms.tsv";
const OWNER: &str = "owner-tok-7171";
const DEADLINE: Duration = Duration::from_secs(15);
const WORKERS: usize = 8;
/// What follows each form.
const TAILS: &[&[&str]] = &[&["--", "x-k5-sweep"], &["--", "-h"]];
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

/// Each form's words with each tail after them, in the file's order, each
/// line once.
fn probes() -> Vec<Vec<String>> {
    let p = root().join(FORMS);
    let text = fs::read_to_string(&p).unwrap_or_else(|e| panic!("read {}: {e}", p.display()));
    let mut lines = text.lines().filter(|l| !l.starts_with('#'));
    assert_eq!(
        lines.next(),
        Some("form\targv\talias_of\tanchor\tentry\tjson\tsource"),
        "{FORMS}: its header"
    );
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for l in lines {
        let cols: Vec<&str> = l.split('\t').collect();
        assert_eq!(cols.len(), 7, "{FORMS}: {l}");
        let words: Vec<String> = if cols[1] == "-" { cols[0] } else { cols[1] }
            .split(' ')
            .map(str::to_string)
            .collect();
        if words
            .iter()
            .any(|w| matches!(w.as_str(), "help" | "--help" | "-h"))
        {
            continue;
        }
        for tail in TAILS {
            let mut argv = words.clone();
            argv.extend(tail.iter().map(|w| w.to_string()));
            if seen.insert(argv.clone()) {
                out.push(argv);
            }
        }
    }
    assert!(out.len() > 1000, "{FORMS}: only {} probes", out.len());
    out
}

/// One side's scratch machine.
struct Machine {
    dir: PathBuf,
    home: PathBuf,
    k2home: PathBuf,
    proj: PathBuf,
    tmp: PathBuf,
    path: String,
    /// `K2_APPROVE_BIN`: the k2-approve stand-in.
    approve: PathBuf,
    stub: Stub,
}

fn machine(tag: &str) -> Machine {
    let dir = std::env::temp_dir().join(format!("k5-ddsweep-{tag}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    let home = dir.join("home");
    let k2home = k2_client::paths::k2_home_in(&home);
    let proj = dir.join("proj");
    let tmp = dir.join("tmp");
    let tools = dir.join("stand-ins");
    fs::create_dir_all(&tools).unwrap();
    for name in STAND_INS {
        let p = tools.join(name);
        fs::write(&p, "#!/bin/sh\nexit 1\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
    }
    // `approve verify`, `show` and `key-id` run the k2-approve program,
    // which each CLI looks for in its own place (beside cli/k2, beside the
    // binary, on PATH): both get this stand-in, so the line doesn't depend
    // on what a host has built.
    let approve = tools.join("k2-approve-stand-in");
    fs::write(
        &approve,
        "#!/bin/sh\necho \"k2-approve stand-in: $*\" >&2\nexit 2\n",
    )
    .unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&approve, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let path = format!(
        "{}:{}",
        tools.display(),
        std::env::var("PATH").expect("a PATH to run the tests with")
    );
    let stub = Stub::start(Path::new(&format!(
        "/tmp/k5dd{tag}-{}.sock",
        std::process::id()
    )));
    Machine {
        dir,
        home,
        k2home,
        proj,
        tmp,
        path,
        approve,
        stub,
    }
}

/// What one side did.
#[derive(Debug, PartialEq, Eq)]
struct Got {
    /// `None`: still running at [`DEADLINE`].
    code: Option<i32>,
    stdout: String,
    stderr: String,
    requests: Vec<String>,
}

impl Machine {
    /// A fresh home, K2 home and project, and an empty stub log.
    fn reset(&self) {
        for d in [&self.home, &self.proj, &self.tmp] {
            let _ = fs::remove_dir_all(d);
        }
        for d in [&self.k2home, &self.proj.join(".k2"), &self.tmp] {
            fs::create_dir_all(d).unwrap();
        }
        fs::write(self.k2home.join("heartbeat.token"), format!("{OWNER}\n")).unwrap();
        fs::write(
            self.k2home.join("heartbeat.port"),
            format!("{}\n", self.stub.port),
        )
        .unwrap();
        self.stub.reset(&[]);
    }

    /// The scratch folder (as typed and URL-encoded), the shell CLI's
    /// path, the stub's port and a run's random job id, written the same
    /// on both sides. Nothing else is masked: a traceback is compared as
    /// printed.
    fn mask(&self, text: &str) -> String {
        let dir = self.dir.to_string_lossy().to_string();
        let encoded = dir.replace('/', "%2F");
        let text = text
            .replace(&dir, "<dir>")
            .replace(&encoded, "<dir>")
            .replace(&cli_k2().to_string_lossy().to_string(), "<cli>")
            .replace(&self.stub.port.to_string(), "<port>");
        mask_after(&text, "\"clientJobId\": \"")
    }

    fn run(&self, bash: bool, argv: &[String]) -> Got {
        self.reset();
        let mut cmd = if bash {
            let mut c = Command::new("bash");
            c.arg(cli_k2());
            c
        } else {
            Command::new(BIN)
        };
        cmd.env_clear()
            .env("PATH", &self.path)
            .env("HOME", &self.home)
            .env("TMPDIR", &self.tmp)
            .env("K2_APPROVE_BIN", &self.approve)
            .current_dir(&self.proj)
            .args(argv)
            .process_group(0)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if bash {
            cmd.env("K5_CLI_VIA", "rust");
        }
        let mut child = cmd.spawn().unwrap_or_else(|e| panic!("run {argv:?}: {e}"));
        let mut out_pipe = child.stdout.take().expect("its stdout");
        let mut err_pipe = child.stderr.take().expect("its stderr");
        let out_reader = std::thread::spawn(move || {
            let mut buf = Vec::new();
            std::io::Read::read_to_end(&mut out_pipe, &mut buf).expect("read its stdout");
            buf
        });
        let err_reader = std::thread::spawn(move || {
            let mut buf = Vec::new();
            std::io::Read::read_to_end(&mut err_pipe, &mut buf).expect("read its stderr");
            buf
        });
        let started = Instant::now();
        let code = loop {
            if let Some(s) = child
                .try_wait()
                .unwrap_or_else(|e| panic!("wait {argv:?}: {e}"))
            {
                use std::os::unix::process::ExitStatusExt;
                break Some(s.code().unwrap_or_else(|| 128 + s.signal().unwrap_or(0)));
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
        let stdout = String::from_utf8_lossy(&out_reader.join().expect("its stdout")).into_owned();
        let stderr = String::from_utf8_lossy(&err_reader.join().expect("its stderr")).into_owned();
        let requests = self
            .stub
            .take_log()
            .iter()
            .map(|r| {
                let text = String::from_utf8_lossy(&r.bytes).into_owned();
                let (head, body) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
                let line = fold_query_escapes(head.split("\r\n").next().unwrap_or_default());
                self.mask(&format!("{line} {body}"))
            })
            .collect();
        Got {
            code,
            stdout: self.mask(&stdout),
            stderr: self.mask(&stderr),
            requests,
        }
    }
}

/// `text` with the hex digits after each `key` written `<id>`.
fn mask_after(text: &str, key: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(at) = rest.find(key) {
        let (head, tail) = rest.split_at(at + key.len());
        out.push_str(head);
        let n = tail.bytes().take_while(u8::is_ascii_hexdigit).count();
        if n > 0 {
            out.push_str("<id>");
        }
        rest = &tail[n..];
    }
    out.push_str(rest);
    out
}

/// A write the shell sent, marked when it holds the `--` itself as a
/// value (the word a verb should never have read).
fn write_of(argv: &[String], request: &str) -> Option<String> {
    let method = request.split(' ').next().unwrap_or_default();
    if method == "GET" {
        return None;
    }
    let suspect = ["\"--\"", "/--", "=--", "%2D%2D"]
        .iter()
        .any(|needle| request.contains(needle));
    let mark = if suspect { "SUSPECT" } else { "write" };
    Some(format!("{mark}: k2 {}: {request}", argv.join(" ")))
}

fn shown(g: &Got) -> String {
    format!(
        "exit {:?}\n  stdout: {:?}\n  stderr: {:?}\n  requests: {:?}",
        g.code, g.stdout, g.stderr, g.requests
    )
}

fn is_write(request: &str) -> bool {
    !request.starts_with("GET ")
}

/// How a line differs, worst first: `A` the binary sends a write the
/// shell doesn't; `B` the reads differ; `C` the shell sends a write the
/// binary doesn't; `D` the same requests, and only the exit, stdout or
/// stderr differ. `None`: no difference.
fn class(bash: &Got, k5: &Got) -> Option<char> {
    if bash == k5 {
        return None;
    }
    let writes =
        |g: &Got| -> Vec<String> { g.requests.iter().filter(|r| is_write(r)).cloned().collect() };
    let reads = |g: &Got| -> Vec<String> {
        g.requests
            .iter()
            .filter(|r| !is_write(r))
            .cloned()
            .collect()
    };
    let (bw, kw) = (writes(bash), writes(k5));
    Some(if kw.iter().any(|r| !bw.contains(r)) {
        'A'
    } else if reads(bash) != reads(k5) {
        'B'
    } else if bw != kw {
        'C'
    } else {
        'D'
    })
}

/// What differs on a line of class `B`, `C` or `D`, for the named list.
fn what(bash: &Got, k5: &Got) -> String {
    let mut parts = Vec::new();
    if bash.code != k5.code {
        parts.push(format!("exit {:?} vs {:?}", bash.code, k5.code));
    }
    if bash.stdout != k5.stdout {
        parts.push("stdout".to_string());
    }
    if bash.stderr != k5.stderr {
        parts.push("stderr".to_string());
    }
    if bash.requests != k5.requests {
        parts.push(format!(
            "requests: bash {}, k5 {}",
            bash.requests.len(),
            k5.requests.len()
        ));
    }
    parts.join("; ")
}

/// The named differences: `class<TAB>line<TAB>reason`, the line as
/// `k2 <words>`.
const NAMED: &str = "crates/k5-cli/tests/data/dashdash_sweep_named.tsv";

fn named() -> Vec<(char, String)> {
    let p = root().join(NAMED);
    let text = fs::read_to_string(&p).unwrap_or_else(|e| panic!("read {}: {e}", p.display()));
    let mut lines = text.lines().filter(|l| !l.starts_with('#'));
    assert_eq!(
        lines.next(),
        Some("class\tline\treason"),
        "{NAMED}: its header"
    );
    lines
        .map(|l| {
            let cols: Vec<&str> = l.split('\t').collect();
            assert_eq!(cols.len(), 3, "{NAMED}: {l}");
            let c = cols[0].chars().next().unwrap_or(' ');
            assert!(
                matches!(c, 'B' | 'C' | 'D') && cols[0].len() == 1,
                "{NAMED}: {l}: the class is B, C or D (a write only the binary sends is never named)"
            );
            assert!(!cols[2].trim().is_empty(), "{NAMED}: {l}: no reason");
            (c, cols[1].to_string())
        })
        .collect()
}

/// The families whose shell still prints a Python traceback on some line
/// of this sweep: a reader that can't read the stub's plain 200 raises.
/// Each is a bug to fix as `ci`, `doc`, `organization` and `compute` were
/// (0.46.18: the family's own plain error on stderr, exit 1), and a fixed
/// family comes off this list. A family not on it
/// that prints a traceback fails, and so does a listed one that no longer
/// prints any, so the list stays exact.
const TRACEBACK_FAMILIES: &[&str] = &["app"];

/// Whether a side printed a Python traceback.
fn has_traceback(g: &Got) -> bool {
    const HEAD: &str = "Traceback (most recent call last):";
    g.stdout.contains(HEAD) || g.stderr.contains(HEAD)
}

struct Row {
    class: char,
    line: String,
    what: String,
    shown: String,
}

#[test]
fn every_form_with_a_dashdash_answers_as_the_shell_does() {
    let probes = probes();
    let named = named();
    let next = AtomicUsize::new(0);
    let differ = Mutex::new(Vec::new());
    let writes = Mutex::new(Vec::new());
    let hung = Mutex::new(Vec::new());
    let tracebacks = Mutex::new(Vec::new());
    std::thread::scope(|s| {
        for w in 0..WORKERS {
            let (probes, next, differ, writes, hung, tracebacks) =
                (&probes, &next, &differ, &writes, &hung, &tracebacks);
            s.spawn(move || {
                let bash_side = machine(&format!("b{w}"));
                let rust_side = machine(&format!("r{w}"));
                loop {
                    let i = next.fetch_add(1, Ordering::SeqCst);
                    let Some(argv) = probes.get(i) else {
                        break;
                    };
                    let b = bash_side.run(true, argv);
                    let r = rust_side.run(false, argv);
                    let line = format!("k2 {}", argv.join(" "));
                    if b.code.is_none() {
                        hung.lock().unwrap().push(line.clone());
                    }
                    if has_traceback(&b) || has_traceback(&r) {
                        tracebacks.lock().unwrap().push(argv[0].clone());
                    }
                    for req in &b.requests {
                        if let Some(wr) = write_of(argv, req) {
                            writes.lock().unwrap().push(wr);
                        }
                    }
                    if let Some(c) = class(&b, &r) {
                        differ.lock().unwrap().push(Row {
                            class: c,
                            what: what(&b, &r),
                            shown: format!("{line}\n bash: {}\n k5:   {}", shown(&b), shown(&r)),
                            line,
                        });
                    }
                }
                let _ = fs::remove_dir_all(&bash_side.dir);
                let _ = fs::remove_dir_all(&rust_side.dir);
            });
        }
    });
    let mut differ = differ.into_inner().unwrap();
    let mut writes = writes.into_inner().unwrap();
    let mut hung = hung.into_inner().unwrap();
    let tracebacks: BTreeSet<String> = tracebacks.into_inner().unwrap().into_iter().collect();
    differ.sort_by(|a, b| (a.class, &a.line).cmp(&(b.class, &b.line)));
    writes.sort();
    hung.sort();
    let count = |c: char| differ.iter().filter(|r| r.class == c).count();
    println!(
        "dashdash sweep: {} probes, {} differ: A={} B={} C={} D={}",
        probes.len(),
        differ.len(),
        count('A'),
        count('B'),
        count('C'),
        count('D')
    );
    let mut per_family = std::collections::BTreeMap::new();
    for r in &differ {
        let family = r.line.split(' ').nth(1).unwrap_or_default().to_string();
        *per_family.entry((r.class, family)).or_insert(0usize) += 1;
    }
    for ((c, family), n) in &per_family {
        println!("dashdash sweep: class {c} {family} {n}");
    }
    for r in differ.iter().filter(|r| matches!(r.class, 'A' | 'B')) {
        println!("dashdash sweep row {}: {}", r.class, r.shown);
    }
    let suspects = writes.iter().filter(|w| w.starts_with("SUSPECT")).count();
    println!(
        "dashdash sweep: {} shell writes after a -- ({suspects} suspect), {} hung on the shell",
        writes.len(),
        hung.len()
    );
    for w in &writes {
        println!("  {w}");
    }
    for h in &hung {
        println!("  hung: {h}");
    }
    // DASHDASH_SWEEP_OUT=<folder>: every differing row in full, and the
    // named list as this run found it.
    if let Some(out) = std::env::var_os("DASHDASH_SWEEP_OUT") {
        let out = PathBuf::from(out);
        fs::create_dir_all(&out).unwrap();
        let rows: Vec<String> = differ
            .iter()
            .map(|r| format!("[{}] {}", r.class, r.shown))
            .collect();
        fs::write(out.join("sweep-rows.txt"), rows.join("\n") + "\n").unwrap();
        let mut tsv = String::from("class\tline\treason\n");
        for r in differ.iter().filter(|r| r.class != 'A') {
            tsv.push_str(&format!("{}\t{}\t{}\n", r.class, r.line, r.what));
        }
        fs::write(out.join("sweep-named.tsv"), tsv).unwrap();
    }
    let mut failures = Vec::new();
    for r in &differ {
        if !named.iter().any(|(c, l)| *c == r.class && *l == r.line) {
            failures.push(format!("[{}] {}", r.class, r.shown));
        }
    }
    for (c, l) in &named {
        if !differ.iter().any(|r| r.class == *c && r.line == *l) {
            failures.push(format!(
                "{NAMED}: [{c}] {l}: named, but it no longer differs so"
            ));
        }
    }
    println!("dashdash sweep: families with a traceback: {tracebacks:?}");
    for f in &tracebacks {
        if !TRACEBACK_FAMILIES.contains(&f.as_str()) {
            failures.push(format!(
                "family {f}: a line printed a Python traceback (one plain line and an exit code instead)"
            ));
        }
    }
    for f in TRACEBACK_FAMILIES {
        if !tracebacks.contains(*f) {
            failures.push(format!(
                "family {f}: no traceback any more: take it off TRACEBACK_FAMILIES"
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{} line(s) with a -- answer otherwise than the shell and not named (or named and gone):\n{}",
        failures.len(),
        failures.join("\n")
    );
}
