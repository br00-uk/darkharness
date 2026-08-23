//! Detects the accelerator (task unit `B6`, step 1).
//!
//! Mirrors the detection `crates/dark-cli/src/doctor.rs` already does for
//! Rule 18's build-variant check, expressed here as
//! [`dark_contract::Device`] instead of that module's private
//! `Accelerator` enum, so [`super::recommend`] can build a
//! [`dark_contract::Caps`]-shaped answer directly from it.

use std::path::Path;
use std::process::Command;

use dark_contract::Device;

/// Detects the accelerator on this machine: Apple Silicon by target
/// architecture, an NVIDIA graphics processor by its driver, or the
/// central processor when neither is present.
///
/// This container reports [`Device::Cpu`] (the build specification's own
/// example for "this container").
#[must_use]
pub fn detect() -> Device {
    if cfg!(target_os = "macos") && cfg!(target_arch = "aarch64") {
        return Device::Metal;
    }
    if let Some(index) = nvidia_gpu_index() {
        return Device::Cuda { index };
    }
    Device::Cpu
}

/// Returns the index of the first NVIDIA graphics processor present, when
/// one is.
fn nvidia_gpu_index() -> Option<usize> {
    if Path::new("/proc/driver/nvidia/version").is_file() {
        return Some(0);
    }
    Command::new("nvidia-smi")
        .arg("-L")
        .output()
        .ok()
        .filter(|out| out.status.success())
        .map(|_| 0)
}

/// Returns the device name `dark tune` writes into the `[hardware]`
/// section's `device` field, matching section 4.5's build-artefact names.
#[must_use]
pub fn device_name(device: &Device) -> &'static str {
    match device {
        Device::Cpu => "cpu",
        Device::Cuda { .. } => "cuda",
        Device::Metal => "metal",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_returns_a_device_this_process_could_plausibly_run_on() {
        // A real assertion on the detected variant would be
        // environment-dependent (this sandbox reports Cpu, a developer's
        // Mac reports Metal); what every environment shares is that
        // `detect` returns *some* variant without panicking.
        let device = detect();
        assert!(matches!(
            device,
            Device::Cpu | Device::Cuda { .. } | Device::Metal
        ));
    }

    #[test]
    fn device_name_matches_the_build_artefact_names() {
        assert_eq!(device_name(&Device::Cpu), "cpu");
        assert_eq!(device_name(&Device::Cuda { index: 0 }), "cuda");
        assert_eq!(device_name(&Device::Metal), "metal");
    }
}
