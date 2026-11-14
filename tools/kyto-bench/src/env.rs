//! What a run takes from its surroundings: the bench that an instance
//! configuration describes, the revision of the tool, the board
//! descriptions it knows, and the nonce of each receipt.
//!
//! A bench's devices are all of the kind `simulated` or all real, of the
//! kinds that [`crate::drivers`] drives. The simulated console replays the
//! files that `[local.console]` lists in `replay`, paths relative to the
//! instance configuration: the `n`-th power-on replays the `n`-th file, one
//! line per millisecond, and after the last file the board writes nothing.
//! It never answers what it is sent. The other simulated devices take no
//! settings.

use std::fs;
use std::io::{self, Write};
use std::path::Path;
use std::rc::Rc;
use std::sync::atomic::{AtomicI32, Ordering};

use serde::Deserialize;

use kyto_bench_core::board::{Board, EMBEDDED};
use kyto_bench_core::instance::{Instance, Role};

use crate::device::{Clock, Console, Delivery, MeterConfirm, PowerConfirm, PowerSwitch};
use crate::drivers::{self, tftp};
use crate::provenance::ToolRevision;
use crate::sim::{self, ScriptProvider, SimConsole, SimRelay, SimSupplyMeter, SimTftp, SimWorld};

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
    /// The flag of the interrupts that a bench of real devices catches.
    pub interrupt: Interrupt,
}

/// The flag that SIGINT, SIGTERM and SIGHUP set, instead of ending the
/// process, once a bench of real devices is open; a simulated bench has
/// none, and a signal ends its run as it would end any process.
#[derive(Clone, Copy, Debug, Default)]
pub struct Interrupt(Option<&'static AtomicI32>);

impl Interrupt {
    /// No flag: nothing is caught.
    pub const fn none() -> Self {
        Self(None)
    }

    /// `flag`, which holds the number of the first signal caught, or 0.
    pub const fn of(flag: &'static AtomicI32) -> Self {
        Self(Some(flag))
    }

    /// Whether a signal has been caught.
    pub fn caught(&self) -> bool {
        self.0.is_some_and(|flag| flag.load(Ordering::SeqCst) != 0)
    }
}

/// What a run takes from its surroundings.
pub trait Env {
    /// The bench that the kinds of the instance's `[devices]` and its
    /// `[local]` settings describe, for `board`, whose description gives the
    /// line settings of its console; `base` is the directory of the instance
    /// configuration. The error says why there is none.
    fn open_bench(&self, board: &Board, instance: &Instance, base: &Path) -> Result<Bench, String>;

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

    /// Whether a run or a publication holds the bench of `instance` now. A
    /// simulated bench is never held.
    fn bench_in_use(&self, _instance: &Instance) -> bool {
        false
    }

    /// Says `message` on the tool's standard error, through [`say_to`].
    fn say(&self, message: &str) {
        say_to(&mut io::stderr(), message);
    }
}

/// Writes `message` to `out` as one line after `kyto-bench: `. A message
/// that cannot be written is dropped, so that it never ends a run or
/// changes an exit status.
pub fn say_to(out: &mut dyn Write, message: &str) {
    let _ = writeln!(out, "kyto-bench: {message}");
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
    fn open_bench(&self, board: &Board, instance: &Instance, base: &Path) -> Result<Bench, String> {
        if drivers::real(instance)? {
            return drivers::open(drivers::settings(board, instance)?);
        }
        let mut recordings = Vec::new();
        for role in Role::ALL {
            let Some(local) = instance.local(role).filter(|local| !local.is_empty()) else {
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

    /// Whether a run or a publication holds the TFTP directory of a bench
    /// of real devices: its lock is tried without waiting.
    fn bench_in_use(&self, instance: &Instance) -> bool {
        tftp::in_use(instance)
    }
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
        interrupt: Interrupt::none(),
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
    use kyto_bench_core::instance::SIMULATED;

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

    /// A writer that fails.
    struct Closed;

    impl Write for Closed {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            Err(io::ErrorKind::BrokenPipe.into())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn say_to_writes_one_line_after_the_tool_name() {
        let mut out = Vec::new();
        say_to(
            &mut out,
            "the run stopped; readings did confirm the power off",
        );
        say_to(&mut out, "µ");
        let said =
            "kyto-bench: the run stopped; readings did confirm the power off\nkyto-bench: µ\n";
        assert_eq!(out, said.as_bytes());
        // A message that cannot be written is dropped.
        say_to(&mut Closed, "nothing reads this");
    }

    #[test]
    fn only_a_bench_of_real_devices_catches_interrupts_or_is_in_use() {
        static FLAG: AtomicI32 = AtomicI32::new(0);
        let interrupt = Interrupt::of(&FLAG);
        assert!(!interrupt.caught());
        FLAG.store(2, Ordering::SeqCst);
        assert!(interrupt.caught() && !Interrupt::none().caught());
        let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("../examples");
        let board = Board::parse(kyto_bench_core::board::RASPBERRY_PI_5).unwrap();
        let simulated = instance(SIMULATED, "");
        let bench = HostEnv.open_bench(&board, &simulated, &base).unwrap();
        assert!(!bench.interrupt.caught());
        assert!(!HostEnv.bench_in_use(&simulated));
    }

    #[test]
    fn the_host_bench_replays_the_files_of_its_console_settings() {
        let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("../examples");
        let replay = "[local.console]\nreplay = [\"captures/echo.bin\"]\n";
        let board = Board::parse(kyto_bench_core::board::RASPBERRY_PI_5).unwrap();
        let mut bench = HostEnv
            .open_bench(&board, &instance(SIMULATED, replay), &base)
            .unwrap();
        assert!(instance(SIMULATED, "").devices().any_simulated());
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
            let opened = HostEnv.open_bench(&board, &instance, &base);
            assert!(opened.err().unwrap().starts_with(error), "{error}");
        }
        // A bench with any simulated device counts as simulated.
        assert!(instance("relay", "").devices().any_simulated());
    }
}
