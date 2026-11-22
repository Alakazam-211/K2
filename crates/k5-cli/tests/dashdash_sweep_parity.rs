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
//! first on `PATH` for programs that act on the machine. The stub answers
//! every request with a plain 200. The scratch folder and the stub's port
//! are written `<dir>` and `<port>` before the two sides are compared; a
//! request is its request line and its body. A side still running at
//! [`DEADLINE`] is killed and counts as hung (both hung is the same).
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

    fn mask(&self, text: &str) -> String {
        text.replace(&self.dir.to_string_lossy().to_string(), "<dir>")
            .replace(&cli_k2().to_string_lossy().to_string(), "<cli>")
            .replace(&self.stub.port.to_string(), "<port>")
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

#[test]
fn every_form_with_a_dashdash_answers_as_the_shell_does() {
    let probes = probes();
    let next = AtomicUsize::new(0);
    let differ = Mutex::new(Vec::new());
    let writes = Mutex::new(Vec::new());
    let hung = Mutex::new(Vec::new());
    std::thread::scope(|s| {
        for w in 0..WORKERS {
            let (probes, next, differ, writes, hung) = (&probes, &next, &differ, &writes, &hung);
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
                    for req in &b.requests {
                        if let Some(wr) = write_of(argv, req) {
                            writes.lock().unwrap().push(wr);
                        }
                    }
                    if b != r {
                        differ.lock().unwrap().push(format!(
                            "{line}\n bash: {}\n k5:   {}",
                            shown(&b),
                            shown(&r)
                        ));
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
    differ.sort();
    writes.sort();
    hung.sort();
    let suspects = writes.iter().filter(|w| w.starts_with("SUSPECT")).count();
    println!(
        "dashdash sweep: {} probes, {} differ, {} shell writes after a -- ({suspects} suspect), {} hung on the shell",
        probes.len(),
        differ.len(),
        writes.len(),
        hung.len()
    );
    for w in &writes {
        println!("  {w}");
    }
    for h in &hung {
        println!("  hung: {h}");
    }
    assert!(
        differ.is_empty(),
        "{} line(s) with a -- answer otherwise than the shell:\n{}",
        differ.len(),
        differ.join("\n")
    );
}
