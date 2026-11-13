//! The part of the kyto bench tool that has no input or output: names,
//! digests, lines, patterns, roles, the documents it reads and writes, the
//! write and bootloader rules, what a cold boot shows, payload checks, the
//! rules of the supply, checks and verdicts, evidence, the re-derivation of
//! a receipt's judgement and public receipts. It reaches no device, clock,
//! file, network or environment variable, so all it decides can be decided
//! again from records.

// A member's Clippy settings cannot add lints, so output is denied here.
#![deny(clippy::print_stdout, clippy::print_stderr, clippy::dbg_macro)]

pub mod board;
pub mod bootloader;
pub mod digest;
pub mod document;
pub mod evidence;
pub mod identity;
pub mod instance;
pub mod lines;
pub mod names;
pub mod observe;
pub mod pattern;
pub mod payload;
pub mod plan;
pub mod power;
pub mod public;
pub mod receipt;
pub mod rejudge;
pub mod roles;
pub mod verdict;
pub mod write;
