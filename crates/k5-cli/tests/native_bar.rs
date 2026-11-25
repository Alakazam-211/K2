//! The native bar: which command lines the k5 binary still hands to the
//! shell CLI. It is the 0.46.17 ship bar: every command
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
//!   proxy: see [`proxied_falls_through`].
//!
//! How: the built binary, with a scratch home and K2 home, a stub daemon
//! that answers every request with 200 and the owner token and port on
//! disk, stdin closed, and a `PATH` with no `python3` on it. Every
//! hand-off checks for `python3` before bash starts
//! ([`k5_cli::fallthrough::run`]), so a form that falls through exits 69
//! with its one line and runs nothing; in a build with no shell CLI
//! (the brand file's `cli.no_bash`, `K2_BRAND_DIR=brand/xy`) the hand-off
//! is refused before that, with its own line and exit 70, and that counts
//! as falling through too. Anything else is native. A probe
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
//!   counts these;
//! - P an edge that exists only because python3 is missing (the
//!   native-without-python3 difference): listed, not counted;
//! - R a run-time hand-off: a form under one environment or input
//!   variant (below). Listed; the ceiling-zero gate counts it only for a
//!   brand with no shell CLI (`cli.no_bash`), where each would be refused.
//!
//! The run-time rows, `tests/data/native_bar_runtime.tsv`, each run one
//! form (or, in a sweep, every router form) under one variant of
//! `tests/data/native_bar_variants.tsv`: a proxy by scheme, a locale,
//! HOME unset, CDPATH, a PWD bash would tidy, a TTY, bash's startup
//! variables, a staged app layout, an odd argument, and so on. They run
//! on a scratch machine of their own, so no variant reaches the fixed
//! rows, and that machine's `<K2 home>/run/cli` is a file: a hand-off
//! that gets past the Python 3 check (a variant whose shell has
//! `python3`) stops there with its line, and bash never starts.
//! `NATIVE_BAR_RUNTIME_REPORT=<file>` writes every run-time probe with
//! its site, variant, group, partner flag and whether it fell through.
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
/// The named, temporary allowance (K2's ruling, 0.46.18): exact rows that
/// may fall through for now, kept out of the ceiling, which stays at zero.
/// Its `native_bar` rows are `kind, class, family, form`.
const ALLOWANCE: &str = "crates/k5-cli/tests/data/native_port_allowance.tsv";
/// The hand-off's line when there is no `python3` ([`k5_cli::fallthrough::FrontError::NoPython`]).
const NO_PYTHON: &str = "is not built into k5 yet and needs Python 3";
/// The hand-off's line in a build with no shell CLI
/// ([`k5_cli::fallthrough::FrontError::NoShellCli`]).
const NO_SHELL_CLI: &str = "this build has no shell CLI to run it";
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
/// The run-time rows (class R) and the variants they run under.
const RUNTIME: &str = "crates/k5-cli/tests/data/native_bar_runtime.tsv";
const VARIANTS: &str = "crates/k5-cli/tests/data/native_bar_variants.tsv";
/// A hand-off past the Python 3 check, on the run-time machine: its
/// cache folder, `<K2 home>/run/cli`, is a file there, so it stops with
/// this line before bash starts.
const CACHE_BLOCKED: &str = "can't write the shell CLI to ";
/// What the run-time machine keeps at `<K2 home>/run/cli`.
const BLOCKED: &str = "blocked: the native bar's run-time machine never runs the shell CLI\n";

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
    /// A run-time row's site and variant (class R only).
    run_time: Option<RunTime>,
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
            run_time: None,
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
            run_time: None,
        });
    }
    out.extend(run_time_probes(&forms, &family_of, &firsts));
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
                run_time: None,
            }
        })
        .collect()
}

fn is_proxied(p: &Probe) -> bool {
    p.form.ends_with(PROXIED_AS)
}

/// What a variant does besides setting and unsetting variables.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Special {
    /// stdin a pseudo-terminal (not the controlling terminal).
    Tty,
    /// stdout a pseudo-terminal (the same one, with [`Special::Tty`]).
    TtyStdout,
    /// A stand-in `python3` first on `PATH`: the shell's Python is Present.
    Python3,
    /// `PATH` without `mktemp`.
    NoMktemp,
    /// The binary run from a copy with the shot runner beside it.
    BesideRunner,
    /// From `<X>.app/Contents/MacOS/`, the runner in `_up_/cli`, no
    /// `_up_/cli/<name>`.
    AppNoBash,
    /// The same with a `_up_/cli/<name>`.
    AppDefault,
    /// Run in this folder (a template).
    Cwd(String),
    /// Run in a folder removed just before exec.
    CwdGone,
    /// This word added at the end of the argv (a template).
    Append(String),
}

impl Special {
    fn parse(word: &str) -> Special {
        match word {
            "tty" => Special::Tty,
            "tty-stdout" => Special::TtyStdout,
            "python3" => Special::Python3,
            "no-mktemp" => Special::NoMktemp,
            "beside-runner" => Special::BesideRunner,
            "app-nobash" => Special::AppNoBash,
            "app-default" => Special::AppDefault,
            "cwd-gone" => Special::CwdGone,
            _ => {
                if let Some(path) = word.strip_prefix("cwd=") {
                    Special::Cwd(path.to_string())
                } else if let Some(w) = word.strip_prefix("append=") {
                    Special::Append(w.to_string())
                } else {
                    panic!("{VARIANTS}: no special named {word:?}")
                }
            }
        }
    }
}

/// One row of [`VARIANTS`]. Values are templates ([`Machine::fill`]).
#[derive(Debug, Clone)]
struct Variant {
    name: String,
    group: String,
    partner: bool,
    set: Vec<(String, String)>,
    unset: Vec<String>,
    special: Vec<Special>,
}

impl Variant {
    fn has(&self, s: &Special) -> bool {
        self.special.contains(s)
    }
}

/// A run-time probe's row: its site and its variant (`None`: the
/// run-time machine as it is).
#[derive(Debug, Clone)]
struct RunTime {
    site: String,
    variant: Option<Variant>,
}

impl RunTime {
    fn has(&self, s: &Special) -> bool {
        self.variant.as_ref().is_some_and(|v| v.has(s))
    }
}

fn variants() -> Vec<Variant> {
    let list = |s: &str| -> Vec<String> {
        if s == "-" {
            Vec::new()
        } else {
            s.split(' ').map(str::to_string).collect()
        }
    };
    let out: Vec<Variant> = rows(
        VARIANTS,
        &[
            "variant", "group", "partner", "set", "unset", "special", "reason",
        ],
    )
    .into_iter()
    .map(|r| {
        assert!(!r[6].trim().is_empty(), "{VARIANTS}: {r:?}: no reason");
        let partner = match r[2].as_str() {
            "yes" => true,
            "no" => false,
            p => panic!("{VARIANTS}: {r:?}: partner is yes or no, not {p:?}"),
        };
        let set = list(&r[3])
            .into_iter()
            .map(|kv| {
                let (k, v) = kv
                    .split_once('=')
                    .unwrap_or_else(|| panic!("{VARIANTS}: {r:?}: set is NAME=value"));
                (k.to_string(), v.to_string())
            })
            .collect();
        let special = if r[5] == "-" {
            Vec::new()
        } else {
            r[5].split(',').map(Special::parse).collect()
        };
        Variant {
            name: r[0].clone(),
            group: r[1].clone(),
            partner,
            set,
            unset: list(&r[4]),
            special,
        }
    })
    .collect();
    let mut seen = BTreeSet::new();
    for v in &out {
        assert!(
            seen.insert(&v.name),
            "{VARIANTS}: two variants named {:?}",
            v.name
        );
    }
    out
}

/// The run-time probes ([`RUNTIME`]), class R. A row names one variant,
/// `-` (none) or `<prefix>*` (each variant whose name starts so); its
/// argv is words, or `*` (every router form that is no alias) or
/// `*first` (the first such form of each family with no help word), a
/// sweep. Every variant is some row's.
fn run_time_probes(
    forms: &[Vec<String>],
    family_of: &dyn Fn(&str) -> String,
    firsts: &BTreeSet<String>,
) -> Vec<Probe> {
    let all = variants();
    let mut used = BTreeSet::new();
    let mut out = Vec::new();
    for r in rows(RUNTIME, &["site", "form", "argv", "variant", "reason"]) {
        assert!(!r[0].trim().is_empty(), "{RUNTIME}: {r:?}: no site");
        assert!(!r[4].trim().is_empty(), "{RUNTIME}: {r:?}: no reason");
        let under: Vec<Option<&Variant>> = if r[3] == "-" {
            vec![None]
        } else if let Some(prefix) = r[3].strip_suffix('*') {
            let v: Vec<Option<&Variant>> = all
                .iter()
                .filter(|v| v.name.starts_with(prefix))
                .map(Some)
                .collect();
            assert!(
                !v.is_empty(),
                "{RUNTIME}: {r:?}: no variant starts {prefix:?}"
            );
            v
        } else {
            let v = all
                .iter()
                .find(|v| v.name == r[3])
                .unwrap_or_else(|| panic!("{RUNTIME}: {r:?}: no variant named {:?}", r[3]));
            vec![Some(v)]
        };
        // (name, argv) of each form the row runs.
        let targets: Vec<(String, Vec<String>)> = match r[2].as_str() {
            "*" | "*first" => {
                assert_eq!(r[1], "*", "{RUNTIME}: {r:?}: a sweep's form is *");
                assert!(r[3] != "-", "{RUNTIME}: {r:?}: a sweep needs a variant");
                let mut fams = BTreeSet::new();
                forms
                    .iter()
                    .filter(|f| f[2] == "-")
                    .map(|f| (f[0].clone(), words(if f[1] == "-" { &f[0] } else { &f[1] })))
                    .filter(|(_, a)| {
                        r[2] == "*"
                            || (!a
                                .iter()
                                .any(|w| matches!(w.as_str(), "help" | "--help" | "-h"))
                                && fams.insert(family_of(&a[0])))
                    })
                    .collect()
            }
            _ => {
                let argv = words(&r[2]);
                // A first word made with an escape (not UTF-8), or no
                // router form's word at the front, is the row's point; any
                // other is a router form's.
                assert!(
                    firsts.contains(&argv[0])
                        || argv[0].contains('\\')
                        || r[0].starts_with("front:"),
                    "{RUNTIME}: {r:?}: its first word is no router form's"
                );
                vec![(r[1].clone(), argv)]
            }
        };
        for v in under {
            if let Some(v) = v {
                used.insert(v.name.clone());
            }
            for (form, argv) in &targets {
                out.push(Probe {
                    class: "R".into(),
                    family: family_of(&argv[0]),
                    form: format!("{form} [{}]", v.map_or("as is", |v| v.name.as_str())),
                    argv: argv.clone(),
                    env: Vec::new(),
                    run_time: Some(RunTime {
                        site: r[0].clone(),
                        variant: v.cloned(),
                    }),
                });
            }
        }
    }
    let unused: Vec<&String> = all
        .iter()
        .map(|v| &v.name)
        .filter(|n| !used.contains(*n))
        .collect();
    assert!(
        unused.is_empty(),
        "{VARIANTS}: no row runs these: {unused:?}"
    );
    out
}

/// A scratch machine: home, K2 home with the owner token and the stub's
/// port, a project, and the `PATH` the binary runs with.
struct Machine {
    dir: PathBuf,
    home: PathBuf,
    k2home: PathBuf,
    proj: PathBuf,
    tmp: PathBuf,
    path: String,
    stub: Stub,
    /// The run-time machine's extras ([`stage_run_time`]).
    extra: Option<Extra>,
}

/// What the run-time machine has that the fixed one doesn't.
struct Extra {
    fx: PathBuf,
    /// A folder with a stand-in `python3` in it.
    py3: PathBuf,
    /// [`Machine::path`] with no `mktemp` on it.
    no_mktemp: String,
    beside: PathBuf,
    app_nobash: PathBuf,
    app_default: PathBuf,
    /// Folders made with a mode that stops their removal, to open again.
    shut: Vec<PathBuf>,
    /// Names the folders [`Special::CwdGone`] and `{fresh:…}` make.
    seq: AtomicUsize,
}

/// `tag` names it beside the others (`""` for the fixed machine).
fn scratch(tag: &str) -> Machine {
    let dir = std::env::temp_dir().join(format!("k5-native-bar-{}{tag}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    let home = dir.join("home");
    let k2home = k2_client::paths::k2_home_in(&home);
    let proj = dir.join("proj");
    let tmp = dir.join("tmp");
    let tools = dir.join("stand-ins");
    for d in [&k2home, &proj.join(".k2"), &tmp, &tools] {
        fs::create_dir_all(d).unwrap();
    }
    let stub = Stub::start(Path::new(&format!(
        "/tmp/k5nb-{}{tag}.sock",
        std::process::id()
    )));
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
        k2home,
        proj,
        tmp,
        path,
        stub,
        extra: None,
    }
}

fn mode(p: &Path, m: u32) {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(p, fs::Permissions::from_mode(m))
        .unwrap_or_else(|e| panic!("chmod {}: {e}", p.display()));
}

fn put(p: &Path, bytes: &[u8], m: u32) {
    if let Some(d) = p.parent() {
        fs::create_dir_all(d).unwrap_or_else(|e| panic!("mkdir {}: {e}", d.display()));
    }
    fs::write(p, bytes).unwrap_or_else(|e| panic!("write {}: {e}", p.display()));
    mode(p, m);
}

fn os(bytes: &[u8]) -> &std::ffi::OsStr {
    use std::os::unix::ffi::OsStrExt;
    std::ffi::OsStr::from_bytes(bytes)
}

/// The run-time machine: a scratch machine of its own, its shell CLI
/// cache blocked ([`BLOCKED`]), and the fixtures, folders and layouts
/// the variants and rows name.
fn stage_run_time() -> Machine {
    let mut b = scratch("-rt");
    let d = b.dir.clone();
    put(&b.k2home.join("run").join("cli"), BLOCKED.as_bytes(), 0o644);
    let fx = d.join("fx");
    put(&fx.join("nul.txt"), b"a\0b\n", 0o644);
    put(&fx.join("latin1.txt"), b"caf\xe9\n", 0o644);
    put(&fx.join("unreadable.txt"), b"x\n", 0o000);
    put(&fx.join("script.sh"), b"#!/bin/sh\nexit 0\n", 0o755);
    put(&fx.join("notexec.txt"), b"x\n", 0o644);
    put(
        &fx.join("elf-not"),
        b"plain text, no #! and no binary header\n",
        0o755,
    );
    put(&fx.join("shot").join("data.json"), b"{}\n", 0o644);
    put(
        &fx.join("shot").join("blobs").join(os(b"\xff.png")),
        b"png\n",
        0o644,
    );
    let names: [&[u8]; 5] = [b"ws", b"-ws", b"nl\nws", b"caf\xe9", b"noread"];
    for name in names {
        fs::create_dir_all(fx.join(os(name)).join(".k2")).unwrap();
    }
    fs::create_dir_all(fx.join("noenter")).unwrap();
    mode(&fx.join("noenter"), 0o000);
    mode(&fx.join("noread"), 0o100);
    for w in ["ws", "w1"] {
        fs::create_dir_all(d.join("cdpath").join(w).join(".k2")).unwrap();
    }
    // A link to ws/.k2 (fx itself has no .k2), and the home under a name
    // that is not UTF-8.
    std::os::unix::fs::symlink("ws/.k2", fx.join("lnk")).unwrap();
    std::os::unix::fs::symlink(&b.home, d.join(os(b"home\xff"))).unwrap();
    // Prepared K2 homes, PATH folders and projects, each copied fresh for
    // a probe that names it as `{fresh:<name>}` (a native run may change
    // its home).
    let stub_sh = b"#!/bin/sh\nexit 1\n";
    put(&fx.join("bin-fed").join("openssl"), stub_sh, 0o755);
    put(&fx.join("bin-fed-shasum").join("openssl"), stub_sh, 0o755);
    put(&fx.join("bin-fed-shasum").join("shasum"), stub_sh, 0o755);
    put(
        &fx.join("fedhome").join("tunnel-key.pem"),
        b"-----BEGIN PRIVATE KEY-----\n",
        0o600,
    );
    put(
        &fx.join("fedhome").join("heartbeat.token"),
        b"owner-tok-7171\n",
        0o644,
    );
    let port = format!("{}\n", b.stub.port);
    put(
        &fx.join("k2home-token-space").join("heartbeat.port"),
        port.as_bytes(),
        0o644,
    );
    let homes: [(&str, &str, &[u8]); 26] = [
        ("k2home-port-bad", "heartbeat.port", b"70000\n"),
        ("k2home-port-bad", "heartbeat.token", b"owner-tok-7171\n"),
        ("k2home-connect", "heartbeat.token", b"owner-tok-7171\n"),
        (
            "k2home-connect",
            "tunnel.json",
            b"{\"token\": \"tun-tok-1\"}",
        ),
        (
            "k2home-connect",
            "connect-account.json",
            b"{\"access_token\": \"acc-tok-1\"}",
        ),
        ("k2home-token-space", "heartbeat.token", b"owner tok-7171"),
        ("home-port-bad", ".k2/heartbeat.port", b"70000\n"),
        ("home-port-nul", ".k2/heartbeat.port", b"70\x00000"),
        ("home-c-port-bad", ".k2/heartbeat.port", b"abc\n"),
        ("home-c-port-bad", ".k2/heartbeat.token", b"owner-tok-7171"),
        ("home-c-port-nul", ".k2/heartbeat.port", b"x\x00y\n"),
        ("home-c-port-nul", ".k2/heartbeat.token", b"owner-tok-7171"),
        ("home-c-port-bad-token-nul", ".k2/heartbeat.port", b"abc\n"),
        (
            "home-c-port-bad-token-nul",
            ".k2/heartbeat.token",
            b"owner\x00tok\n",
        ),
        ("home-c-port-bad-token-dir", ".k2/heartbeat.port", b"abc\n"),
        ("home-hb-port-latin1", "heartbeat.port", b"80\xe9\n"),
        ("home-hb-token-latin1", "heartbeat.token", b"tok\xe9\n"),
        ("home-hb-port-big", "heartbeat.port", b"99999\n"),
        (
            "home-connect-account-latin1",
            "connect-account.json",
            b"{\"email\":\"caf\xe9\"}",
        ),
        (
            "home-connect-daemon-port",
            "heartbeat.token",
            b"owner-tok-7171\n",
        ),
        ("home-connect-daemon-port", "daemon.port", b"80x\n"),
        (
            "home-connect-logout-ipv6",
            "settings.json",
            b"{\"connectDbUrl\":\"http://[::1]:9\"}",
        ),
        (
            "home-connect-logout-ipv6",
            "connect-account.json",
            b"{\"access_token\":\"t1\"}",
        ),
        ("home-remote-tokens-nul", "connect-tokens.json", b"{\x00}"),
        (
            "home-remote-hosts-type",
            "connect-hosts.json",
            b"[{\"hostname\":1}]",
        ),
        ("sess-subnoread", ".k2/sessions/s1/archive.ndjson", b"{}\n"),
    ];
    for (dir, file, bytes) in homes {
        put(&fx.join(dir).join(file), bytes, 0o644);
    }
    // The old home is the brand's legacy name (`.k2so` with ours; a brand
    // with no alias has its command's name), as the binary looks for it.
    let legacy = format!(".{}", k2_client::cli_install::LEGACY_NAME);
    put(&fx.join("oldhome").join(legacy).join("connect-tokens.json"), b"[]", 0o644);
    fs::create_dir_all(fx.join("home-c-port-bad-token-dir/.k2/heartbeat.token")).unwrap();
    fs::create_dir_all(fx.join("home-empty/.k2")).unwrap();
    // A PATH with cat on it and nothing else (no ps, no python3).
    let cat = std::env::split_paths(&b.path)
        .map(|d| d.join("cat"))
        .find(|c| c.is_file())
        .expect("a cat on PATH");
    fs::create_dir_all(fx.join("bin-cat")).unwrap();
    std::os::unix::fs::symlink(&cat, fx.join("bin-cat").join("cat")).unwrap();
    let deep = format!("{}{}", "[".repeat(501), "]".repeat(501));
    put(
        &fx.join("home-connect-tunnel-deep").join("tunnel.json"),
        deep.as_bytes(),
        0o644,
    );
    fs::create_dir_all(fx.join("sess-latin1/.k2/sessions").join(os(b"caf\xe9"))).unwrap();
    fs::create_dir_all(fx.join("sess-noread/.k2/sessions/s1")).unwrap();
    mode(&fx.join("sess-noread/.k2/sessions"), 0o100);
    mode(&fx.join("sess-subnoread/.k2/sessions/s1"), 0o100);
    put(&d.join("bashenv"), b":\n", 0o644);
    let py3 = d.join("py3");
    put(&py3.join("python3"), b"#!/bin/sh\nexit 1\n", 0o755);
    let no_mktemp = path_without(&b.path, "mktemp", &d.join("no-mktemp"));
    // The binary, copied where an install puts it: links are resolved
    // before the layout is read, so a link would not do.
    let runner =
        fs::read(root().join("cli/k2-widget-shot.mjs")).expect("read cli/k2-widget-shot.mjs");
    let bin = fs::read(BIN).unwrap_or_else(|e| panic!("read {BIN}: {e}"));
    let beside = d.join("beside").join("k5-cli");
    put(&beside, &bin, 0o755);
    put(&d.join("beside").join("k2-widget-shot.mjs"), &runner, 0o644);
    let app = |kind: &str, with_cli: bool| -> PathBuf {
        let contents = d.join(kind).join("Globex.app").join("Contents");
        let exe = contents.join("MacOS").join("k5-cli");
        put(&exe, &bin, 0o755);
        let up = contents.join("Resources").join("_up_").join("cli");
        put(&up.join("k2-widget-shot.mjs"), &runner, 0o644);
        if with_cli {
            put(
                &up.join(k2_client::cli_install::CLI_NAME),
                b"#!/bin/bash\nexit 1\n",
                0o755,
            );
        }
        exe
    };
    let app_nobash = app("nobash", false);
    let app_default = app("default", true);
    b.extra = Some(Extra {
        shut: vec![
            fx.join("noenter"),
            fx.join("noread"),
            fx.join("sess-noread/.k2/sessions"),
            fx.join("sess-subnoread/.k2/sessions/s1"),
        ],
        fx,
        py3,
        no_mktemp,
        beside,
        app_nobash,
        app_default,
        seq: AtomicUsize::new(0),
    });
    b
}

/// `path` with `name` taken off it: each folder that has one is a
/// folder of links under `shadows` to the rest.
fn path_without(path: &str, name: &str, shadows: &Path) -> String {
    let mut dirs = Vec::new();
    for (i, dir) in std::env::split_paths(path).enumerate() {
        if fs::symlink_metadata(dir.join(name)).is_err() {
            dirs.push(dir);
            continue;
        }
        let shadow = shadows.join(i.to_string());
        fs::create_dir_all(&shadow).unwrap();
        for entry in fs::read_dir(&dir).unwrap_or_else(|e| panic!("read {}: {e}", dir.display())) {
            let n = entry
                .unwrap_or_else(|e| panic!("read {}: {e}", dir.display()))
                .file_name();
            if n != name {
                std::os::unix::fs::symlink(dir.join(&n), shadow.join(&n)).unwrap();
            }
        }
        dirs.push(shadow);
    }
    std::env::join_paths(dirs)
        .expect("the folders join")
        .into_string()
        .expect("a UTF-8 PATH")
}

impl Machine {
    /// A template's bytes: `{dir}`, `{home}`, `{k2home}`, `{proj}`,
    /// `{tmp}`, `{fx}`, `{sock}` and `{port}` filled in, `{fresh:<name>}`
    /// a copy of `{fx}/<name>` made for this probe ([`Machine::fresh`]),
    /// any other `{` itself; and the escapes `\s` (space), `\t`, `\n`, `\\` and
    /// `\xNN` (one byte) read.
    fn fill(&self, t: &str) -> std::ffi::OsString {
        use std::os::unix::ffi::OsStringExt;
        let fx = self
            .extra
            .as_ref()
            .map(|e| e.fx.clone())
            .unwrap_or_default();
        let place = |name: &str| -> Option<String> {
            let p: &Path = match name {
                "dir" => &self.dir,
                "home" => &self.home,
                "k2home" => &self.k2home,
                "proj" => &self.proj,
                "tmp" => &self.tmp,
                "fx" => &fx,
                "sock" => &self.stub.sock,
                "port" => return Some(self.stub.port.to_string()),
                _ => return name.strip_prefix("fresh:").map(|n| self.fresh(n)),
            };
            Some(p.to_str().expect("a UTF-8 scratch path").to_string())
        };
        let mut out = Vec::new();
        let b = t.as_bytes();
        let mut i = 0;
        while i < b.len() {
            match b[i] {
                b'\\' => {
                    let (byte, len) = match b.get(i + 1) {
                        Some(b's') => (b' ', 2),
                        Some(b't') => (b'\t', 2),
                        Some(b'n') => (b'\n', 2),
                        Some(b'\\') => (b'\\', 2),
                        Some(b'x') => {
                            let hex = t
                                .get(i + 2..i + 4)
                                .unwrap_or_else(|| panic!("{t:?}: a short \\x"));
                            let v = u8::from_str_radix(hex, 16)
                                .unwrap_or_else(|_| panic!("{t:?}: \\x{hex}"));
                            (v, 4)
                        }
                        _ => panic!("{t:?}: an escape that is not \\s, \\t, \\n, \\\\ or \\xNN"),
                    };
                    out.push(byte);
                    i += len;
                }
                b'{' => match t[i..]
                    .find('}')
                    .and_then(|end| Some((end, place(&t[i + 1..i + end])?)))
                {
                    Some((end, filled)) => {
                        out.extend_from_slice(filled.as_bytes());
                        i += end + 1;
                    }
                    None => {
                        out.push(b'{');
                        i += 1;
                    }
                },
                c => {
                    out.push(c);
                    i += 1;
                }
            }
        }
        std::ffi::OsString::from_vec(out)
    }
}

impl Machine {
    /// A fresh copy of `{fx}/<name>` for one probe: its path.
    fn fresh(&self, name: &str) -> String {
        let e = self.extra.as_ref().expect("the run-time machine");
        let from = e.fx.join(name);
        assert!(
            from.is_dir(),
            "{{fresh:{name}}}: no folder {}",
            from.display()
        );
        let to = self
            .dir
            .join("fresh")
            .join(e.seq.fetch_add(1, Ordering::SeqCst).to_string())
            .join(name);
        copy_tree(&from, &to);
        to.to_str().expect("a UTF-8 scratch path").to_string()
    }
}

/// `from`, copied to `to` with its modes (links as links).
fn copy_tree(from: &Path, to: &Path) {
    use std::os::unix::fs::PermissionsExt;
    fs::create_dir_all(to).unwrap_or_else(|e| panic!("mkdir {}: {e}", to.display()));
    for entry in fs::read_dir(from).unwrap_or_else(|e| panic!("read {}: {e}", from.display())) {
        let entry = entry.unwrap_or_else(|e| panic!("read {}: {e}", from.display()));
        let (src, dst) = (entry.path(), to.join(entry.file_name()));
        let meta =
            fs::symlink_metadata(&src).unwrap_or_else(|e| panic!("stat {}: {e}", src.display()));
        if meta.is_dir() {
            copy_tree(&src, &dst);
        } else if meta.file_type().is_symlink() {
            std::os::unix::fs::symlink(fs::read_link(&src).unwrap(), &dst).unwrap();
            continue;
        } else {
            fs::copy(&src, &dst).unwrap_or_else(|e| panic!("copy {}: {e}", src.display()));
        }
        mode(&dst, meta.permissions().mode() & 0o7777);
    }
}

/// A pseudo-terminal: its controller and the terminal end, opened with
/// `O_NOCTTY` (so it is no one's controlling terminal).
fn pty() -> (fs::File, fs::File) {
    use std::os::fd::FromRawFd;
    static NAMES: Mutex<()> = Mutex::new(());
    // SAFETY: plain libc calls on descriptors this function owns; the
    // name ptsname returns is copied under a lock before anything else
    // can call it.
    unsafe {
        let m = libc::posix_openpt(libc::O_RDWR | libc::O_NOCTTY);
        assert!(m >= 0, "posix_openpt: {}", std::io::Error::last_os_error());
        let controller = fs::File::from_raw_fd(m);
        let flags = libc::fcntl(m, libc::F_GETFL);
        assert!(
            flags >= 0 && libc::fcntl(m, libc::F_SETFL, flags | libc::O_NONBLOCK) == 0,
            "fcntl O_NONBLOCK"
        );
        assert_eq!(
            libc::grantpt(m),
            0,
            "grantpt: {}",
            std::io::Error::last_os_error()
        );
        assert_eq!(
            libc::unlockpt(m),
            0,
            "unlockpt: {}",
            std::io::Error::last_os_error()
        );
        let name = {
            let _held = NAMES.lock().expect("the ptsname lock");
            let p = libc::ptsname(m);
            assert!(!p.is_null(), "ptsname: {}", std::io::Error::last_os_error());
            std::ffi::CStr::from_ptr(p).to_owned()
        };
        let s = libc::open(name.as_ptr(), libc::O_RDWR | libc::O_NOCTTY);
        assert!(s >= 0, "open {name:?}: {}", std::io::Error::last_os_error());
        (controller, fs::File::from_raw_fd(s))
    }
}

/// Runs `p` with `env` on top of its own: its exit (`None`: still
/// running at [`DEADLINE`], killed) and its stderr. A run-time probe
/// gets its variant too ([`Machine::fill`] for its values and argv).
fn spawn(b: &Machine, p: &Probe, env: &[(String, String)]) -> (Option<ExitStatus>, String) {
    let rt = p.run_time.as_ref();
    assert_eq!(
        rt.is_some(),
        b.extra.is_some(),
        "{}: a run-time probe runs on the run-time machine, and nothing else does",
        p.form
    );
    let variant = rt.and_then(|r| r.variant.as_ref());
    let has = |s: &Special| rt.is_some_and(|r| r.has(s));
    let extra = || {
        b.extra
            .as_ref()
            .expect("a run-time probe on the run-time machine")
    };
    let program: &Path = if has(&Special::BesideRunner) {
        &extra().beside
    } else if has(&Special::AppNoBash) {
        &extra().app_nobash
    } else if has(&Special::AppDefault) {
        &extra().app_default
    } else {
        Path::new(BIN)
    };
    let mut path = if has(&Special::NoMktemp) {
        extra().no_mktemp.clone()
    } else {
        b.path.clone()
    };
    if has(&Special::Python3) {
        path = format!("{}:{path}", extra().py3.display());
    }
    let mut cmd = Command::new(program);
    cmd.env_clear()
        .env("PATH", &path)
        .env("HOME", &b.home)
        .env("TMPDIR", &b.tmp)
        .envs(p.env.iter().map(|(k, v)| (k.as_str(), v.as_str())))
        .envs(env.iter().map(|(k, v)| (k.as_str(), v.as_str())))
        .current_dir(&b.proj)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    let mut controller = None;
    if rt.is_some() {
        cmd.args(p.argv.iter().map(|w| b.fill(w)));
        // A session of its own: no controlling terminal, whatever the
        // test runner has, and a group of its own to kill.
        // SAFETY: setsid only.
        unsafe {
            cmd.pre_exec(|| {
                if libc::setsid() < 0 {
                    Err(std::io::Error::last_os_error())
                } else {
                    Ok(())
                }
            });
        }
        if has(&Special::Tty) || has(&Special::TtyStdout) {
            let (c, t) = pty();
            if has(&Special::TtyStdout) {
                cmd.stdout(Stdio::from(
                    t.try_clone().expect("a second terminal descriptor"),
                ));
            }
            if has(&Special::Tty) {
                cmd.stdin(Stdio::from(t));
            }
            controller = Some(c);
        }
    } else {
        cmd.args(&p.argv).process_group(0);
    }
    if let Some(v) = variant {
        for (k, val) in &v.set {
            cmd.env(k, b.fill(val));
        }
        for k in &v.unset {
            cmd.env_remove(k);
        }
        for s in &v.special {
            match s {
                Special::Cwd(t) => {
                    cmd.current_dir(b.fill(t));
                }
                Special::CwdGone => {
                    let n = extra().seq.fetch_add(1, Ordering::SeqCst);
                    let gone = b.dir.join("gone").join(n.to_string());
                    fs::create_dir_all(&gone).unwrap();
                    let c = std::ffi::CString::new(gone.to_str().expect("a UTF-8 path"))
                        .expect("no NUL in a scratch path");
                    cmd.current_dir(&gone);
                    // SAFETY: rmdir only, on a name made before the fork.
                    unsafe {
                        cmd.pre_exec(move || {
                            if libc::rmdir(c.as_ptr()) == 0 {
                                Ok(())
                            } else {
                                Err(std::io::Error::last_os_error())
                            }
                        });
                    }
                }
                Special::Append(w) => {
                    cmd.arg(b.fill(w));
                }
                Special::Tty
                | Special::TtyStdout
                | Special::Python3
                | Special::NoMktemp
                | Special::BesideRunner
                | Special::AppNoBash
                | Special::AppDefault => {}
            }
        }
    }
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
        // What it writes to its terminal is read and dropped, so it never
        // fills (the controller doesn't block).
        if let Some(c) = controller.as_mut() {
            let mut buf = [0u8; 4096];
            while matches!(std::io::Read::read(c, &mut buf), Ok(n) if n > 0) {}
        }
        std::thread::sleep(Duration::from_millis(5));
    };
    // A probe's terminal stays open until it is done.
    drop(controller);
    let stderr = String::from_utf8_lossy(&reader.join().expect("its stderr")).into_owned();
    (status, stderr)
}

/// Whether `p` falls through: the hand-off's exit and line, the
/// missing-Python one (69) in a build with the shell CLI, the no-shell
/// one (70) in a build with none. Neither build prints the other's.
fn falls_through(b: &Machine, p: &Probe) -> bool {
    let (status, stderr) = spawn(b, p, &[]);
    let Some(status) = status else {
        // Still in its own code at the deadline: native.
        return false;
    };
    let no_python = stderr.contains(NO_PYTHON);
    let no_shell = stderr.contains(NO_SHELL_CLI);
    assert!(
        !no_python || status.code() == Some(69),
        "{:?}: the missing-Python line with exit {:?}:\n{stderr}",
        p.argv,
        status.code()
    );
    assert!(
        !no_shell || status.code() == Some(70),
        "{:?}: the no-shell-CLI line with exit {:?}:\n{stderr}",
        p.argv,
        status.code()
    );
    let (ours, theirs) = if k2_client::brand::CLI_NO_BASH {
        (no_shell, no_python)
    } else {
        (no_python, no_shell)
    };
    assert!(
        !theirs,
        "{:?}: the other build's hand-off line (this build: no_bash = {}):\n{stderr}",
        p.argv,
        k2_client::brand::CLI_NO_BASH
    );
    ours
}

/// Whether each of `probes` falls through by `judge`, [`WORKERS`] at a
/// time.
fn in_parallel(probes: &[&Probe], judge: impl Fn(&Probe) -> bool + Sync) -> Vec<bool> {
    let next = AtomicUsize::new(0);
    let fell = Mutex::new(vec![false; probes.len()]);
    std::thread::scope(|s| {
        for _ in 0..WORKERS {
            s.spawn(|| loop {
                let i = next.fetch_add(1, Ordering::SeqCst);
                let Some(&p) = probes.get(i) else { break };
                let f = judge(p);
                fell.lock().expect("the results")[i] = f;
            });
        }
    });
    fell.into_inner().expect("the results")
}

/// The probes that fall through, in probe order: the [`PROXIED`] ones
/// after the others, one at a time, then the run-time ones on their own
/// machine ([`run_time`]).
fn run(probes: &[Probe]) -> Vec<Probe> {
    let (timed, fixed): (Vec<&Probe>, Vec<&Probe>) =
        probes.iter().partition(|p| p.run_time.is_some());
    let b = scratch("");
    let (plain, proxy): (Vec<&Probe>, Vec<&Probe>) =
        fixed.into_iter().partition(|p| !is_proxied(p));
    let fell = in_parallel(&plain, |p| falls_through(&b, p));
    let pki = tunnel::Pki::for_names(&b.dir.join("pki"), &["0.0.0.0"]);
    let proxy_fell: Vec<bool> = proxy
        .iter()
        .map(|p| proxied_falls_through(&b, &pki, p))
        .collect();
    let _ = fs::remove_dir_all(&b.dir);
    let mut out: Vec<Probe> = plain
        .into_iter()
        .zip(fell)
        .chain(proxy.into_iter().zip(proxy_fell))
        .filter(|(_, f)| *f)
        .map(|(p, _)| p.clone())
        .collect();
    out.extend(run_time(&timed));
    out
}

/// The run-time probes that fall through, on the run-time machine
/// ([`stage_run_time`]). Fails if the shell CLI's blocked cache is not
/// as it was made (something got past it). Writes the report when
/// `NATIVE_BAR_RUNTIME_REPORT` names a file.
fn run_time(probes: &[&Probe]) -> Vec<Probe> {
    if probes.is_empty() {
        return Vec::new();
    }
    let b = stage_run_time();
    let fell = in_parallel(probes, |p| run_time_falls_through(&b, p));
    let cache = b.k2home.join("run").join("cli");
    assert_eq!(
        fs::read_to_string(&cache).ok().as_deref(),
        Some(BLOCKED),
        "{}: the run-time machine's blocked shell CLI cache changed",
        cache.display()
    );
    if let Ok(to) = std::env::var("NATIVE_BAR_RUNTIME_REPORT") {
        let mut s = String::from("site\tvariant\tgroup\tpartner\tfamily\tform\tfell\n");
        for (p, f) in probes.iter().zip(&fell) {
            let rt = p.run_time.as_ref().expect("a run-time probe");
            let (name, group, partner) = match &rt.variant {
                Some(v) => (
                    v.name.as_str(),
                    v.group.as_str(),
                    if v.partner { "yes" } else { "no" },
                ),
                None => ("-", "-", "no"),
            };
            s.push_str(&format!(
                "{}\t{name}\t{group}\t{partner}\t{}\t{}\t{}\n",
                rt.site,
                p.family,
                p.form,
                if *f { "yes" } else { "no" }
            ));
        }
        fs::write(&to, s).unwrap_or_else(|e| panic!("write {to}: {e}"));
    }
    if let Some(e) = &b.extra {
        for d in &e.shut {
            mode(d, 0o755);
        }
    }
    let _ = fs::remove_dir_all(&b.dir);
    probes
        .iter()
        .zip(fell)
        .filter(|(_, f)| *f)
        .map(|(p, _)| (*p).clone())
        .collect()
}

/// Whether a run-time probe falls through: as [`falls_through`], or,
/// under a variant whose shell has `python3` (past the Python 3 check),
/// it stopped at the blocked cache with that line and exit 1.
fn run_time_falls_through(b: &Machine, p: &Probe) -> bool {
    if falls_through(b, p) {
        return true;
    }
    let rt = p.run_time.as_ref().expect("a run-time probe");
    if !rt.has(&Special::Python3) {
        return false;
    }
    let (status, stderr) = spawn(b, p, &[]);
    let blocked = stderr.contains(CACHE_BLOCKED);
    assert!(
        !blocked || status.and_then(|s| s.code()) == Some(1),
        "{:?}: the blocked cache's line with exit {:?}:\n{stderr}",
        p.form,
        status.map(|s| s.code())
    );
    blocked
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
    let by: Vec<String> = ["V", "H", "F", "E", "P", "R"]
        .iter()
        .map(|c| format!("{c} {}", counts.get(c).copied().unwrap_or(0)))
        .collect();
    let mut s = String::from(
        "# GENERATED by crates/k5-cli/tests/native_bar.rs (NATIVE_BAR_WRITE=<file>): the probes\n\
         # k5 still hands to bash. It may only fall; the ship bar is no row of a counted class (V verb,\n\
         # H help, F flag, E edge; P listed, not counted; R run-time: counted for a no-bash brand).\n",
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
    // The allowance's rows are taken out before the ceiling is compared or
    // written; each one must still fall through and name a probe.
    let allowed: BTreeSet<String> = read(ALLOWANCE)
        .lines()
        .filter_map(|l| l.strip_prefix("native_bar\t"))
        .map(str::to_string)
        .collect();
    assert!(
        read(ALLOWANCE).contains("\n# due: native port due before the n2 build (0.46.19)\n"),
        "{ALLOWANCE} carries its due line"
    );
    let row = |p: &Probe| format!("{}\t{}\t{}", p.class, p.family, p.form);
    let fell_allowed: BTreeSet<String> = fell.iter().map(row).filter(|r| allowed.contains(r)).collect();
    let fell: Vec<Probe> = fell.into_iter().filter(|p| !allowed.contains(&row(p))).collect();
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
    let allowed_native: Vec<&String> = allowed
        .iter()
        .filter(|r| names.contains(*r) && !fell_allowed.contains(*r))
        .collect();
    let allowed_stray: Vec<&String> = allowed.iter().filter(|r| !names.contains(*r)).collect();
    if !allowed_native.is_empty() {
        problems += &format!(
            "these are native now: take them out of {ALLOWANCE} ({}):\n  {}\n",
            allowed_native.len(),
            listing(&allowed_native)
        );
    }
    if !allowed_stray.is_empty() {
        problems += &format!(
            "these allowance rows name no probe ({}):\n  {}\n",
            allowed_stray.len(),
            listing(&allowed_stray)
        );
    }
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
    println!("plus {} on the named allowance {ALLOWANCE}", fell_allowed.len());
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
