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
//! events and the client's input stay JSON text. That session protocol
//! (frame vocabulary, ack pacing) will move here later; today this crate
//! holds the codec.

pub mod k1;
