//! Measures the machine and writes a profile (task unit `B6`).
//!
//! [`device`] detects the accelerator, [`memory`] reads system memory,
//! [`rate`] measures the generation rate against `&dyn Engine`, [`profile`]
//! classifies the machine and recommends a profile, and
//! [`hardware_section`] serialises the result as the `[hardware]` section
//! `dark tune` writes. Every one of those is real code exercised by a
//! test — this task unit needs no model file, only a machine (and this
//! container reports [`dark_contract::Device::Cpu`], the build
//! specification's own example for "this container").

pub mod device;
pub mod hardware_section;
pub mod memory;
pub mod profile;
pub mod rate;

pub use hardware_section::HardwareSection;
pub use memory::MemoryReading;
pub use profile::{HardwareClass, ProfileRecommendation};

use std::collections::BTreeMap;

use dark_contract::{Engine, Result, RoleClass};

/// The prompt `dark tune` sends to measure the generation rate.
///
/// Short and generic on purpose: this measures raw tokens-per-second, not
/// how well the model answers, so the content of the prompt does not
/// matter beyond being long enough to produce a real reply.
const MEASUREMENT_PROMPT: &str = "Write a short paragraph about the weather.";

/// A complete `dark tune` run: what was detected, measured, and
/// recommended.
#[derive(Debug, Clone, PartialEq)]
pub struct TuneReport {
    /// The device `dark tune` detected.
    pub device: dark_contract::Device,
    /// The memory `dark tune` read.
    pub memory: MemoryReading,
    /// The generation rate `dark tune` measured, when measurement ran.
    pub measured_tok_s: Option<f32>,
    /// The hardware class this run classified the machine as.
    pub class: HardwareClass,
    /// The recommended profile.
    pub recommendation: ProfileRecommendation,
}

impl TuneReport {
    /// Converts bytes to gibibytes, for display and for
    /// [`HardwareSection`].
    #[allow(clippy::cast_precision_loss)]
    fn bytes_to_gib(bytes: u64) -> f64 {
        bytes as f64 / (1024.0 * 1024.0 * 1024.0)
    }

    /// Builds the `[hardware]` section this run would write, labelling the
    /// measured rate with `model_label` (for example `qwen3-4b-q4k`).
    #[must_use]
    pub fn to_hardware_section(&self, model_label: &str) -> HardwareSection {
        let mut measured_tok_s = BTreeMap::new();
        if let Some(rate) = self.measured_tok_s {
            measured_tok_s.insert(model_label.to_owned(), rate);
        }
        HardwareSection {
            device: device::device_name(&self.device).to_owned(),
            memory_total_gb: Self::bytes_to_gib(self.memory.total_bytes),
            memory_budget_gb: Self::bytes_to_gib(self.memory.budget_bytes()),
            measured_tok_s,
        }
    }
}

/// Runs `dark tune`: detects the device, reads memory, measures the
/// generation rate against `engine`, classifies the machine, and
/// recommends a profile.
///
/// Pass `None` for `engine` to skip the live measurement — for example,
/// when no model is loaded yet and this run only needs to report the
/// device and memory. [`TuneReport::measured_tok_s`] is `None` in that
/// case, and [`profile::recommend`] still runs from the device and memory
/// alone.
///
/// # Errors
///
/// Returns an error when `engine` is given and the measurement fails.
pub async fn run(engine: Option<(&dyn Engine, RoleClass)>) -> Result<TuneReport> {
    let device = device::detect();
    let memory = memory::read();
    let memory_total_gb = TuneReport::bytes_to_gib(memory.total_bytes);
    let memory_budget_gb = TuneReport::bytes_to_gib(memory.budget_bytes());

    let measured_tok_s = match engine {
        Some((engine, class)) => Some(rate::measure(engine, class, MEASUREMENT_PROMPT).await?),
        None => None,
    };

    let class = profile::classify(&device, memory_budget_gb);
    let recommendation =
        profile::recommend(&device, memory_total_gb, memory_budget_gb, measured_tok_s);

    Ok(TuneReport {
        device,
        memory,
        measured_tok_s,
        class,
        recommendation,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use dark_engine_fake::FakeEngine;

    #[tokio::test]
    async fn run_with_no_engine_skips_measurement_but_still_recommends() {
        let report = run(None).await.unwrap();
        assert_eq!(report.measured_tok_s, None);
        assert!(!report.recommendation.model.is_empty());
    }

    #[tokio::test]
    async fn run_with_an_engine_measures_and_records_the_rate() {
        let engine = FakeEngine::with_replies(["a short reply here"]);
        let report = run(Some((&engine, RoleClass::Worker))).await.unwrap();
        assert!(report.measured_tok_s.is_some());
        assert_eq!(report.recommendation.expected_tok_s, report.measured_tok_s);
    }

    #[test]
    fn to_hardware_section_labels_the_measured_rate() {
        let report = TuneReport {
            device: dark_contract::Device::Cpu,
            memory: MemoryReading {
                total_bytes: 16 * 1024 * 1024 * 1024,
                available_bytes: 8 * 1024 * 1024 * 1024,
            },
            measured_tok_s: Some(3.4),
            class: HardwareClass::CpuOnly,
            recommendation: profile::recommend(&dark_contract::Device::Cpu, 16.0, 7.2, Some(3.4)),
        };
        let section = report.to_hardware_section("qwen3-4b-q4k");
        assert_eq!(section.device, "cpu");
        assert_eq!(section.measured_tok_s.get("qwen3-4b-q4k"), Some(&3.4));
    }

    #[test]
    fn to_hardware_section_records_no_rate_when_none_was_measured() {
        let report = TuneReport {
            device: dark_contract::Device::Cpu,
            memory: MemoryReading {
                total_bytes: 16 * 1024 * 1024 * 1024,
                available_bytes: 8 * 1024 * 1024 * 1024,
            },
            measured_tok_s: None,
            class: HardwareClass::CpuOnly,
            recommendation: profile::recommend(&dark_contract::Device::Cpu, 16.0, 7.2, None),
        };
        let section = report.to_hardware_section("qwen3-4b-q4k");
        assert!(section.measured_tok_s.is_empty());
    }
}
