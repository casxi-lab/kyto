//! The write gate: payload files that let the board's firmware write
//! firmware, EEPROM contents or storage media are staged only with a permit
//! of their class, and only the gate of a run plan entry that enables the
//! class gives one. Only this crate builds a gate, so no code outside it can
//! make a permit, and a permit borrows the gate that gave it, so it cannot
//! outlive that gate. No command runs a run plan yet, so only tests build a
//! gate. [`crate::device::Delivery::stage`] checks every payload file before
//! a delivery device writes anything.
//!
//! An entry's enable is needed for such a write, but it does not authorize
//! one: whoever runs a plan that enables a write decides that outside the
//! tool. No class, permit or command exists for any other kind of write.

use std::fmt;
use std::marker::PhantomData;

use kyto_bench_core::names::PayloadName;
use kyto_bench_core::write::{self, Unpermitted, WriteClass, WriteEnables, WriteTrigger};

/// The permission to stage payload files that let the firmware write one
/// class. It has no public constructor: only [`WriteGate::permit`] makes
/// one, for a class that the entry enables, and it lives no longer than the
/// gate `'g` that made it.
#[derive(Debug, PartialEq, Eq)]
pub struct WritePermit<'g> {
    class: WriteClass,
    gate: PhantomData<&'g WriteGate>,
}

impl WritePermit<'_> {
    /// The class the permit covers.
    pub fn class(&self) -> WriteClass {
        self.class
    }
}

/// The gate of one run plan entry.
#[derive(Debug)]
pub struct WriteGate {
    enables: WriteEnables,
}

/// A write that the gate refuses.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WriteRefused {
    /// The entry does not enable the class.
    NotEnabled(WriteClass),
    /// Payload files that let the firmware write a class that no permit
    /// covers.
    Unpermitted(Vec<Unpermitted>),
}

impl WriteGate {
    /// The gate of an entry that enables `enables`.
    #[cfg(test)]
    pub(crate) fn new(enables: &WriteEnables) -> Self {
        Self { enables: *enables }
    }

    /// A permit for `class`, if the entry enables it.
    pub fn permit(&self, class: WriteClass) -> Result<WritePermit<'_>, WriteRefused> {
        if self.enables.enabled(class) {
            Ok(WritePermit {
                class,
                gate: PhantomData,
            })
        } else {
            Err(WriteRefused::NotEnabled(class))
        }
    }
}

/// Checks the files of a payload against the board's write `triggers`
/// before any of them is written: a file that lets the firmware write a
/// class needs a permit of that class. [`crate::device::Delivery::stage`]
/// calls this before it hands the files to the device.
pub fn check_staging<'a>(
    files: impl IntoIterator<Item = &'a PayloadName>,
    triggers: &[WriteTrigger],
    permits: &[WritePermit<'_>],
) -> Result<(), WriteRefused> {
    let permitted: Vec<WriteClass> = permits.iter().map(WritePermit::class).collect();
    write::check_triggers(files, triggers, &permitted).map_err(WriteRefused::Unpermitted)
}

impl fmt::Display for WriteRefused {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotEnabled(class) => {
                write!(f, "the run plan entry does not enable writes of {class}")
            }
            Self::Unpermitted(files) => {
                let files: Vec<String> = files.iter().map(ToString::to_string).collect();
                f.write_str(&files.join("; "))
            }
        }
    }
}

impl std::error::Error for WriteRefused {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_permit_is_given_only_for_an_enabled_class() {
        let none = WriteGate::new(&WriteEnables::default());
        for class in WriteClass::ALL {
            assert_eq!(none.permit(class), Err(WriteRefused::NotEnabled(class)));
        }
        let eeprom = WriteGate::new(&WriteEnables {
            eeprom: true,
            ..WriteEnables::default()
        });
        assert_eq!(
            eeprom
                .permit(WriteClass::Eeprom)
                .map(|permit| permit.class()),
            Ok(WriteClass::Eeprom)
        );
        assert!(eeprom.permit(WriteClass::Firmware).is_err());
        assert!(eeprom.permit(WriteClass::Storage).is_err());
        assert_eq!(
            none.permit(WriteClass::Storage).unwrap_err().to_string(),
            "the run plan entry does not enable writes of storage"
        );
    }

    #[test]
    fn a_permit_covers_only_its_own_class() {
        let trigger = |class, file: &str| WriteTrigger::new(class, vec![file.to_string()]);
        let triggers = [
            trigger(WriteClass::Eeprom, "update.bin").unwrap(),
            trigger(WriteClass::Storage, "disk.img").unwrap(),
        ];
        let files =
            ["update.bin", "disk.img", "notes.txt"].map(|name| PayloadName::parse(name).unwrap());
        let gate = WriteGate::new(&WriteEnables {
            eeprom: true,
            ..WriteEnables::default()
        });
        let permits = [gate.permit(WriteClass::Eeprom).unwrap()];
        let refused = check_staging(&files, &triggers, &permits).unwrap_err();
        assert_eq!(
            refused,
            WriteRefused::Unpermitted(vec![Unpermitted {
                file: files[1].clone(),
                class: WriteClass::Storage
            }])
        );
        assert_eq!(
            refused.to_string(),
            "the payload file disk.img lets the firmware write storage, which is not enabled"
        );
        assert_eq!(check_staging(&files[..1], &triggers, &permits), Ok(()));
        assert!(check_staging(&files[..1], &triggers, &[]).is_err());
    }
}
