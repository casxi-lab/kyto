//! The earliest bootloader release a board needs: the latest of the bounds
//! of its board description that apply to its identity record
//! (`docs/bench/formats.md`, minimum bootloader). The board's manufacturing
//! version, `min_boot_version` or else `manufacturing_version`, gives the
//! earliest release with that version or a later one; a change notice gives
//! a date for some values of a field, ignoring ASCII case. A bound whose
//! value the record does not give is undecided, and the minimum is then the
//! latest decided bound, marked undecided with every undecided bound listed.

use std::fmt;

use serde::Deserialize;

use crate::board::Board;
use crate::identity::{Field, FieldRef, IdentityRecord, Value};

/// The date of a bootloader release, written `YYYY/MM/DD` as the bootloader
/// prints it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize)]
#[serde(try_from = "String")]
pub struct ReleaseDate {
    year: u16,
    month: u8,
    day: u8,
}

/// Why a text is not a release date.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DateError {
    pub text: String,
}

impl ReleaseDate {
    /// The date, if it exists in the Gregorian calendar.
    pub fn new(year: u16, month: u8, day: u8) -> Option<Self> {
        let leap =
            year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400));
        let days = match month {
            1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
            4 | 6 | 9 | 11 => 30,
            2 if leap => 29,
            2 => 28,
            _ => return None,
        };
        (1..=days)
            .contains(&day)
            .then_some(Self { year, month, day })
    }

    /// Reads `YYYY/MM/DD`: four, two and two decimal digits.
    pub fn parse(text: &str) -> Result<Self, DateError> {
        let number = |range: std::ops::Range<usize>| {
            let digits = text
                .get(range)
                .filter(|part| part.bytes().all(|b| b.is_ascii_digit()))?;
            digits.parse::<u16>().ok()
        };
        let [year, month, day] = [0..4, 5..7, 8..10].map(number);
        let slashes =
            text.len() == 10 && text.get(4..5) == Some("/") && text.get(7..8) == Some("/");
        let date = match (year, month.map(u8::try_from), day.map(u8::try_from)) {
            (Some(year), Some(Ok(month)), Some(Ok(day))) if slashes => Self::new(year, month, day),
            _ => None,
        };
        date.ok_or_else(|| DateError { text: text.into() })
    }
}

impl fmt::Display for ReleaseDate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:04}/{:02}/{:02}", self.year, self.month, self.day)
    }
}

impl TryFrom<String> for ReleaseDate {
    type Error = DateError;

    fn try_from(text: String) -> Result<Self, Self::Error> {
        Self::parse(&text)
    }
}

impl fmt::Display for DateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = &self.text;
        write!(f, "{text:?} is not a date written YYYY/MM/DD that exists")
    }
}

impl std::error::Error for DateError {}

/// The earliest bootloader release a board needs, as far as its identity
/// record decides it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Minimum {
    /// The latest date of the bounds that the record decides and that apply;
    /// `None` when none does.
    pub at_least: Option<ReleaseDate>,
    /// Every bound that needs a value the record does not give, in the order
    /// of the board description, whether its date is before or after
    /// `at_least`. The minimum is decided when this is empty; otherwise it is
    /// `at_least` or later.
    pub undecided: Vec<Undecided>,
}

/// A bound that the identity record does not decide.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Undecided {
    /// The identity value the bound needs.
    pub field: FieldRef,
    /// The date the bound sets when it applies; `None` when the date depends
    /// on the value itself, such as an unknown manufacturing version, or the
    /// board description lists no release for it.
    pub at_least: Option<ReleaseDate>,
}

/// The earliest bootloader release that `board`'s rules give for the board
/// that `identity` describes. The record is one that
/// [`IdentityRecord::check`] accepts for the description.
pub fn minimum(board: &Board, identity: &IdentityRecord) -> Minimum {
    let rules = &board.min_bootloader;
    let mut decided: Vec<ReleaseDate> = Vec::new();
    let mut open: Vec<Undecided> = Vec::new();

    // `min_boot_version` and `manufacturing_version` are the same number,
    // which a checked record never gives as two different values.
    let version = [Field::MinBootVersion, Field::ManufacturingVersion]
        .into_iter()
        .map(FieldRef::whole)
        .find_map(|field| match identity.value(field) {
            Value::Number(version) => Some((field, *version)),
            _ => None,
        });
    match version {
        Some((field, version)) => {
            let first = rules
                .releases
                .iter()
                .find(|release| release.version >= version)
                .map(|release| release.date);
            match first {
                Some(date) => decided.push(date),
                None => open.push(Undecided {
                    field,
                    at_least: None,
                }),
            }
        }
        None => open.push(Undecided {
            field: FieldRef::whole(Field::MinBootVersion),
            at_least: None,
        }),
    }

    for bound in &rules.bounds {
        match identity.value(bound.field) {
            Value::Text(value) => {
                if bound
                    .any_of
                    .iter()
                    .any(|listed| listed.eq_ignore_ascii_case(value))
                {
                    decided.push(bound.at_least);
                }
            }
            _ => open.push(Undecided {
                field: bound.field,
                at_least: Some(bound.at_least),
            }),
        }
    }

    Minimum {
        at_least: decided.into_iter().max(),
        undecided: open,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::{self, Board};

    /// The identity template with the values of the given keys, one line
    /// each, filled in.
    fn identity(values: &[(&str, &str)]) -> IdentityRecord {
        let mut text = String::new();
        let mut filled = 0;
        for line in include_str!("../../templates/identity.toml").lines() {
            let key = line.split_whitespace().next().unwrap_or_default();
            match values.iter().find(|(wanted, _)| *wanted == key) {
                Some((_, value)) => {
                    filled += 1;
                    text.push_str(&line.replacen("\"unknown\"", value, 1));
                }
                None => text.push_str(line),
            }
            text.push('\n');
        }
        assert_eq!(filled, values.len(), "the template has every key");
        IdentityRecord::parse(&text).unwrap()
    }

    fn pi5() -> Board {
        Board::parse(board::RASPBERRY_PI_5).unwrap()
    }

    fn date(text: &str) -> ReleaseDate {
        ReleaseDate::parse(text).unwrap()
    }

    fn field(text: &str) -> FieldRef {
        FieldRef::parse(text).unwrap()
    }

    #[test]
    fn minimum_bootloader_takes_the_latest_bound() {
        let board = pi5();
        // PCB revision 4 from March 2024; manufacturing version 1 from the
        // release of 2026/05/11; an alternative SPI flash part from
        // 2026/05/05.
        let record = identity(&[
            ("pcb_version", "\"4\""),
            ("min_boot_version", "1"),
            ("spi_flash.part", "\"W25Q16RVXHJQ\""),
        ]);
        let found = minimum(&board, &record);
        assert_eq!(found.at_least, Some(date("2026/05/11")));
        // The memory part is unknown and its bound, 2026/09/25, is later.
        assert_eq!(
            found.undecided,
            [Undecided {
                field: field("memory.part"),
                at_least: Some(date("2026/09/25"))
            }]
        );
        assert!(!found.undecided.is_empty());

        // PCB revision 7 and an alternative memory part from 2026/09/25 are
        // later than manufacturing version 1; manufacturing version 3 is
        // later still.
        let record = identity(&[
            ("pcb_version", "\"7\""),
            ("min_boot_version", "1"),
            ("memory.vendor", "\"Rayson\""),
            ("memory.part", "\"RS2G32LR4D4BNR-46BT\""),
        ]);
        let found = minimum(&board, &record);
        assert_eq!(found.at_least, Some(date("2026/09/25")));
        // The SPI flash part is unknown: its bound is listed though its date
        // is earlier than the decided one.
        assert_eq!(
            found.undecided,
            [Undecided {
                field: field("spi_flash.part"),
                at_least: Some(date("2026/05/05"))
            }]
        );
        // A part number applies its bound in any ASCII case.
        let record = identity(&[
            ("pcb_version", "\"3\""),
            ("min_boot_version", "1"),
            ("memory.part", "\"rs2g32lr4d4bnr-46bt\""),
            ("spi_flash.part", "\"another-part\""),
        ]);
        assert_eq!(
            minimum(&board, &record),
            Minimum {
                at_least: Some(date("2026/09/25")),
                undecided: Vec::new()
            }
        );
        let record = identity(&[
            ("pcb_version", "\"7\""),
            ("min_boot_version", "3"),
            ("memory.part", "\"RS2G32LR4D4BNR-46BT\""),
            ("spi_flash.part", "\"GD25Q16EEIGR\""),
        ]);
        assert_eq!(
            minimum(&board, &record),
            Minimum {
                at_least: Some(date("2026/10/23")),
                undecided: Vec::new()
            }
        );
    }

    #[test]
    fn the_min_boot_version_gives_the_first_release_of_that_version_or_later() {
        let board = pi5();
        for (version, first) in [
            ("1", "2026/05/11"),
            ("2", "2026/09/23"),
            ("3", "2026/10/23"),
        ] {
            let record = identity(&[
                ("pcb_version", "\"3\""),
                ("min_boot_version", version),
                ("memory.part", "\"another-part\""),
            ]);
            let found = minimum(&board, &record);
            assert_eq!(found.at_least, Some(date(first)), "version {version}");
        }
        // A version later than every release the description lists cannot be
        // given a date.
        let record = identity(&[("min_boot_version", "4")]);
        let found = minimum(&board, &record);
        assert!(found.undecided.contains(&Undecided {
            field: field("min_boot_version"),
            at_least: None
        }));
    }

    #[test]
    fn the_manufacturing_version_stands_in_for_an_unknown_min_boot_version() {
        let board = pi5();
        let record = identity(&[
            ("pcb_version", "\"3\""),
            ("manufacturing_version", "2"),
            ("memory.part", "\"another-part\""),
            ("spi_flash.part", "\"another-part\""),
        ]);
        assert_eq!(
            minimum(&board, &record),
            Minimum {
                at_least: Some(date("2026/09/23")),
                undecided: Vec::new()
            }
        );
        // When both are known, they are equal in a checked record.
        let record = identity(&[
            ("pcb_version", "\"3\""),
            ("manufacturing_version", "3"),
            ("min_boot_version", "3"),
            ("memory.part", "\"another-part\""),
        ]);
        record.check(&board).unwrap();
        assert_eq!(minimum(&board, &record).at_least, Some(date("2026/10/23")));
        // A version above every listed release is undecided under the field
        // that gave it.
        let record = identity(&[("manufacturing_version", "4")]);
        assert!(minimum(&board, &record).undecided.contains(&Undecided {
            field: field("manufacturing_version"),
            at_least: None
        }));
    }

    #[test]
    fn an_unknown_part_makes_its_bound_undecided() {
        let board = pi5();
        let found = minimum(&board, &identity(&[]));
        assert_eq!(found.at_least, None);
        assert_eq!(
            found.undecided,
            [
                Undecided {
                    field: field("min_boot_version"),
                    at_least: None
                },
                Undecided {
                    field: field("pcb_version"),
                    at_least: Some(date("2024/03/01"))
                },
                Undecided {
                    field: field("pcb_version"),
                    at_least: Some(date("2026/09/25"))
                },
                Undecided {
                    field: field("memory.part"),
                    at_least: Some(date("2026/09/25"))
                },
                Undecided {
                    field: field("spi_flash.part"),
                    at_least: Some(date("2026/05/05"))
                },
            ]
        );
        // Bounds whose dates are earlier than the decided date are listed
        // too: the result is undecided until every bound is decided.
        let found = minimum(
            &board,
            &identity(&[("min_boot_version", "3"), ("pcb_version", "\"7\"")]),
        );
        assert_eq!(found.at_least, Some(date("2026/10/23")));
        assert_eq!(
            found.undecided,
            [
                Undecided {
                    field: field("memory.part"),
                    at_least: Some(date("2026/09/25"))
                },
                Undecided {
                    field: field("spi_flash.part"),
                    at_least: Some(date("2026/05/05"))
                },
            ]
        );
    }

    #[test]
    fn a_value_outside_a_bound_does_not_apply_it() {
        let board = pi5();
        let record = identity(&[
            ("pcb_version", "\"3\""),
            ("min_boot_version", "1"),
            ("memory.vendor", "\"Micron\""),
            ("memory.part", "\"another-part\""),
            ("spi_flash.part", "\"another-part\""),
        ]);
        let found = minimum(&board, &record);
        assert_eq!(found.at_least, Some(date("2026/05/11")));
        assert!(found.undecided.is_empty(), "{:?}", found.undecided);
    }

    #[test]
    fn release_dates_are_written_year_month_day() {
        assert_eq!(date("2026/09/25").to_string(), "2026/09/25");
        assert!(date("2024/03/01") < date("2026/05/05"));
        assert!(date("2026/05/05") < date("2026/05/11"));
        assert_eq!(date("2024/02/29"), ReleaseDate::new(2024, 2, 29).unwrap());
        for text in [
            "2026-09-25",
            "2026/9/25",
            "26/09/25",
            "2026/13/01",
            "2026/00/10",
            "2026/02/29",
            "2026/04/31",
            "2026/09/+5",
            "2026/09/25 ",
            "",
        ] {
            assert!(ReleaseDate::parse(text).is_err(), "{text:?}");
        }
        assert_eq!(ReleaseDate::new(2100, 2, 29), None);
        assert!(ReleaseDate::new(2000, 2, 29).is_some());
    }
}
