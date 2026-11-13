//! The simulated TFTP directory: image delivery for network boot.

use std::collections::BTreeMap;

use kyto_bench_core::names::{self, GenerationName, PayloadName};
use kyto_bench_core::write::WriteTrigger;

use crate::device::{Delivery, DeviceError, Role};

use super::console::Files;
use super::world::SimWorld;

/// The directory that the board's TFTP server reads, kept in memory: staged
/// generations and the pointer to the served one. A new generation never
/// touches an existing one, and the board is served through the pointer.
pub struct SimTftp {
    world: SimWorld,
    triggers: Vec<WriteTrigger>,
}

/// Faults of the TFTP directory.
#[derive(Clone, Debug, Default)]
pub struct TftpKnobs {
    /// Staging fails before anything is written.
    pub stage_fails: bool,
    /// Staging writes a changed copy of every file.
    pub corrupts_staged: bool,
    /// The board is served a changed copy of every file.
    pub corrupts_served: bool,
    /// Switching the served generation fails.
    pub switch_fails: bool,
}

/// The directory's part of the world.
#[derive(Default)]
pub(super) struct TftpState {
    generations: BTreeMap<GenerationName, Files>,
    served: Option<GenerationName>,
}

impl TftpState {
    /// The files of the served generation.
    pub(super) fn served(&self) -> Option<&Files> {
        self.served.and_then(|name| self.generations.get(&name))
    }
}

impl SimTftp {
    /// The TFTP directory of `world`, for a board whose write triggers are
    /// `triggers`.
    pub fn new(world: &SimWorld, triggers: Vec<WriteTrigger>) -> Self {
        Self {
            world: world.clone(),
            triggers,
        }
    }
}

fn delivery_error(detail: impl Into<String>) -> DeviceError {
    DeviceError::new(Role::Delivery, detail)
}

/// Changes one bit of `bytes`, or adds a byte to an empty file.
fn corrupt(bytes: &mut Vec<u8>) {
    match bytes.first_mut() {
        Some(first) => *first ^= 1,
        None => bytes.push(0),
    }
}

impl Delivery for SimTftp {
    fn write_triggers(&self) -> &[WriteTrigger] {
        &self.triggers
    }

    fn write_generation(
        &mut self,
        files: &[(PayloadName, Vec<u8>)],
    ) -> Result<GenerationName, DeviceError> {
        let mut world = self.world.borrow_mut();
        if world.knobs.tftp.stage_fails {
            return Err(delivery_error(
                "the simulated TFTP directory could not stage",
            ));
        }
        names::check_distinct(files.iter().map(|(name, _)| name))
            .map_err(|same| delivery_error(same.to_string()))?;
        let name = GenerationName::next_after(world.tftp.generations.keys().copied())
            .ok_or_else(|| delivery_error("no generation name is left"))?;
        let corrupts = world.knobs.tftp.corrupts_staged;
        let generation = files
            .iter()
            .map(|(file, bytes)| {
                let mut bytes = bytes.clone();
                if corrupts {
                    corrupt(&mut bytes);
                }
                (file.clone(), bytes)
            })
            .collect();
        world.tftp.generations.insert(name, generation);
        Ok(name)
    }

    fn read_staged(
        &mut self,
        generation: &GenerationName,
        name: &PayloadName,
    ) -> Result<Vec<u8>, DeviceError> {
        let world = self.world.borrow();
        let files = world
            .tftp
            .generations
            .get(generation)
            .ok_or_else(|| delivery_error(format!("{generation} is not staged")))?;
        files
            .get(name)
            .cloned()
            .ok_or_else(|| delivery_error(format!("{generation} has no file {name}")))
    }

    fn switch_to(
        &mut self,
        generation: &GenerationName,
    ) -> Result<Option<GenerationName>, DeviceError> {
        let mut world = self.world.borrow_mut();
        if world.knobs.tftp.switch_fails {
            return Err(delivery_error(
                "the simulated TFTP directory could not switch",
            ));
        }
        if !world.tftp.generations.contains_key(generation) {
            return Err(delivery_error(format!("{generation} is not staged")));
        }
        Ok(world.tftp.served.replace(*generation))
    }

    fn read_served(&mut self, name: &PayloadName) -> Result<Vec<u8>, DeviceError> {
        let world = self.world.borrow();
        let files = world
            .tftp
            .served()
            .ok_or_else(|| delivery_error("no generation is served"))?;
        let mut bytes = files
            .get(name)
            .cloned()
            .ok_or_else(|| delivery_error(format!("the served generation has no file {name}")))?;
        if world.knobs.tftp.corrupts_served {
            corrupt(&mut bytes);
        }
        Ok(bytes)
    }
}

#[cfg(test)]
mod tests {
    use kyto_bench_core::board::{self, Board};
    use kyto_bench_core::write::{Unpermitted, WriteClass, WriteEnables};

    use super::*;
    use crate::device::StageError;
    use crate::gate::{WriteGate, WriteRefused};

    fn name(text: &str) -> PayloadName {
        PayloadName::parse(text).unwrap()
    }

    fn files(pairs: &[(&str, &[u8])]) -> Vec<(PayloadName, Vec<u8>)> {
        pairs
            .iter()
            .map(|(file, bytes)| (name(file), bytes.to_vec()))
            .collect()
    }

    fn generation(number: u32) -> GenerationName {
        GenerationName::new(number).unwrap()
    }

    /// The TFTP directory of a world, for the Raspberry Pi 5.
    fn pi5_tftp() -> (SimWorld, SimTftp) {
        let board = Board::parse(board::RASPBERRY_PI_5).unwrap();
        let triggers = board
            .write_triggers
            .iter()
            .map(|trigger| trigger.trigger.clone())
            .collect();
        let world = SimWorld::new();
        let tftp = SimTftp::new(&world, triggers);
        (world, tftp)
    }

    fn eeprom_gate() -> WriteGate {
        WriteGate::new(&WriteEnables {
            eeprom: true,
            ..WriteEnables::default()
        })
    }

    #[test]
    fn stage_refuses_a_trigger_file_without_a_permit() {
        let (world, mut tftp) = pi5_tftp();
        let payload = files(&[("config.txt", b"x"), ("PIEEPROM.UPD", b"image")]);
        let firmware = WriteGate::new(&WriteEnables {
            firmware: true,
            storage: true,
            ..WriteEnables::default()
        });
        let other_classes = [
            firmware.permit(WriteClass::Firmware).unwrap(),
            firmware.permit(WriteClass::Storage).unwrap(),
        ];
        for permits in [&[][..], &other_classes[..]] {
            assert_eq!(
                tftp.stage(&payload, permits),
                Err(StageError::Refused(WriteRefused::Unpermitted(vec![
                    Unpermitted {
                        file: name("PIEEPROM.UPD"),
                        class: WriteClass::Eeprom
                    }
                ])))
            );
        }
        // Nothing was staged: no generation exists, and the next payload
        // gets the first name.
        assert!(world.borrow().tftp.generations.is_empty());
        assert!(
            tftp.read_staged(&generation(1), &name("config.txt"))
                .is_err()
        );
        let staged = tftp.stage(&payload[..1], &[]).unwrap();
        assert_eq!(staged, generation(1));
    }

    #[test]
    fn stage_accepts_a_trigger_file_with_a_permit_of_its_class() {
        let (_, mut tftp) = pi5_tftp();
        let payload = files(&[("config.txt", b"x"), ("pieeprom.upd", b"image")]);
        let gate = eeprom_gate();
        let permits = [gate.permit(WriteClass::Eeprom).unwrap()];
        let staged = tftp.stage(&payload, &permits).unwrap();
        assert_eq!(
            tftp.read_staged(&staged, &name("pieeprom.upd")),
            Ok(b"image".to_vec())
        );
    }

    #[test]
    fn sim_tftp_stages_a_whole_new_generation_and_never_overwrites() {
        let (_, mut tftp) = pi5_tftp();
        let first = files(&[("config.txt", b"one"), ("overlays/a.dtbo", b"")]);
        let second = files(&[("config.txt", b"two")]);
        assert_eq!(tftp.stage(&first, &[]), Ok(generation(1)));
        assert_eq!(tftp.switch_to(&generation(1)), Ok(None));
        // The same payload again, and another one, each get a new name;
        // the served generation and its files stay as they were.
        assert_eq!(tftp.stage(&first, &[]), Ok(generation(2)));
        assert_eq!(tftp.stage(&second, &[]), Ok(generation(3)));
        for (file, bytes) in &first {
            assert_eq!(tftp.read_staged(&generation(1), file), Ok(bytes.clone()));
            assert_eq!(tftp.read_staged(&generation(2), file), Ok(bytes.clone()));
            assert_eq!(tftp.read_served(file), Ok(bytes.clone()));
        }
        // A generation holds exactly the files staged in it.
        assert_eq!(
            tftp.read_staged(&generation(3), &name("config.txt")),
            Ok(b"two".to_vec())
        );
        assert!(
            tftp.read_staged(&generation(3), &name("overlays/a.dtbo"))
                .is_err()
        );
        // Names that the TFTP server would serve as one file, and a file
        // that has the name of a directory, are refused, and nothing is
        // staged.
        let same = files(&[("config.txt", b"a"), ("CONFIG.TXT", b"b")]);
        let directory = files(&[("Overlays", b"a"), ("overlays/a.dtbo", b"b")]);
        for clash in [same, directory] {
            let error = tftp.stage(&clash, &[]).unwrap_err();
            assert!(
                matches!(&error, StageError::Device(error) if error.role == Role::Delivery),
                "{error}"
            );
        }
        assert_eq!(tftp.stage(&second, &[]), Ok(generation(4)));
    }

    #[test]
    fn read_served_reads_through_the_current_pointer() {
        let (_, mut tftp) = pi5_tftp();
        assert!(tftp.read_served(&name("config.txt")).is_err());
        tftp.stage(&files(&[("config.txt", b"one")]), &[]).unwrap();
        tftp.stage(&files(&[("config.txt", b"two")]), &[]).unwrap();
        // Staging serves nothing.
        assert!(tftp.read_served(&name("config.txt")).is_err());
        assert_eq!(tftp.switch_to(&generation(2)), Ok(None));
        assert_eq!(tftp.read_served(&name("config.txt")), Ok(b"two".to_vec()));
        assert_eq!(tftp.switch_to(&generation(1)), Ok(Some(generation(2))));
        assert_eq!(tftp.read_served(&name("config.txt")), Ok(b"one".to_vec()));
        assert!(tftp.read_served(&name("kernel.img")).is_err());
        assert!(tftp.switch_to(&generation(3)).is_err());
        assert_eq!(tftp.read_served(&name("config.txt")), Ok(b"one".to_vec()));
    }

    #[test]
    fn tftp_knobs_fail_or_change_their_step() {
        let (world, mut tftp) = pi5_tftp();
        let payload = files(&[("config.txt", b"one"), ("empty", b"")]);
        world.knobs().tftp.stage_fails = true;
        assert!(matches!(
            tftp.stage(&payload, &[]),
            Err(StageError::Device(_))
        ));
        assert!(world.borrow().tftp.generations.is_empty());
        world.knobs().tftp = TftpKnobs {
            corrupts_staged: true,
            ..TftpKnobs::default()
        };
        let staged = tftp.stage(&payload, &[]).unwrap();
        assert_eq!(
            tftp.read_staged(&staged, &name("config.txt")),
            Ok(b"nne".to_vec())
        );
        assert_eq!(tftp.read_staged(&staged, &name("empty")), Ok(vec![0]));
        world.knobs().tftp = TftpKnobs {
            switch_fails: true,
            ..TftpKnobs::default()
        };
        assert!(tftp.switch_to(&staged).is_err());
        assert!(tftp.read_served(&name("config.txt")).is_err());
        world.knobs().tftp = TftpKnobs {
            corrupts_served: true,
            ..TftpKnobs::default()
        };
        let clean = tftp.stage(&payload, &[]).unwrap();
        assert_eq!(
            tftp.read_staged(&clean, &name("config.txt")),
            Ok(b"one".to_vec())
        );
        tftp.switch_to(&clean).unwrap();
        assert_eq!(tftp.read_served(&name("config.txt")), Ok(b"nne".to_vec()));
    }
}
