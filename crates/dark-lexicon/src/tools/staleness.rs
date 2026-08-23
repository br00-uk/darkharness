//! Pack staleness: how many days old a pack's ingest is, against its own
//! staleness policy.
//!
//! Task unit `G5` needs this for `docs_get`'s `pack: { age_days, stale }`
//! fields (the PRD's exact shape) and for the warning banner
//! [`super::get`] puts in the returned snippet text when a pack is stale.
//! `crate::pack::manifest::Staleness::policy` (`"90d"`) and
//! `crate::pack::manifest::Ingest::at` are the two inputs; neither the
//! pack module nor this crate's dependency list (Rule 16: no date-handling
//! crate) gives a ready answer, so [`days_from_civil`] and
//! [`civil_from_days`] hand-implement the one calendar conversion the
//! crate needs — Howard Hinnant's `days_from_civil` / `civil_from_days`
//! algorithms, pure integer arithmetic, proleptic Gregorian, published at
//! <http://howardhinnant.github.io/date_algorithms.html>.

use dark_contract::{ErrCode, Error, Result};

use crate::pack::PackManifest;

/// Parses a staleness policy string, for example `90d`, into a day count.
///
/// # Errors
///
/// Returns `E_TOOL_FAILED` when `policy` is not `<digits>d`. The manifest
/// sample task unit `G1` gives (and every fixture in this crate) only ever
/// uses this form, so a stricter shape is not invented here — see the
/// module's own report for this as a scoping note, not a resolved
/// contradiction: the specification never states a second unit.
pub fn parse_policy_days(policy: &str) -> Result<u32> {
    let digits = policy.strip_suffix('d').ok_or_else(|| {
        Error::new(
            ErrCode::ToolFailed,
            format!("staleness policy '{policy}' is not of the form '<N>d'"),
        )
    })?;
    digits.parse::<u32>().map_err(|source| {
        Error::new(
            ErrCode::ToolFailed,
            format!("staleness policy '{policy}' does not parse: {source}"),
        )
    })
}

/// Converts a civil (Gregorian) date to the day number since the Unix
/// epoch (1970-01-01 is day 0).
#[must_use]
pub fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let month = i64::from(month);
    let day = i64::from(day);
    let y = if month <= 2 { year - 1 } else { year };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400; // [0, 399]
    let mp = (month + 9) % 12; // Mar = 0 .. Feb = 11
    let doy = (153 * mp + 2) / 5 + day - 1; // [0, 365]
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // [0, 146096]
    era * 146_097 + doe - 719_468
}

/// Converts a day number since the Unix epoch back to a civil date: the
/// inverse of [`days_from_civil`].
#[must_use]
pub fn civil_from_days(z: i64) -> (i32, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    let year = if m <= 2 { y + 1 } else { y };
    (
        i32::try_from(year).unwrap_or(i32::MAX),
        u32::try_from(m).unwrap_or(0),
        u32::try_from(d).unwrap_or(0),
    )
}

/// Returns today's day number since the Unix epoch, read from the system
/// clock.
///
/// # Errors
///
/// Returns `E_TOOL_FAILED` when the system clock reads before the Unix
/// epoch.
pub fn today_epoch_day() -> Result<i64> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|source| {
            Error::new(
                ErrCode::ToolFailed,
                format!("system clock reads before the Unix epoch: {source}"),
            )
        })?;
    Ok(i64::try_from(now.as_secs() / 86400).unwrap_or(i64::MAX))
}

/// Computes how many days old `manifest`'s ingest is, and whether that age
/// exceeds its own staleness policy.
///
/// Returns `(age_days, stale)`.
///
/// # Errors
///
/// Returns `E_TOOL_FAILED` when `[ingest] at` carries no date, when
/// `[staleness] policy` does not parse, or when the system clock reads
/// before the Unix epoch.
pub fn evaluate(manifest: &PackManifest) -> Result<(u32, bool)> {
    let date = manifest.ingest.at.date.ok_or_else(|| {
        Error::new(
            ErrCode::ToolFailed,
            "pack.toml's [ingest] at carries no date",
        )
    })?;
    let ingest_day = days_from_civil(
        i64::from(date.year),
        u32::from(date.month),
        u32::from(date.day),
    );
    let today = today_epoch_day()?;
    let age_days = u32::try_from((today - ingest_day).max(0)).unwrap_or(u32::MAX);
    let policy_days = parse_policy_days(&manifest.staleness.policy)?;
    Ok((age_days, age_days > policy_days))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_unix_epoch_is_day_zero() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
    }

    #[test]
    fn the_year_two_thousand_is_the_well_known_reference_day() {
        assert_eq!(days_from_civil(2000, 1, 1), 10957);
    }

    #[test]
    fn the_manifest_samples_ingest_date_converts() {
        // 2026-08-19, the date `crates/dark-lexicon/src/pack/manifest.rs`'s
        // own sample manifest uses.
        let day = days_from_civil(2026, 8, 19);
        let (y, m, d) = civil_from_days(day);
        assert_eq!((y, m, d), (2026, 8, 19));
    }

    #[test]
    fn civil_from_days_reverses_days_from_civil_across_a_range_of_dates() {
        for day in -1000..3000 {
            let (y, m, d) = civil_from_days(day);
            assert_eq!(
                days_from_civil(i64::from(y), m, d),
                day,
                "round trip failed for day {day}"
            );
        }
    }

    #[test]
    fn a_leap_day_round_trips() {
        let day = days_from_civil(2024, 2, 29);
        assert_eq!(civil_from_days(day), (2024, 2, 29));
    }

    #[test]
    fn parse_policy_days_reads_the_sample_manifest_form() {
        assert_eq!(parse_policy_days("90d").unwrap(), 90);
        assert_eq!(parse_policy_days("1d").unwrap(), 1);
    }

    #[test]
    fn parse_policy_days_rejects_an_unrecognised_unit() {
        let err = parse_policy_days("90w").unwrap_err();
        assert_eq!(err.code, ErrCode::ToolFailed);
    }

    #[test]
    fn parse_policy_days_rejects_non_numeric_text() {
        assert!(parse_policy_days("stale").is_err());
        assert!(parse_policy_days("d").is_err());
    }

    fn manifest_with(policy: &str, year: u16, month: u8, day: u8) -> PackManifest {
        let toml_text = format!(
            r#"
[pack]
name = "examplelib"
version = "1.0.0"
ecosystem = "crates.io"

[source]
kind = "localdir"
url = "."

[ingest]
at = {year:04}-{month:02}-{day:02}T00:00:00Z
tool_version = "1.0.0"
chunker = "heading-v1"
chunks = 1

[embed]
model = "Qwen/Qwen3-Embedding-0.6B"
dim = 4
quant = "int8"

[staleness]
policy = "{policy}"

[license]
spdx = "MIT"
notice_required = true
"#
        );
        PackManifest::from_toml_str(&toml_text).expect("sample manifest parses")
    }

    #[test]
    fn a_pack_ingested_long_ago_is_stale() {
        let manifest = manifest_with("90d", 2000, 1, 1);
        let (age_days, stale) = evaluate(&manifest).unwrap();
        assert!(age_days > 90);
        assert!(stale);
    }

    #[test]
    fn a_pack_ingested_today_is_not_stale() {
        let today = today_epoch_day().unwrap();
        let (year, month, day) = civil_from_days(today);
        let manifest = manifest_with(
            "90d",
            u16::try_from(year).unwrap(),
            u8::try_from(month).unwrap(),
            u8::try_from(day).unwrap(),
        );
        let (age_days, stale) = evaluate(&manifest).unwrap();
        assert_eq!(age_days, 0);
        assert!(!stale);
    }

    #[test]
    fn evaluate_reports_tool_failed_for_an_unparseable_policy() {
        let manifest = manifest_with("90w", 2020, 1, 1);
        assert!(evaluate(&manifest).is_err());
    }
}
