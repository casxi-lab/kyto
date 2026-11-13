//! Simulated devices: one of each class, all on one discrete-event world.
//!
//! A [`SimWorld`] holds the simulated time, the relay's contacts, the supply
//! that they feed, the board's console output and the staged TFTP
//! generations. Nothing runs in the background and nothing sleeps: a device
//! call computes the world's state at the current simulated time, and only
//! the clock's waits and the console's opening and receiving move the time
//! on. The world is the bench clock of its devices, so a run on them is
//! deterministic.
//!
//! [`Knobs`] inject faults into each device; a knob changes what a device
//! does, never its interface. Every knob is off by default.

mod console;
mod meter;
mod relay;
mod tftp;
mod world;

pub use console::{ConsoleKnobs, Files, Responder, Script, ScriptProvider, SimConsole, replay};
pub use meter::{MeterKnobs, SimSupplyMeter};
pub use relay::{RelayKnobs, SimRelay};
pub use tftp::{SimTftp, TftpKnobs};
pub use world::{Knobs, ON_MICRO, SimWorld, SupplyKnobs};
