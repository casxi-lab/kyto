//! What a run takes from its surroundings: the bench that an instance
//! configuration describes, the revision of the tool, the board
//! descriptions it knows, and the nonce of each receipt.
//!
//! This build knows one kind of device, `simulated`, for every role, and a
//! bench of simulated devices only. The simulated console replays the files
//! that `[local.console]` lists in `replay`, paths relative to the instance
//! configuration: the `n`-th power-on replays the `n`-th file, one line per
//! millisecond, and after the last file the board writes nothing. It never
//! answers what it is sent. The other simulated devices take no settings.

use std::fs;
use std::path::Path;
use std::rc::Rc;

use serde::Deserialize;

use kyto_bench_core::board::EMBEDDED;
use kyto_bench_core::instance::{Instance, Role};

use crate::device::{Clock, Console, Delivery, MeterConfirm, PowerConfirm, PowerSwitch};
use crate::provenance::ToolRevision;
use crate::sim::{self, ScriptProvider, SimConsole, SimRelay, SimSupplyMeter, SimTftp, SimWorld};

/// The kind of a simulated device.
pub const SIMULATED: &str = "simulated";

/// The devices of a bench and the clock that they share.
pub struct Bench {
    pub clock: Rc<dyn Clock>,
    pub power: Box<dyn PowerSwitch>,
    pub confirm: Box<dyn PowerConfirm>,
    pub console: Box<dyn Console>,
    pub delivery: Box<dyn Delivery>,
    /// The relay that power events name, from this machine's settings of
    /// the power device; `None` for a simulated one.
    pub relay: Option<String>,
}

/// What a run takes from its surroundings.
pub trait Env {
    /// The bench that the kinds of the instance's `[devices]` and its
    /// `[local]` settings describe; `base` is the directory of the instance
    /// configuration. The error says why there is none.
    fn open_bench(&self, instance: &Instance, base: &Path) -> Result<Bench, String>;

    /// The revision of the tool.
    fn tool(&self) -> ToolRevision {
        ToolRevision::built()
    }

    /// The texts of the board descriptions that a run can use.
    fn boards(&self) -> &[&'static str] {
        &EMBEDDED
    }

    /// The nonce of a receipt: 128 bits from the operating system's
    /// generator, as [`nonce`] gives them.
    fn nonce(&self) -> Result<String, String> {
        nonce()
    }
}

/// The surroundings of the `kyto-bench` command.
#[derive(Clone, Copy, Debug, Default)]
pub struct HostEnv;

/// The settings of a simulated console.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReplaySettings {
    replay: Vec<String>,
}

impl Env for HostEnv {
    fn open_bench(&self, instance: &Instance, base: &Path) -> Result<Bench, String> {
        let mut recordings = Vec::new();
        for role in Role::ALL {
            let (kind, local) = (&instance.devices().device(role).kind, instance.local(role));
            if kind != SIMULATED {
                return Err(format!(
                    "this build has no {role} device of the kind {kind}"
                ));
            }
            let Some(local) = local.filter(|local| !local.is_empty()) else {
                continue;
            };
            if role != Role::Console {
                return Err(format!(
                    "[local.{role}]: a simulated {role} takes no settings"
                ));
            }
            let settings: ReplaySettings = local
                .clone()
                .try_into()
                .map_err(|error| format!("[local.{role}]: {error}"))?;
            for path in settings.replay {
                let read = fs::read(base.join(&path));
                recordings.push(read.map_err(|error| format!("cannot read {path}: {error}"))?);
            }
        }
        Ok(simulated(&SimWorld::new(), sim::replay(recordings)))
    }
}

/// Whether every device of `instance` is simulated.
pub fn is_simulated(instance: &Instance) -> bool {
    Role::ALL
        .iter()
        .all(|role| instance.devices().device(*role).kind == SIMULATED)
}

/// The bench of a simulated device of each role on `world`, whose console
/// plays what `script` gives.
pub fn simulated(world: &SimWorld, script: ScriptProvider) -> Bench {
    Bench {
        clock: Rc::new(world.clone()),
        power: Box::new(SimRelay::new(world)),
        confirm: Box::new(MeterConfirm(SimSupplyMeter::new(world))),
        console: Box::new(SimConsole::new(world, script)),
        delivery: Box::new(SimTftp::new(world)),
        relay: None,
    }
}

/// A nonce: 128 bits from the operating system's generator, as 32
/// lowercase hexadecimal digits.
pub fn nonce() -> Result<String, String> {
    let mut bits = [0; 16];
    getrandom::fill(&mut bits).map_err(|error| format!("no random bits: {error}"))?;
    Ok(bits.iter().map(|byte| format!("{byte:02x}")).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::{Nanos, PowerState, Received};

    /// An instance configuration of simulated devices with `local` as its
    /// `[local]` tables after the salt.
    fn instance(kind: &str, local: &str) -> Instance {
        let text = include_str!("../../examples/instance.toml");
        let at = text.find("\n[local.console]").unwrap();
        // A salt that kyto does not publish, which devices of another kind
        // need.
        let salt = text.find("salt = \"").unwrap() + 8;
        let text = [&text[..salt], &"0".repeat(32), &text[salt + 32..at]].concat();
        let text = text.replacen(
            "power = { kind = \"simulated\"",
            &format!("power = {{ kind = \"{kind}\""),
            1,
        );
        Instance::parse(&format!("{text}\n{local}")).unwrap()
    }

    #[test]
    fn nonce_is_128_bits_from_the_operating_system() {
        let (first, second) = (nonce().unwrap(), nonce().unwrap());
        for nonce in [&first, &second] {
            assert_eq!(nonce.len(), 32);
            assert!(
                nonce
                    .bytes()
                    .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
            );
        }
        assert_ne!(first, second);
    }

    #[test]
    fn the_host_bench_replays_the_files_of_its_console_settings() {
        let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("../examples");
        let replay = "[local.console]\nreplay = [\"captures/echo.bin\"]\n";
        let mut bench = HostEnv
            .open_bench(&instance(SIMULATED, replay), &base)
            .unwrap();
        assert!(is_simulated(&instance(SIMULATED, "")));
        assert_eq!(bench.relay, None);
        bench.console.open(Nanos(0)).unwrap();
        bench.power.command(PowerState::On).unwrap();
        let first = bench.console.receive(Nanos::from_millis(5)).unwrap();
        let line = Received::Bytes {
            at: Nanos::from_millis(1),
            bytes: b"\xff\r\n".to_vec(),
        };
        assert_eq!(first, line);
        let refused = [
            (
                instance("relay", ""),
                "this build has no power device of the kind relay",
            ),
            (
                instance(SIMULATED, "[local.power]\nport = \"a\"\n"),
                "[local.power]: a simulated power takes no settings",
            ),
            (
                instance(SIMULATED, "[local.console]\nreplay = [\"absent.bin\"]\n"),
                "cannot read absent.bin",
            ),
        ];
        for (instance, error) in refused {
            let opened = HostEnv.open_bench(&instance, &base);
            assert!(opened.err().unwrap().starts_with(error), "{error}");
        }
        assert!(!is_simulated(&instance("relay", "")));
    }
}
