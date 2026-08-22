//! Dark-mode signalling for child processes.
//!
//! `dark-airlock` does not spawn processes; task unit `C3` owns the command
//! tool. This module gives that tool one thing to call before it spawns a
//! process in dark mode: the environment variable to set.
//!
//! # This is advisory, on every platform
//!
//! Setting `DARK_OFFLINE=1` is a request, not a barrier. It asks a
//! cooperating child — git, cargo, npm, a language server — to skip its own
//! network calls. Nothing here stops an uncooperative or compromised child
//! from opening a socket anyway. This module creates no network namespace,
//! no seccomp filter, no firewall rule, and no process sandbox of any kind
//! on any platform.
//!
//! **macOS and Windows.** Neither platform gives a normal, unprivileged
//! process a lightweight way to strip network access from a child it
//! spawns. There is no comparable primitive to reach for. On these two
//! platforms the environment variable is the entire mechanism, and it will
//! stay that way: treat the child-process block as advisory there, and
//! expect it to remain advisory.
//!
//! **Linux.** The environment variable is advisory here too, today: this
//! crate applies no kernel-level restriction. Linux is the platform where a
//! future task unit could add real enforcement on top of this signal — a
//! network namespace, a seccomp-bpf filter, or an `unshare` wrapper around
//! the child — because the kernel primitives exist and are reachable from
//! an unprivileged process. No such enforcement exists yet. Until it does,
//! treat Linux the same as the other two: advisory only.

use std::process::Command;

/// The environment variable a child process checks for dark mode.
///
/// A cooperating child reads this variable and skips its own network calls
/// when it is set to `"1"`. See the module docs for what setting it does,
/// and does not, guarantee.
pub const DARK_OFFLINE_ENV: &str = "DARK_OFFLINE";

/// Returns the environment variables that dark mode sets for a child
/// process.
///
/// The list is empty when `dark` is `false`. Apply every pair to the
/// child's environment before it starts.
#[must_use]
pub fn child_env(dark: bool) -> Vec<(&'static str, &'static str)> {
    if dark {
        vec![(DARK_OFFLINE_ENV, "1")]
    } else {
        Vec::new()
    }
}

/// Applies the dark-mode environment to a [`std::process::Command`].
///
/// Call this before spawning the process. It is advisory; see the module
/// docs for exactly what that means on each platform.
pub fn apply_to_command(dark: bool, command: &mut Command) {
    for (key, value) in child_env(dark) {
        command.env(key, value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dark_mode_sets_dark_offline() {
        assert_eq!(child_env(true), vec![("DARK_OFFLINE", "1")]);
    }

    #[test]
    fn light_mode_sets_nothing() {
        assert!(child_env(false).is_empty());
    }

    #[test]
    fn apply_to_command_sets_the_variable_in_dark_mode() {
        let mut command = Command::new("dark-airlock-test-does-not-need-to-exist");
        apply_to_command(true, &mut command);
        let envs: Vec<_> = command.get_envs().collect();
        assert_eq!(
            envs,
            vec![(
                std::ffi::OsStr::new("DARK_OFFLINE"),
                Some(std::ffi::OsStr::new("1"))
            )]
        );
    }

    #[test]
    fn apply_to_command_sets_nothing_in_light_mode() {
        let mut command = Command::new("dark-airlock-test-does-not-need-to-exist");
        apply_to_command(false, &mut command);
        assert_eq!(command.get_envs().count(), 0);
    }
}
