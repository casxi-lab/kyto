//! The kyto hardware-in-the-loop bench tool. It drives a bench through the
//! device interfaces of [`device`], of which [`sim`] has one simulated device
//! per class, stages payload files that let the board's firmware write only
//! with a permit of the [`gate`], and knows the revision that its build
//! embeds. The formats and rules it uses are in `kyto_bench_core`.

pub mod device;
pub mod gate;
pub mod provenance;
pub mod sim;
