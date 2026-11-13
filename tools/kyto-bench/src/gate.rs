//! The write gate: payload files that let the board's firmware write
//! firmware, EEPROM contents or storage media are staged only when the run
//! plan entry of the gate enables their class, and a file that sets a
//! refused property is never staged. A delivery device writes only
//! [`Checked`] files, which only [`WriteGate::stage`] makes, from the write
//! triggers and refused properties of the gate's board description, so no
//! caller chooses what the files are checked against, and no other code can
//! make them:
//!
//! ```compile_fail
//! let checked = kyto_bench::gate::Checked(&[]);
//! ```
//!
//! Only this crate builds a gate, for the entry that a run stages:
//!
//! ```compile_fail
//! use kyto_bench_core::board::{Board, RASPBERRY_PI_5};
//! use kyto_bench_core::write::WriteEnables;
//!
//! let board = Board::parse(RASPBERRY_PI_5).unwrap();
//! let gate = kyto_bench::gate::WriteGate::new(&WriteEnables::default(), &board);
//! ```
//!
//! An entry's enable is needed for such a write, but it does not authorize
//! one: whoever runs a plan that enables a write decides that outside the
//! tool. No class or command exists for any other kind of write.

use std::fmt;

use kyto_bench_core::board::Board;
use kyto_bench_core::names::{GenerationName, PayloadName};
use kyto_bench_core::payload;
use kyto_bench_core::write::{self, Unpermitted, WriteEnables};

use crate::device::{Delivery, StageError};

/// The gate of one run plan entry on the board of its description.
#[derive(Debug)]
pub struct WriteGate<'b> {
    enables: WriteEnables,
    board: &'b Board,
}

/// A write that the gate refuses.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WriteRefused {
    /// Payload files that let the firmware write a class that the entry
    /// does not enable.
    Unpermitted(Vec<Unpermitted>),
    /// A payload file sets a refused property, which no enable permits.
    RefusedProperty(String),
}

impl<'b> WriteGate<'b> {
    /// The gate of an entry that enables `enables`, on `board`; a run makes
    /// one for each entry when it stages the entry's payload.
    pub(crate) fn new(enables: &WriteEnables, board: &'b Board) -> Self {
        let enables = *enables;
        Self { enables, board }
    }

    /// Stages `files` on `delivery` as a new generation and returns its
    /// name, once the gate has checked them: a file that is a write trigger
    /// of the board is staged only when the entry enables its class, and no
    /// file sets a refused property of the board. A refused file leaves the
    /// device untouched.
    pub fn stage(
        &self,
        delivery: &mut dyn Delivery,
        files: &[(PayloadName, Vec<u8>)],
    ) -> Result<GenerationName, StageError> {
        let refused = |refused| Err(StageError::Refused(refused));
        let (names, triggers) = (files.iter().map(|(name, _)| name), self.board.triggers());
        if let Err(unpermitted) = write::check_triggers(names, &triggers, &self.enables.classes()) {
            return refused(WriteRefused::Unpermitted(unpermitted));
        }
        let property = |(name, bytes): &(PayloadName, Vec<u8>)| {
            payload::refused_property(self.board, name, bytes)
        };
        if let Some(detail) = files.iter().find_map(property) {
            return refused(WriteRefused::RefusedProperty(detail));
        }
        let written = delivery.write_generation(Checked(files));
        written.map_err(StageError::Device)
    }
}

/// The files of a payload, each a name and its bytes, that the write gate
/// has checked. It has no public constructor: only [`WriteGate::stage`]
/// makes one, and hands it to the delivery device.
#[derive(Debug)]
pub struct Checked<'f>(&'f [(PayloadName, Vec<u8>)]);

impl<'f> Checked<'f> {
    /// The files.
    pub fn files(&self) -> &'f [(PayloadName, Vec<u8>)] {
        self.0
    }
}

impl fmt::Display for WriteRefused {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unpermitted(files) => {
                let files: Vec<String> = files.iter().map(ToString::to_string).collect();
                f.write_str(&files.join("; "))
            }
            Self::RefusedProperty(detail) => f.write_str(detail),
        }
    }
}

impl std::error::Error for WriteRefused {}

#[cfg(test)]
mod tests {
    use kyto_bench_core::board::RASPBERRY_PI_5;
    use kyto_bench_core::write::WriteClass;

    use super::*;
    use crate::sim::{SimTftp, SimWorld};

    fn pi5() -> Board {
        Board::parse(RASPBERRY_PI_5).unwrap()
    }

    #[test]
    fn the_gate_stages_only_the_files_it_checked_against_its_board() {
        let (board, world) = (pi5(), SimWorld::new());
        let mut tftp = SimTftp::new(&world);
        let files = [("config.txt", &b"x\n"[..]), ("PIEEPROM.UPD", b"image")]
            .map(|(name, bytes)| (PayloadName::parse(name).unwrap(), bytes.to_vec()));
        let first = GenerationName::new(1).unwrap();
        // Every class but the one of the trigger file.
        let others = WriteEnables {
            firmware: true,
            storage: true,
            ..WriteEnables::default()
        };
        for enables in [WriteEnables::default(), others] {
            let refused = WriteGate::new(&enables, &board).stage(&mut tftp, &files);
            let unpermitted = vec![Unpermitted {
                file: files[1].0.clone(),
                class: WriteClass::Eeprom,
            }];
            let expected = WriteRefused::Unpermitted(unpermitted);
            let lets = "PIEEPROM.UPD lets the firmware write eeprom, which is not enabled";
            assert!(expected.to_string().ends_with(lets), "{expected}");
            assert_eq!(refused, Err(StageError::Refused(expected)));
            assert!(tftp.read_staged(&first, &files[0].0).is_err());
        }
        let eeprom = WriteEnables {
            eeprom: true,
            ..WriteEnables::default()
        };
        let staged = WriteGate::new(&eeprom, &board).stage(&mut tftp, &files);
        assert_eq!(staged, Ok(first));
        assert_eq!(tftp.read_staged(&first, &files[1].0), Ok(b"image".to_vec()));
        // A refused property is refused whatever the entry enables.
        let all = WriteEnables {
            firmware: true,
            eeprom: true,
            storage: true,
        };
        let config = [(files[0].0.clone(), b"program_pubkey=1\n".to_vec())];
        let refused = WriteGate::new(&all, &board).stage(&mut tftp, &config);
        let Err(StageError::Refused(WriteRefused::RefusedProperty(detail))) = refused else {
            panic!("{refused:?}");
        };
        assert!(detail.contains("program_pubkey"), "{detail}");
        let second = GenerationName::new(2).unwrap();
        assert!(tftp.read_staged(&second, &files[0].0).is_err());
    }
}
