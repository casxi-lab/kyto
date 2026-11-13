//! The kyto hardware-in-the-loop bench tool. It drives a bench through the
//! device interfaces of [`device`], of which [`sim`] has one simulated device
//! per class, stages payload files that let the board's firmware write only
//! through the write [`gate`], and knows the revision that its build
//! embeds. Its command line, [`cli`], reads the files of a run ([`inputs`]),
//! opens the bench of the instance configuration ([`env`]), runs the plan
//! ([`run`]) while it records what happens ([`record`]), and writes the
//! receipt ([`assemble`]) and its public version ([`output`]). The formats
//! and rules it uses are in `kyto_bench_core`.

pub mod assemble;
pub mod cli;
pub mod device;
pub mod env;
pub mod gate;
pub mod inputs;
pub mod output;
pub mod provenance;
pub mod record;
pub mod run;
pub mod sim;
