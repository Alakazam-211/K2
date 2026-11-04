//! Board files (`.k5board`) in a K2 tab, single-player (prd-board-tab-v1).
//!
//! The file is always the truth. The format's codec (`k2-mpc-board`) is the
//! only code that reads or writes the format; K2 owns the file: where it lives,
//! the writer lock, the guard's compare, and the atomic replace.

// The Garden's way out (`garden_board_to_file`, `People`) is M2's export.
#[allow(dead_code)]
pub mod map;
pub mod held;
pub mod routes;
pub mod save;
pub mod stage;
