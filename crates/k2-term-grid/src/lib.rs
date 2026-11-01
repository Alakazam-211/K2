//! A terminal grid model for remote rendering: snapshots and deltas of an
//! [`alacritty_terminal`] `Term`, a compact line model, and reflow of that
//! model to another width.
//!
//! A host runs the terminal emulator next to the PTY and sends what it
//! shows to any number of viewers. This crate is the shape of what gets
//! sent:
//!
//! - [`grid_snapshot`]: [`TermGridSnapshot`](grid_snapshot::TermGridSnapshot)
//!   (grid + scrollback + cursor + mouse/alt-screen modes) and
//!   [`TermGridDelta`](grid_snapshot::TermGridDelta) (damaged rows +
//!   appended scrollback), built from a live `Term`, with rows encoded as
//!   style-homogeneous runs. [`build_emit`](grid_snapshot::build_emit)
//!   turns the Term's damage into the next frame (full, delta or nothing).
//! - [`grid_types`]: the compact line model ([`GridUpdate`](grid_types::GridUpdate)):
//!   plain text per row plus sparse style spans and a soft-wrap flag.
//! - [`reflow`]: re-wraps a [`GridUpdate`](grid_types::GridUpdate) at a
//!   different width, so a narrow viewer (a phone) gets real line breaks
//!   instead of a clipped or scaled screen.
//!
//! The types serialize with `serde` (camelCase JSON). For a compact
//! binary encoding of the snapshot and delta types, see the companion wire
//! crate (`k2-term-wire`).
//!
//! `alacritty_terminal` is licensed Apache-2.0 only, so a build that uses
//! this crate carries an Apache-2.0 dependency whichever licence you take
//! this crate under.

pub mod grid_snapshot;
pub mod grid_types;
pub mod reflow;
