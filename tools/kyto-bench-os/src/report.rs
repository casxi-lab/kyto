//! Report values and their JSON text, and the UTC time stamps that reports
//! record.
//!
//! A report is a `serde_json` value. The crate is built with its
//! `preserve_order` feature, so an object keeps the order in which its keys
//! were first inserted, and inserting a key that is present replaces its
//! value in place. [`to_json`] writes the pretty form of the crate, the form
//! of the receipt of `cargo xtask check`: nested values are indented by two
//! spaces, and the text ends in a newline.

use std::time::{SystemTime, UNIX_EPOCH};

pub use serde_json::{Map, Value};

/// The JSON text of `value` with two-space indentation and a final newline.
pub fn to_json(value: &Value) -> String {
    format!("{value:#}\n")
}

/// The UTC calendar fields of `time` as `(year, month, day, hour, minute,
/// second, microsecond)`; a time before 1970 counts as the start of 1970.
fn utc_fields(time: SystemTime) -> (i64, u32, u32, u64, u64, u64, u64) {
    let since = time.duration_since(UNIX_EPOCH).unwrap_or_default();
    let seconds = since.as_secs();
    let micros = u64::from(since.subsec_micros());
    let (year, month, day) = civil_from_days((seconds / 86_400) as i64);
    let rest = seconds % 86_400;
    (
        year,
        month,
        day,
        rest / 3600,
        rest % 3600 / 60,
        rest % 60,
        micros,
    )
}

/// `time` in UTC as RFC 3339 with six fraction digits and the offset `Z`,
/// such as `2026-10-14T07:45:17.000123Z`.
pub fn utc_text(time: SystemTime) -> String {
    let (y, mo, d, h, mi, s, us) = utc_fields(time);
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{s:02}.{us:06}Z")
}

/// `time` in UTC in the basic ISO 8601 form `%Y%m%dT%H%M%S.%fZ`, such as
/// `20261014T074517.000123Z`, for names of run directories.
pub fn utc_stamp_of(time: SystemTime) -> String {
    let (y, mo, d, h, mi, s, us) = utc_fields(time);
    format!("{y:04}{mo:02}{d:02}T{h:02}{mi:02}{s:02}.{us:06}Z")
}

/// The current time as [`utc_text`] writes it.
pub fn utc_now() -> String {
    utc_text(SystemTime::now())
}

/// The current time as [`utc_stamp_of`] writes it.
pub fn utc_stamp() -> String {
    utc_stamp_of(SystemTime::now())
}

/// The proleptic Gregorian date of a count of days since 1970-01-01.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * month_index + 2) / 5 + 1) as u32;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    } as u32;
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::time::Duration;

    #[test]
    fn a_report_keeps_its_key_order_and_two_space_indentation() {
        let report = json!({
            "schema_version": 1,
            "status": "PASS",
            "runs": [],
            "elapsed_seconds": 0.5,
            "note": "a\"b\\c\n\u{1}é",
            "inner": { "ok": true, "none": null },
        });
        assert_eq!(
            to_json(&report),
            "{\n  \"schema_version\": 1,\n  \"status\": \"PASS\",\n  \"runs\": [],\n  \"elapsed_seconds\": 0.5,\n  \"note\": \"a\\\"b\\\\c\\n\\u0001é\",\n  \"inner\": {\n    \"ok\": true,\n    \"none\": null\n  }\n}\n"
        );
    }

    #[test]
    fn inserting_a_present_key_replaces_its_value_in_place() {
        let mut report = Map::new();
        report.insert("a".into(), 1.into());
        report.insert("b".into(), 2.into());
        report.insert("a".into(), 3.into());
        assert_eq!(
            to_json(&Value::Object(report)),
            "{\n  \"a\": 3,\n  \"b\": 2\n}\n"
        );
    }

    #[test]
    fn timestamps_are_rfc_3339_and_basic_iso_8601_in_utc() {
        // 2026-10-14 is day 20,740 after 1970-01-01.
        let time =
            UNIX_EPOCH + Duration::new(20_740 * 86_400 + 7 * 3600 + 45 * 60 + 17, 123_456_789);
        assert_eq!(utc_text(time), "2026-10-14T07:45:17.123456Z");
        assert_eq!(utc_stamp_of(time), "20261014T074517.123456Z");
        let whole = UNIX_EPOCH + Duration::from_secs(951_782_400);
        assert_eq!(utc_text(whole), "2000-02-29T00:00:00.000000Z");
        assert_eq!(utc_text(UNIX_EPOCH), "1970-01-01T00:00:00.000000Z");
        let before = UNIX_EPOCH - Duration::from_secs(1);
        assert_eq!(utc_text(before), "1970-01-01T00:00:00.000000Z");
        assert_eq!(civil_from_days(20_740), (2026, 10, 14));
        assert_eq!(civil_from_days(-1), (1969, 12, 31));
        // The current time has the same shapes; only its value is unknown.
        assert_eq!(utc_now().len(), "2026-10-14T07:45:17.123456Z".len());
        assert!(utc_now().ends_with('Z'));
        assert_eq!(utc_stamp().len(), "20261014T074517.123456Z".len());
    }
}
