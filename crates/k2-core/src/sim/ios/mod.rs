//! The iOS Simulator lane on a Mac daemon (prd-mobile-dev-v1 §3.3, L-I),
//! implementing [`crate::sim::SimLane`].
//!
//! - [`simctl`]: `xcrun simctl --set <K2 set>`, K2's own device set
//!   (`~/.k2/sim/ios/set`), never the person's own Simulators.
//! - [`helper`]: `k2-simhelper`, compiled on first use with the Mac's Xcode;
//!   frames, input and the accessibility tree over its pipes.
//! - [`tree`]: accessibility JSON → the core's `RawNode`s.
//! - [`lane`]: `IosLane`.
//!
//! L-I spike (2026-10-27, z3mbpZ macOS 27.0.1 + Xcode 26.5, M5 + Xcode
//! 26.6, iOS 26.5): AXe 1.8.0 can't see a non-default device set (M-A14), so
//! K2 carries its own helper instead of AXe. Typing with no Simulator window
//! works through the HID keyboard; setting the value through accessibility
//! is the fallback that needs no keyboard at all (M-A17).

pub mod helper;
pub mod lane;
pub mod simctl;
pub mod tree;

pub use lane::{IosLane, IosLaneConfig, TESTED_XCODE};
