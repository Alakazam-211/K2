//! The k5 command line in Rust, built piece by piece.
//!
//! So far it holds two modules:
//!
//! - [`out::json`]: a JSON reader and printer whose output is byte for byte
//!   what Python 3 prints for `json.dumps(json.loads(text), **options)`. The
//!   shell CLI pipes the daemon's JSON through Python before printing it, so
//!   the Rust CLI must print the same bytes.
//! - [`transport::encode`]: the query string escaping the shell CLI gets from
//!   Python's `urllib.parse.quote(value, safe='')`.

pub mod out;
pub mod transport;
