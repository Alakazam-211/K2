//! Retired-but-preserved surface.
//!
//! Modules here are kept compiling so existing callers don't break,
//! but every public item carries a `#[deprecated]` annotation pointing
//! at the rationale. New code MUST NOT add references to this module —
//! it exists only to give the deprecation window a clean home in the
//! source tree.
//!
//! Each module's top-level doc comment explains why it was retired and
//! what replaced it (if anything).

#![allow(deprecated)]

pub mod delegate;
