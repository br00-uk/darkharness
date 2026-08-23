//! Classifies the machine and recommends a profile (task unit `B6`,
//! steps 4 and 6).
//!
//! The exact model names, quantisations, and context lengths below are
//! this harness's starting recommendations, in the sense Appendix C
//! describes: examples to verify against the model card and the loaded
//! model, not a promise this document keeps updated as models change.
//! `dark tune` exists precisely so this table does not have to stay
//! correct by itself.

use dark_contract::Device;

/// `24 GiB` in gibibytes, as an `f64` for comparison against
/// [`HardwareSection`](super::hardware_section::HardwareSection)'s
/// `memory_total_gb`. Rule 1's line: below this, the architect, worker,
/// and scout role classes share one resident model.
const SHARED_MODEL_THRESHOLD_GB: f64 = 24.0;

/// A coarse hardware class for `dark doctor`/`dark tune` to report
/// (Rule 9).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HardwareClass {
    /// No accelerator. Rule 10's default profile applies.
    CpuOnly,
    /// An accelerator with less than 8 GiB of budget.
    EntryGpu,
    /// An accelerator with 8 GiB up to 16 GiB of budget.
    MidGpu,
    /// An accelerator with 16 GiB up to 24 GiB of budget.
    HighGpu,
    /// An accelerator with 24 GiB of budget or more.
    Workstation,
}

impl HardwareClass {
    /// Returns the label `dark tune` and `dark doctor` print.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::CpuOnly => "central processor only",
            Self::EntryGpu => "entry graphics processor",
            Self::MidGpu => "mid-range graphics processor",
            Self::HighGpu => "high-end graphics processor",
            Self::Workstation => "workstation-class graphics processor",
        }
    }
}

/// Classifies the machine from its device and memory budget (Rule 9).
#[must_use]
pub fn classify(device: &Device, memory_budget_gb: f64) -> HardwareClass {
    if matches!(device, Device::Cpu) {
        return HardwareClass::CpuOnly;
    }
    if memory_budget_gb < 8.0 {
        HardwareClass::EntryGpu
    } else if memory_budget_gb < 16.0 {
        HardwareClass::MidGpu
    } else if memory_budget_gb < 24.0 {
        HardwareClass::HighGpu
    } else {
        HardwareClass::Workstation
    }
}

/// What `dark tune` recommends: the model, the quantisation, the context,
/// the expected rate, and whether the role classes share one model
/// (task unit `B6`, step 6).
#[derive(Debug, Clone, PartialEq)]
pub struct ProfileRecommendation {
    /// The recommended model repository.
    pub model: String,
    /// The recommended quantisation.
    pub quant: String,
    /// The recommended context length.
    pub context: usize,
    /// The generation rate to expect, when a measurement is available.
    pub expected_tok_s: Option<f32>,
    /// Whether the architect, worker, and scout role classes share one
    /// resident model (Rule 1).
    pub share_role_classes: bool,
    /// Whether thinking is on by default for this profile (Rule 10: off
    /// for the central-processor default).
    pub thinking: bool,
    /// The round-trip limit for this profile (Rule 10: 12 for the
    /// central-processor default).
    pub round_trip_limit: usize,
}

/// Recommends a profile from the machine's class and (when `dark tune` has
/// measured one) generation rate.
#[must_use]
pub fn recommend(
    device: &Device,
    memory_total_gb: f64,
    memory_budget_gb: f64,
    measured_tok_s: Option<f32>,
) -> ProfileRecommendation {
    let class = classify(device, memory_budget_gb);
    let share_role_classes = memory_total_gb < SHARED_MODEL_THRESHOLD_GB;

    let (model, quant, context, thinking, round_trip_limit) = match class {
        // Rule 10: central-processor default is a 4B model, thinking off,
        // round-trip limit 12.
        HardwareClass::CpuOnly => ("Qwen/Qwen3-4B", "q4k", 8192, false, 12),
        HardwareClass::EntryGpu => ("Qwen/Qwen3-4B", "q4k", 16_384, true, 24),
        HardwareClass::MidGpu => ("Qwen/Qwen3-8B", "q4k", 32_768, true, 24),
        HardwareClass::HighGpu => ("Qwen/Qwen3-14B", "q4k", 32_768, true, 32),
        HardwareClass::Workstation => ("Qwen/Qwen3-32B", "q4k", 32_768, true, 32),
    };

    ProfileRecommendation {
        model: model.to_owned(),
        quant: quant.to_owned(),
        context,
        expected_tok_s: measured_tok_s,
        share_role_classes,
        thinking,
        round_trip_limit,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpu_only_is_classified_regardless_of_reported_memory() {
        // A CPU-only machine can still report a lot of RAM; the class is
        // about the accelerator, not the memory.
        assert_eq!(classify(&Device::Cpu, 64.0), HardwareClass::CpuOnly);
    }

    #[test]
    fn gpu_classes_follow_the_budget_tiers() {
        let cuda = Device::Cuda { index: 0 };
        assert_eq!(classify(&cuda, 4.0), HardwareClass::EntryGpu);
        assert_eq!(classify(&cuda, 12.0), HardwareClass::MidGpu);
        assert_eq!(classify(&cuda, 20.0), HardwareClass::HighGpu);
        assert_eq!(classify(&cuda, 32.0), HardwareClass::Workstation);
    }

    #[test]
    fn boundaries_round_down_to_the_lower_tier() {
        let cuda = Device::Cuda { index: 0 };
        assert_eq!(classify(&cuda, 8.0), HardwareClass::MidGpu);
        assert_eq!(classify(&cuda, 16.0), HardwareClass::HighGpu);
        assert_eq!(classify(&cuda, 24.0), HardwareClass::Workstation);
    }

    #[test]
    fn cpu_only_recommends_the_rule_10_default() {
        let rec = recommend(&Device::Cpu, 16.0, 12.0, None);
        assert_eq!(rec.model, "Qwen/Qwen3-4B");
        assert!(!rec.thinking, "Rule 10: thinking is off on the CPU default");
        assert_eq!(
            rec.round_trip_limit, 12,
            "Rule 10: the round-trip limit is 12"
        );
    }

    #[test]
    fn below_24_gib_shares_the_role_classes() {
        let rec = recommend(&Device::Cpu, 16.0, 12.0, None);
        assert!(rec.share_role_classes, "Rule 1");
    }

    #[test]
    fn at_or_above_24_gib_does_not_share_the_role_classes() {
        let cuda = Device::Cuda { index: 0 };
        let rec = recommend(&cuda, 32.0, 28.0, None);
        assert!(!rec.share_role_classes);
    }

    #[test]
    fn the_measured_rate_passes_through_untouched() {
        let rec = recommend(&Device::Cpu, 16.0, 12.0, Some(3.4));
        assert_eq!(rec.expected_tok_s, Some(3.4));
    }

    #[test]
    fn a_workstation_gpu_recommends_the_largest_profile() {
        let cuda = Device::Cuda { index: 0 };
        let rec = recommend(&cuda, 64.0, 48.0, None);
        assert_eq!(rec.model, "Qwen/Qwen3-32B");
        assert!(rec.thinking);
    }

    #[test]
    fn hardware_class_labels_are_stable() {
        assert_eq!(HardwareClass::CpuOnly.label(), "central processor only");
        assert_eq!(
            HardwareClass::Workstation.label(),
            "workstation-class graphics processor"
        );
    }
}
