//! A compact binary wire format ("k1") for terminal grid snapshots and
//! deltas, sent as websocket binary messages.
//!
//! [`k1`] encodes and decodes the snapshot and delta types of the grid
//! crate (`k2-term-grid`): fixed-layout little-endian integers, length-
//! prefixed UTF-8 run text, a sentinel for "default colour", style bits in
//! one byte. A k1 frame carries exactly what the JSON form carries, about
//! 2.5× smaller, and decoding it reproduces the same object. The byte
//! layout is documented once, in the [`k1`] module docs.
//!
//! The intended transport is one websocket per viewer: the server sends a
//! snapshot on connect and deltas after, as binary frames, while the other
//! events and the client's input stay JSON text. This crate holds the
//! codec only; the session protocol around it is up to you.
//!
//! "k1" is the name of wire format version 1. Every frame starts with the
//! magic byte [`WIRE_MAGIC`](k1::WIRE_MAGIC) (`0x6B`, ASCII `k`), then the
//! format version [`WIRE_VERSION`](k1::WIRE_VERSION) (`1`), then the frame
//! kind. [`decode_frame`](k1::decode_frame) refuses a frame with another
//! magic byte or version with a [`WireDecodeError`](k1::WireDecodeError);
//! it never guesses. The next incompatible layout gets a new version
//! number, so a decoder that only knows k1 fails loudly instead of
//! misreading it. (See the [`k1`] docs for one flag added within version 1.)
//!
//! # Example
//!
//! ```
//! use k2_term_grid::grid_snapshot::{CellRun, CursorSnapshot, TermGridSnapshot};
//! use k2_term_wire::k1::{decode_frame, encode_snapshot, WireFrame, WIRE_MAGIC, WIRE_VERSION};
//!
//! let run = CellRun {
//!     text: "hello".into(),
//!     fg: Some(0xff8800), // 0xRRGGBB; None = the terminal's default colour
//!     bg: None,
//!     bold: true,
//!     italic: false,
//!     underline: false,
//!     inverse: false,
//!     dim: false,
//!     strikeout: false,
//!     wrapped: None,
//!     cols: None,
//! };
//! let snapshot = TermGridSnapshot {
//!     pane_id: "pane-1".into(),
//!     cols: 80,
//!     rows: 24,
//!     grid: vec![vec![run]],
//!     scrollback: vec![],
//!     cursor: CursorSnapshot { row: 0, col: 5, visible: true },
//!     version: 1,
//!     display_offset: 0,
//!     mouse_report: false,
//!     sgr_mouse: false,
//!     alt_screen: false,
//! };
//!
//! let bytes = encode_snapshot(&snapshot); // send as one websocket binary message
//! assert_eq!(&bytes[..2], &[WIRE_MAGIC, WIRE_VERSION]);
//!
//! let WireFrame::Snapshot(back) = decode_frame(&bytes).unwrap() else {
//!     panic!("a snapshot decodes as a snapshot");
//! };
//! assert_eq!(back.pane_id, "pane-1");
//! assert_eq!(back.grid[0][0].text, "hello");
//! assert_eq!(back.grid[0][0].fg, Some(0xff8800));
//! assert!(back.grid[0][0].bold);
//! assert_eq!((back.cursor.row, back.cursor.col), (0, 5));
//!
//! // A frame from another format version is refused, not misread.
//! let mut future = bytes.clone();
//! future[1] = 2;
//! assert!(decode_frame(&future).is_err());
//! ```

pub mod k1;
