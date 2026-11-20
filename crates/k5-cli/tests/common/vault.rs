//! The rig the vault's parity tests run on: `cli/k2 vault …` against
//! `k5-cli vault …` (and the binary again with no `python3` on `PATH`),
//! each against two logging stubs of its own: the daemon (TCP, and its
//! socket as the session's cell socket) and the vault socket at
//! `<K2 home>/vault.sock`, the person's channel. Each side gets the same
//! environment, the same stdin and the same answers, and must send the
//! same requests to each stub and print the same stdout and stderr with
//! the same exit code. The masked differences are [`super::parity`]'s
//! ([`super::parity::normalise`]).
//!
//! What [`super::parity::Rig`] has no room for, and this does: bytes on
//! stdin (a value for `set`, a password for a confirm), and the vault
//! socket. Its K2 home sits under a short `/tmp` folder, as a socket's
//! path must be short.
//!
//! No secret reaches a child: the binary runs with a folder of spies
//! first on `PATH`, one for each program it could start (`python3`,
//! `curl`, `bash`, `sh`, `env`), each writing its argv and environment to
//! a log before it runs the real one. A case's [`VCase::secrets`] must be
//! in none of it: the proof `tests/cli/secrets_off_child_argv_env.sh`
//! gives the shell's side.

#![cfg(unix)]

use super::parity::{cli_k2, normalise, Out, BIN, OWNER};
use super::stub::{Reply, Stub};
use std::ffi::OsString;
use std::io::Write;
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};

static N: AtomicUsize = AtomicUsize::new(0);

/// The programs a spy stands in for.
pub const SPIED: &[&str] = &["python3", "curl", "bash", "sh", "env", "cat"];

/// How a case's outputs compare.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VCompare {
    /// Byte for byte.
    Exact,
    /// The shell's Python raises: stderr matches up to the traceback and
    /// its last line's exception name (see [`super::parity::Compare::Traceback`]).
    Traceback,
}

pub struct VCase {
    pub name: &'static str,
    /// The command's first word: `vault`, unless a family that asks for
    /// the owner's confirm (`api-key`) or hands a secret to the vault
    /// (`msg --secret`) runs on this rig too.
    pub noun: &'static str,
    /// The words after [`VCase::noun`].
    pub args: Vec<&'static str>,
    /// `{port}` is the side's daemon stub's port, `{sock}` its socket (a
    /// session's cell socket), `{vsock}` the vault socket's path.
    pub env: Vec<(&'static str, String)>,
    pub stdin: Vec<u8>,
    /// The daemon stub's answers.
    pub daemon: Vec<Reply>,
    /// The vault socket's answers. `None`: there is no vault socket.
    pub vault: Option<Vec<Reply>>,
    /// The owner token and the port in the K2 home's files.
    pub owner_on_disk: bool,
    /// Bash's exit code and how many requests each stub must get: the case
    /// proves what it says.
    pub code: i32,
    pub daemon_requests: usize,
    pub vault_requests: usize,
    pub compare: VCompare,
    /// Values no child of the binary may see on argv or in its
    /// environment.
    pub secrets: Vec<&'static str>,
    /// The binary hands this line to the shell, so with no `python3` it
    /// prints the missing-Python line instead: that side isn't run.
    pub handed: bool,
    /// Files the verb writes: each is removed before every side runs, and
    /// what each side left there (its bytes and mode, or none) must match.
    pub files: Vec<PathBuf>,
}

impl VCase {
    /// A case with the owner on disk, a vault socket and nothing else.
    pub fn new(name: &'static str, args: &[&'static str]) -> VCase {
        VCase {
            name,
            noun: "vault",
            args: args.to_vec(),
            env: Vec::new(),
            stdin: Vec::new(),
            daemon: Vec::new(),
            vault: Some(Vec::new()),
            owner_on_disk: true,
            code: 0,
            daemon_requests: 0,
            vault_requests: 0,
            compare: VCompare::Exact,
            secrets: Vec::new(),
            handed: false,
            files: Vec::new(),
        }
    }
}

/// One side's run: its output and each stub's normalised requests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ran {
    pub out: Out,
    pub daemon: Vec<String>,
    pub vault: Vec<String>,
    /// [`VCase::files`] after the run: bytes and permission bits.
    pub files: Vec<Option<(Vec<u8>, u32)>>,
}

pub struct VRig {
    pub dir: PathBuf,
    pub home: PathBuf,
    pub k2home: PathBuf,
    pub proj: PathBuf,
    pub spies: PathBuf,
    pub spy_log: PathBuf,
    no_python3: String,
}

impl VRig {
    pub fn new() -> VRig {
        let n = N.fetch_add(1, Ordering::SeqCst);
        let dir = PathBuf::from(format!("/tmp/k5v{}-{n}", std::process::id()));
        if dir.exists() {
            std::fs::remove_dir_all(&dir).unwrap();
        }
        let home = dir.join("h");
        let k2home = k2_client::paths::k2_home_in(&home);
        let proj = dir.join("p");
        let spies = dir.join("spy");
        let spies_np = dir.join("spy-np");
        for d in [
            &k2home,
            &proj.join(".k2"),
            &spies,
            &spies_np,
            &dir.join("t"),
        ] {
            std::fs::create_dir_all(d).unwrap();
        }
        let spy_log = dir.join("spy.log");
        // The second folder has no python3 spy, for the run with none.
        for (folder, progs) in [(&spies, SPIED), (&spies_np, &SPIED[1..])] {
            for prog in progs {
                let script = format!(
                    "#!/bin/sh\n{{ printf 'argv:'; printf ' %s' \"$0\" \"$@\"; printf '\\n'; /usr/bin/env; }} >> '{}'\nPATH=\"${{PATH#{}:}}\" exec {prog} \"$@\"\n",
                    spy_log.display(),
                    folder.display()
                );
                let p = folder.join(prog);
                std::fs::write(&p, script).unwrap();
                std::fs::set_permissions(&p, std::os::unix::fs::PermissionsExt::from_mode(0o755))
                    .unwrap();
            }
        }
        let no_python3 = super::path_without_python3(&dir);
        VRig {
            dir,
            home,
            k2home,
            proj,
            spies,
            spy_log,
            no_python3,
        }
    }

    /// One side: `bash` runs `cli/k2`; else the binary, with the spies
    /// first on `PATH` (and no `python3` after them when `no_python3`).
    pub fn run(&self, bash: bool, no_python3: bool, c: &VCase) -> Ran {
        let tag = match (bash, no_python3) {
            (true, _) => "b",
            (false, false) => "r",
            (false, true) => "n",
        };
        let n = N.fetch_add(1, Ordering::SeqCst);
        let daemon = Stub::start(&PathBuf::from(format!(
            "/tmp/k5v{tag}-{}-{n}.sock",
            std::process::id()
        )));
        let vsock = self.k2home.join("vault.sock");
        let _ = std::fs::remove_file(&vsock);
        let vault = c.vault.as_ref().map(|replies| {
            let s = Stub::start(&vsock);
            s.reset(replies);
            s
        });
        daemon.reset(&c.daemon);
        for f in ["heartbeat.port", "heartbeat.token", "daemon.token"] {
            let _ = std::fs::remove_file(self.k2home.join(f));
        }
        let _ = std::fs::remove_dir_all(self.k2home.join("run"));
        if c.owner_on_disk {
            std::fs::write(self.k2home.join("heartbeat.token"), format!("{OWNER}\n")).unwrap();
            std::fs::write(
                self.k2home.join("heartbeat.port"),
                format!("{}\n", daemon.port),
            )
            .unwrap();
        }
        let _ = std::fs::remove_file(&self.spy_log);
        for f in &c.files {
            let _ = std::fs::remove_file(f);
        }
        let fill = |v: &str| {
            v.replace("{port}", &daemon.port.to_string())
                .replace("{sock}", &daemon.sock.to_string_lossy())
                .replace("{vsock}", &vsock.to_string_lossy())
        };
        let env: Vec<(String, String)> = c
            .env
            .iter()
            .map(|(k, v)| (k.to_string(), fill(v)))
            .collect();
        let base = if no_python3 {
            OsString::from(&self.no_python3)
        } else {
            std::env::var_os("PATH").expect("a PATH")
        };
        let mut cmd = if bash {
            let mut cmd = Command::new("bash");
            cmd.env_clear();
            cmd.arg(cli_k2());
            cmd.env("PATH", &base);
            cmd.env("K5_CLI_VIA", "rust");
            let lookup = |name: &str| {
                env.iter()
                    .find(|(k, _)| k == name)
                    .map(|(_, v)| OsString::from(v))
            };
            let args: Vec<OsString> = std::iter::once(c.noun)
                .chain(c.args.iter().copied())
                .map(OsString::from)
                .collect();
            let verdict = k5_cli::passport::decide(
                k5_cli::passport::facts(&args, lookup),
                k5_cli::passport::live_session_ancestor,
            );
            if verdict.would_refuse() {
                cmd.env("K5_CLI_WOULD_REFUSE", "1");
            }
            cmd
        } else {
            let spies = if no_python3 {
                self.dir.join("spy-np")
            } else {
                self.spies.clone()
            };
            let mut path = spies.into_os_string();
            path.push(":");
            path.push(&base);
            let mut cmd = Command::new(BIN);
            cmd.env_clear();
            cmd.env("PATH", path);
            cmd
        };
        cmd.env("HOME", &self.home)
            .env("TMPDIR", self.dir.join("t"))
            .current_dir(&self.proj)
            .envs(env.iter().map(|(k, v)| (k.as_str(), v.as_str())))
            .arg(c.noun)
            .args(&c.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = cmd.spawn().unwrap_or_else(|e| panic!("run {cmd:?}: {e}"));
        {
            let mut stdin = child.stdin.take().expect("stdin");
            // A side that doesn't read it all closes the pipe early.
            let _ = stdin.write_all(&c.stdin);
        }
        let o = child.wait_with_output().expect("wait");
        let signal = o.status.signal();
        let out = Out {
            code: o.status.code().or_else(|| signal.map(|s| 128 + s)),
            signal,
            stdout: String::from_utf8_lossy(&o.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&o.stderr).into_owned(),
        };
        let native = !bash;
        let norm = |log: Vec<super::stub::Logged>, port: u16| -> Vec<String> {
            log.iter()
                .map(|r| {
                    normalise(r, native, c.noun, c.name)
                        .replace(&format!("127.0.0.1:{port}"), "127.0.0.1:<port>")
                })
                .collect()
        };
        let files = c
            .files
            .iter()
            .map(|f| {
                use std::os::unix::fs::PermissionsExt;
                let meta = std::fs::metadata(f).ok()?;
                let bytes = std::fs::read(f).expect("read a file the verb wrote");
                Some((bytes, meta.permissions().mode() & 0o777))
            })
            .collect();
        Ran {
            out,
            daemon: norm(daemon.take_log(), daemon.port),
            vault: vault.map_or_else(Vec::new, |v| norm(v.take_log(), 0)),
            files,
        }
    }

    /// Runs the three sides of `c` and compares them.
    pub fn check(&self, c: &VCase) {
        let bash = self.run(true, false, c);
        println!("-- {}: bash: {:?}", c.name, bash.out);
        assert_eq!(
            bash.out.code,
            Some(c.code),
            "{}: bash's exit: {bash:#?}",
            c.name
        );
        assert_eq!(
            (bash.daemon.len(), bash.vault.len()),
            (c.daemon_requests, c.vault_requests),
            "{}: bash's requests: {bash:#?}",
            c.name
        );
        let sides: &[bool] = if c.handed { &[false] } else { &[false, true] };
        for &no_python3 in sides {
            let rust = self.run(false, no_python3, c);
            let side = if no_python3 {
                "rust with no python3"
            } else {
                "rust"
            };
            println!("-- {}: {side}: {:?}", c.name, rust.out);
            assert_eq!(
                rust.out.stdout, bash.out.stdout,
                "{}: {side}: stdout",
                c.name
            );
            match c.compare {
                VCompare::Exact => {
                    assert_eq!(
                        rust.out.stderr, bash.out.stderr,
                        "{}: {side}: stderr",
                        c.name
                    )
                }
                VCompare::Traceback => assert_eq!(
                    traceback_shape(&rust.out.stderr),
                    traceback_shape(&bash.out.stderr),
                    "{}: {side}: stderr",
                    c.name
                ),
            }
            assert_eq!(rust.out.code, bash.out.code, "{}: {side}: exit", c.name);
            assert_eq!(
                rust.daemon, bash.daemon,
                "{}: {side}: daemon requests",
                c.name
            );
            assert_eq!(
                rust.vault, bash.vault,
                "{}: {side}: vault socket requests",
                c.name
            );
            assert_eq!(rust.files, bash.files, "{}: {side}: files", c.name);
            self.no_secret_to_a_child(c);
        }
    }

    /// No case secret in any child's argv or environment.
    pub fn no_secret_to_a_child(&self, c: &VCase) {
        let log = std::fs::read(&self.spy_log).unwrap_or_default();
        let log = String::from_utf8_lossy(&log);
        for s in &c.secrets {
            assert!(
                !log.contains(s),
                "{}: a child of the binary saw the secret {s:?}:\n{log}",
                c.name
            );
        }
    }

    pub fn path(&self) -> &Path {
        &self.dir
    }
}

impl Drop for VRig {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Stderr with a Python traceback's frames left out: what comes before
/// `Traceback (most recent call last):`, then the exception's name.
pub fn traceback_shape(stderr: &str) -> String {
    let mut out = String::new();
    let mut in_tb = false;
    for line in stderr.split_inclusive('\n') {
        if line.starts_with("Traceback (most recent call last):") {
            in_tb = true;
            out.push_str("<traceback>\n");
            continue;
        }
        if in_tb {
            if line.starts_with(' ') {
                continue;
            }
            in_tb = false;
            let name = line.split(':').next().unwrap_or(line);
            out.push_str(name.trim_end());
            out.push('\n');
            continue;
        }
        out.push_str(line);
    }
    out
}
