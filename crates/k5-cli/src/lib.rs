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
//! - [`called_as`]: the name the command was called by (k2 or k5), and the
//!   variables the front hands the shell when a verb falls through.
//! - [`home`]: the K2 home and a workspace's private folder, by k2-client's
//!   rules.
//! - [`front`]: what the binary (`src/main.rs`, `k5-cli`) does with a
//!   command line: `--version` here, everything else to the shell CLI.
//! - [`fallthrough`]: handing a verb to the shell CLI, byte for byte, from
//!   the copy of `cli/k2` packed into the binary ([`embedded`]).

pub mod called_as;
pub mod embedded;
pub mod fallthrough;
pub mod front;
pub mod home;
pub mod out;
pub mod transport;
