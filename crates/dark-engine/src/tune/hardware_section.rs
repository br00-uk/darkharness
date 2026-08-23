//! The `[hardware]` section of the configuration file (task unit `B6`,
//! step 5).
//!
//! ```toml
//! [hardware]
//! device = "metal"
//! memory_total_gb = 36.0
//! memory_budget_gb = 26.0
//! measured_tok_s = { "qwen3-14b-q4" = 41.2 }
//! ```
//!
//! This module only builds and serialises the section; it does not decide
//! where in the configuration file it lives — that is `dark-config`'s
//! concern (task unit `J2`), which this crate does not own.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// The `[hardware]` section: what `dark tune` measured about this machine.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HardwareSection {
    /// `cpu`, `cuda`, or `metal` (see [`super::device::device_name`]).
    pub device: String,
    /// Total system memory, in gibibytes.
    pub memory_total_gb: f64,
    /// The memory `dark tune` measured as available to budget against
    /// (section 4.1's headroom already subtracted).
    pub memory_budget_gb: f64,
    /// Measured generation rate for each model/quantisation pair `dark
    /// tune` has measured, keyed by a label such as `qwen3-14b-q4`.
    ///
    /// A `BTreeMap` here, not a `HashMap`, so two writes of the same
    /// measurements serialise to byte-identical TOML: a hash map's
    /// iteration order is not stable across runs, and a config file a
    /// person diffs between two `dark tune` runs should not show noise
    /// from key reordering alone.
    pub measured_tok_s: BTreeMap<String, f32>,
}

impl HardwareSection {
    /// Serialises this section as TOML text, headed by `[hardware]`.
    ///
    /// # Errors
    ///
    /// Returns the underlying [`toml::ser::Error`] when serialisation
    /// fails. This should not happen for a struct built entirely from
    /// plain data, but the caller still gets a real error rather than a
    /// panic if it ever does.
    pub fn to_toml(&self) -> Result<String, toml::ser::Error> {
        toml::to_string_pretty(&TableWrapper { hardware: self })
    }
}

/// Wraps [`HardwareSection`] under a `hardware` key, so serialising it
/// produces a `[hardware]` table rather than bare top-level keys.
#[derive(Serialize)]
struct TableWrapper<'a> {
    hardware: &'a HardwareSection,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> HardwareSection {
        let mut measured_tok_s = BTreeMap::new();
        measured_tok_s.insert("qwen3-14b-q4".to_owned(), 41.2);
        HardwareSection {
            device: "metal".to_owned(),
            memory_total_gb: 36.0,
            memory_budget_gb: 26.0,
            measured_tok_s,
        }
    }

    #[test]
    fn to_toml_produces_a_hardware_table() {
        let text = sample().to_toml().unwrap();
        assert!(text.starts_with("[hardware]"));
        assert!(text.contains("device = \"metal\""));
        assert!(text.contains("memory_total_gb = 36.0"));
        assert!(text.contains("qwen3-14b-q4"));
    }

    #[test]
    fn to_toml_round_trips_through_a_generic_toml_value() {
        let text = sample().to_toml().unwrap();
        let table: toml::Table = text.parse().unwrap();
        let hardware = table.get("hardware").unwrap();
        assert_eq!(hardware.get("device").unwrap().as_str(), Some("metal"));
    }

    #[test]
    fn to_toml_is_byte_identical_across_two_calls() {
        // The BTreeMap ordering guarantee: repeated serialisation of the
        // same data must not shuffle the measured_tok_s keys.
        let section = sample();
        assert_eq!(section.to_toml().unwrap(), section.to_toml().unwrap());
    }
}
