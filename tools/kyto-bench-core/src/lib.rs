//! The part of the kyto bench tool that has no input or output: digests,
//! names, console lines, line patterns, device roles, the documents it
//! reads, the write and minimum-bootloader rules, what a cold boot shows
//! and the payload checks. It reaches no device, clock, file, network or
//! environment variable, so all it decides can be decided again from records.

// A member's Clippy settings cannot add lints, so output is denied here.
#![deny(clippy::print_stdout, clippy::print_stderr, clippy::dbg_macro)]

pub mod board;
pub mod bootloader;
pub mod digest;
pub mod document;
pub mod identity;
pub mod instance;
pub mod lines;
pub mod names;
pub mod observe;
pub mod pattern;
pub mod payload;
pub mod plan;
pub mod receipt;
pub mod roles;
pub mod write;
