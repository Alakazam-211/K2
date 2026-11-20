//! What the shell's embedded Python does that more than one family copies
//! (first written for the board family, then shared):
//!
//! - [`json`]: Python 3.12's `json.loads` errors, and how Python reads a
//!   text file or a byte string (`open(...).read()`, `bytes.decode`), with
//!   its `OSError` words.
//! - [`ops`]: the operations a reader uses on a value `json.loads` gave
//!   (`v[k]`, `k in v`, `sep.join(v)`, `x or default`, `float(word)`), with
//!   the exception Python raises where the value is not what it expects.
//!
//! - [`base64`]: `base64.b64encode`.
//! - [`difflib`]: `difflib.unified_diff` with its `SequenceMatcher`.
//! - [`mimetypes`]: `mimetypes.guess_type`, the built-in map and the
//!   system's `mime.types` files.
//! - [`path`]: `posixpath`'s `normpath`, `realpath`, `relpath` and the
//!   rest, on text.
//! - [`time`]: `time.strftime(fmt, time.localtime(secs))`.
//!
//! The rest is in [`crate::families`] (`py_get`, `py_len`, `py_d`) and
//! [`crate::out::py`].

pub mod base64;
pub mod difflib;
pub mod json;
pub mod mimetypes;
pub mod ops;
pub mod path;
pub mod time;
