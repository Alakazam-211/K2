//! The k5 command line in Rust, built piece by piece.
//!
//! So far it holds one module, [`out::json`]: a JSON reader and printer whose
//! output is byte for byte what Python 3 prints for
//! `json.dumps(json.loads(text), **options)`. The shell CLI pipes the daemon's
//! JSON through Python before printing it, so the Rust CLI must print the
//! same bytes.

pub mod out;
