//! A terminal grid model for remote rendering: snapshots and deltas of an
//! [`alacritty_terminal`] `Term`, a compact line model, and reflow of that
//! model to another width.
//!
//! A host runs the terminal emulator next to the PTY and sends what it
//! shows to any number of viewers. This crate is the shape of what gets
//! sent:
//!
//! - The [`grid_snapshot`] module: [`TermGridSnapshot`](grid_snapshot::TermGridSnapshot)
//!   (grid + scrollback + cursor + mouse/alt-screen modes) and
//!   [`TermGridDelta`](grid_snapshot::TermGridDelta) (damaged rows +
//!   appended scrollback), built from a live `Term`, with rows encoded as
//!   style-homogeneous runs. [`build_emit`](grid_snapshot::build_emit)
//!   turns the Term's damage into the next frame (full, delta or nothing).
//! - The [`grid_types`] module: the compact line model ([`GridUpdate`](grid_types::GridUpdate)):
//!   plain text per row plus sparse style spans and a soft-wrap flag.
//! - The [`reflow`] module re-wraps a [`GridUpdate`](grid_types::GridUpdate) at a
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
//!
//! # Example
//!
//! Build a `Term`, feed it bytes, take the first frame (always a full
//! snapshot), then a delta, and serialise both. Call `build_emit` after
//! each batch of PTY output; it reads and resets the Term's damage.
//!
//! ```
//! use alacritty_terminal::event::VoidListener;
//! use alacritty_terminal::grid::Dimensions;
//! use alacritty_terminal::term::{Config, Term};
//! use k2_term_grid::grid_snapshot::{build_emit, EmitDecision, EmitState};
//!
//! struct Size { cols: usize, rows: usize }
//! impl Dimensions for Size {
//!     fn total_lines(&self) -> usize { self.rows }
//!     fn screen_lines(&self) -> usize { self.rows }
//!     fn columns(&self) -> usize { self.cols }
//! }
//!
//! let mut term = Term::new(Config::default(), &Size { cols: 40, rows: 5 }, VoidListener);
//! let mut parser: vte::ansi::Processor = vte::ansi::Processor::new();
//! let mut state = EmitState::default();
//!
//! // Bytes as they come from a PTY.
//! parser.advance(&mut term, b"hello \x1b[1;31mworld\x1b[0m\r\n");
//! let EmitDecision::Full(snapshot) = build_emit("pane-1", &mut term, &mut state) else {
//!     panic!("the first frame is a full snapshot");
//! };
//! let text: String = snapshot.grid[0].iter().map(|run| run.text.as_str()).collect();
//! assert_eq!(text, "hello world");
//! assert!(snapshot.grid[0][1].bold); // "world" is its own bold, red run
//! let json = serde_json::to_string(&snapshot).unwrap(); // camelCase JSON
//! assert!(json.contains("\"paneId\":\"pane-1\""));
//!
//! // More output: the next frame carries only the damaged rows.
//! parser.advance(&mut term, b"second line");
//! let EmitDecision::Delta(delta) = build_emit("pane-1", &mut term, &mut state) else {
//!     panic!("a small change is a delta");
//! };
//! assert!(delta.damaged_rows.iter().any(|d| d.row == 1));
//! let _json = serde_json::to_string(&delta).unwrap();
//! ```

pub mod grid_snapshot;
pub mod grid_types;
pub mod reflow;
