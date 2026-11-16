//! `mpc-tools`: the kit's and the packs' check and build tools, one binary with subcommands
//! (tools/README.md, "mpc-tools").
//!
//! The rules every subcommand keeps:
//! - exit codes: 0 ok, 1 an error or a check that found problems, 2 bad usage, 3 refused
//!   (the tool would not do what was asked, and changed nothing), 4 not found;
//! - stdout carries the result only; `--json` prints exactly one JSON document there instead;
//!   notices and errors go to stderr.
//!
//! [`run`] is the whole tool with its process boundary passed in ([`Ctx`]), so tests drive it
//! in-process with their own arguments, stdin, environment and folder.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use clap::{Parser, Subcommand};

pub mod edit;
pub mod evidence;
pub mod licences;
pub mod pack_check;
pub mod py;
pub mod pyjson;
pub mod undo;
pub mod unzip;

/// Exit code: done, or a check that found nothing.
pub const OK: i32 = 0;
/// Exit code: an error, or a check that found problems.
pub const ERROR: i32 = 1;
/// Exit code: bad usage.
pub const USAGE: i32 = 2;
/// Exit code: refused; nothing was changed.
pub const REFUSED: i32 = 3;
/// Exit code: an input (a file, a folder, an archive member) is not there.
pub const NOT_FOUND: i32 = 4;

/// What a run reads from outside the arguments.
pub struct Ctx {
    /// The standard input, when the caller has it in hand; `None` reads the process's own.
    pub stdin: Option<Vec<u8>>,
    /// The environment the subcommands read (by name).
    pub env: BTreeMap<String, String>,
    /// The folder relative paths are taken from.
    pub cwd: PathBuf,
}

impl Ctx {
    /// The process's own stdin, environment and current folder.
    pub fn from_process() -> Ctx {
        Ctx {
            stdin: None,
            env: std::env::vars().collect(),
            cwd: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
        }
    }

    /// Reads the whole of stdin.
    pub fn read_stdin(&self) -> Result<Vec<u8>, Fail> {
        match &self.stdin {
            Some(b) => Ok(b.clone()),
            None => {
                let mut b = Vec::new();
                std::io::Read::read_to_end(&mut std::io::stdin(), &mut b)
                    .map_err(|e| Fail::error(format!("stdin: {e}")))?;
                Ok(b)
            }
        }
    }

    /// `p` taken from [`Ctx::cwd`] when it is relative.
    pub fn path(&self, p: &Path) -> PathBuf {
        if p.is_absolute() {
            p.to_path_buf()
        } else {
            self.cwd.join(p)
        }
    }

    /// An environment variable.
    pub fn var(&self, name: &str) -> Option<&str> {
        self.env.get(name).map(String::as_str)
    }
}

/// A run's outcome, as the process ends with it.
#[derive(Debug, Default)]
pub struct Output {
    pub code: i32,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

/// A subcommand's result: its exit code ([`OK`], or [`ERROR`] for a check that found
/// problems), the text for stdout, the JSON document for `--json`, and notices for stderr.
#[derive(Debug)]
pub struct Report {
    pub code: i32,
    pub text: String,
    pub json: serde_json::Value,
    pub notices: Vec<String>,
}

impl Report {
    pub fn new(code: i32, text: String, json: serde_json::Value) -> Report {
        Report {
            code,
            text,
            json,
            notices: Vec::new(),
        }
    }
}

/// A subcommand that stopped: the exit code and one line for stderr.
#[derive(Debug, PartialEq, Eq)]
pub struct Fail {
    pub code: i32,
    pub message: String,
}

impl Fail {
    pub fn error(message: impl Into<String>) -> Fail {
        Fail {
            code: ERROR,
            message: message.into(),
        }
    }
    pub fn usage(message: impl Into<String>) -> Fail {
        Fail {
            code: USAGE,
            message: message.into(),
        }
    }
    pub fn refused(message: impl Into<String>) -> Fail {
        Fail {
            code: REFUSED,
            message: message.into(),
        }
    }
    pub fn not_found(message: impl Into<String>) -> Fail {
        Fail {
            code: NOT_FOUND,
            message: message.into(),
        }
    }
    /// An I/O error on `path`: [`NOT_FOUND`] when it isn't there, [`ERROR`] otherwise.
    pub fn io(path: &Path, e: std::io::Error) -> Fail {
        let message = format!("{}: {e}", path.display());
        if e.kind() == std::io::ErrorKind::NotFound {
            Fail::not_found(message)
        } else {
            Fail::error(message)
        }
    }
}

#[derive(Parser)]
#[command(
    name = "mpc-tools",
    version,
    about = "The kit's and the packs' check and build tools",
    long_about = "The kit's and the packs' check and build tools.\n\nExit codes: 0 ok, 1 an error or a check that found problems, 2 bad usage, 3 refused (nothing changed), 4 not found. stdout carries the result only; --json prints exactly one JSON document instead; notices go to stderr."
)]
struct Cli {
    /// Print the result as exactly one JSON document on stdout
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Every dependency's licence is on the allow-list (every tracked Cargo.lock)
    Licences(licences::Args),
    /// The undo model check's tools
    Undo {
        #[command(subcommand)]
        cmd: undo::Cmd,
    },
    /// The pack template's checks, run from a pack's root
    PackCheck {
        #[command(subcommand)]
        cmd: pack_check::Cmd,
    },
    /// Take one member out of a zip, or all of them
    Unzip(unzip::Args),
    /// Small exact edits of a text file
    Edit {
        #[command(subcommand)]
        cmd: edit::Cmd,
    },
    /// The evidence notes' tools: the citation check and the run-record collector
    Evidence {
        #[command(subcommand)]
        cmd: evidence::Cmd,
    },
}

/// Runs the tool on `args` (the program name first, as `std::env::args_os` gives them).
pub fn run<I, T>(args: I, ctx: &Ctx) -> Output
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    let cli = match Cli::try_parse_from(args) {
        Ok(cli) => cli,
        Err(e) => {
            let text = e.render().to_string().into_bytes();
            return match e.kind() {
                clap::error::ErrorKind::DisplayHelp | clap::error::ErrorKind::DisplayVersion => {
                    Output {
                        code: OK,
                        stdout: text,
                        stderr: Vec::new(),
                    }
                }
                _ => Output {
                    code: USAGE,
                    stdout: Vec::new(),
                    stderr: text,
                },
            };
        }
    };
    let result = match cli.cmd {
        Cmd::Licences(a) => licences::run(&a, ctx),
        Cmd::Undo { cmd } => undo::run(&cmd, ctx),
        Cmd::PackCheck { cmd } => pack_check::run(&cmd, ctx),
        Cmd::Unzip(a) => unzip::run(&a, ctx),
        Cmd::Edit { cmd } => edit::run(&cmd, ctx),
        Cmd::Evidence { cmd } => evidence::run(&cmd, ctx),
    };
    finish(result, cli.json)
}

fn finish(result: Result<Report, Fail>, json: bool) -> Output {
    let mut out = Output::default();
    match result {
        Ok(r) => {
            for n in &r.notices {
                out.stderr.extend_from_slice(n.as_bytes());
                out.stderr.push(b'\n');
            }
            if json {
                out.stdout = serde_json::to_string(&r.json)
                    .expect("a JSON value always prints")
                    .into_bytes();
                out.stdout.push(b'\n');
            } else {
                out.stdout = r.text.into_bytes();
            }
            out.code = r.code;
        }
        Err(f) => {
            out.stderr = format!("{}\n", f.message).into_bytes();
            if json {
                let doc = serde_json::json!({ "error": f.message, "code": f.code });
                out.stdout = format!("{doc}\n").into_bytes();
            }
            out.code = f.code;
        }
    }
    out
}
