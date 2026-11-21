//! The k5 command line in Rust, built piece by piece.
//!
//! It replaces the shell CLI (`cli/k2`, built from `cli/src`) one family of
//! verbs at a time; a verb it does not handle goes on to the shell script.
//! So it must resolve names, settings and paths exactly as the shell does.
//!
//! It is not std-only: it depends on `k2-client`, which holds the one
//! registry of setting names (`env_names`), the product's names (`brand`)
//! and the K2 home's paths (`paths`). This crate reads them there and
//! keeps no copy.
//!
//! So far it holds:
//!
//! - [`out::json`]: a JSON reader and printer whose output is byte for byte
//!   what Python 3 prints for `json.dumps(json.loads(text), **options)`. The
//!   shell CLI pipes the daemon's JSON through Python before printing it, so
//!   the Rust CLI must print the same bytes.
//! - [`transport::encode`]: the query string escaping the shell CLI gets from
//!   Python's `urllib.parse.quote(value, safe='')`.
//! - [`transport`]: the HTTP client that sends the daemon what the shell
//!   CLI's curl sends for the same call, over loopback TCP and the cell
//!   socket, with its tries, time limits and permission note, and the one
//!   line when the server's version is not the binary's
//!   ([`transport::skew`]).
//! - [`daemon`]: where the daemon is and the credential the binary's own
//!   requests use, by the shell CLI's settings.
//! - [`called_as`]: the name the command was called by (k2 or k5), and the
//!   variables the front hands the shell when a verb falls through.
//! - [`home`]: the K2 home and a workspace's private folder, by k2-client's
//!   rules.
//! - [`route`]: the route table, one row per arm of the shell router's
//!   top-level `case`: who answers each command (the binary, its
//!   `--schema` entry, or the shell CLI).
//! - [`front`]: what the binary (`src/main.rs`, `k5-cli`) does with a
//!   command line, by the route table: `--version`, `--version --check`
//!   ([`check`]), `cli status` ([`cli_status`]) and the families in
//!   [`families`] here, everything else to the shell CLI.
//! - [`families`]: the command families the binary answers itself, as the
//!   shell CLI does: `whoami`, `done`, `respond`, `read`, `activity` and
//!   the read forms of `inbox`, `sessions`, `connections` and `feedback`
//!   so far.
//! - [`prelude`]: what the shell CLI does around such a verb: its first
//!   line, its two gates, an `id` tool's passport checks and its EXIT
//!   trap's lines.
//! - [`fallthrough`]: handing a verb to the shell CLI, byte for byte, from
//!   the copy of `cli/k2` packed into the binary ([`embedded`]).
//! - [`passport`]: the passport rule the front applies before a verb
//!   falls through. It only counts for now.
//! - [`port`]: the shell's port check, before any command runs: a port
//!   setting or saved port outside 1 to 65535 stops it (exit 2).
//! - [`trace`]: opt-in timing of the front's phases on stderr
//!   (`K5_CLI_TRACE_TIMING`).
//! - [`text`]: the study and help pages, embedded from `cli/text`, and the
//!   help forms the binary may answer; [`pages`]: which command line is
//!   which page, and whether the shell's environment ([`offline`]) lets
//!   the binary print it.
//! - [`cards`]: whether permission cards are on for this caller, asked
//!   as the shell asks before a page that depends on them.
//! - [`feedback_optin`]: whether send-feedback is hidden from this
//!   caller while agents may not report, asked as the shell asks before
//!   the daily tier of the help and send-feedback's study page.
//! - [`tools`]: the shell's tool table, which catalog tool a command line
//!   is and whether it needs the session's own passport.
//! - [`python`]: whether the shell has a Python 3 to run, and whether a
//!   native command runs without one.
//! - [`retired`]: the shell's table of retired verbs, for the line a
//!   retired verb gets and the deprecated tier of the help.

pub mod airgap;
pub mod called_as;
pub mod cards;
pub mod check;
pub mod cli_status;
pub mod daemon;
pub mod embedded;
pub mod exit;
pub mod fallthrough;
pub mod families;
pub mod feedback_optin;
pub mod front;
pub mod home;
pub mod notice;
pub mod offline;
pub mod out;
pub mod pages;
pub mod passport;
pub mod port;
pub mod prelude;
pub mod python;
pub mod retired;
pub mod route;
pub mod text;
pub mod tools;
pub mod trace;
pub mod transport;
