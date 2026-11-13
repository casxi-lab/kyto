//! The part of the kyto bench tool that has no input or output: digests,
//! names, console lines, line patterns, the write rules and the device roles.
//! It reaches no device, clock, file, network or environment variable, so
//! everything it decides can be decided again from recorded data.

// A member's Clippy settings cannot add lints, so output is denied here.
#![deny(clippy::print_stdout, clippy::print_stderr, clippy::dbg_macro)]

pub mod digest;
pub mod document;
pub mod lines;
pub mod names;
pub mod pattern;
pub mod roles;
pub mod write;
