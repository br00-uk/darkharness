//! Best-effort network isolation for a command that runs in dark mode.
//!
//! The build specification asks the harness to run a dark-mode command in an
//! empty network namespace "where the platform permits it" (task unit `C3`,
//! step 9). `unsafe_code` is forbidden workspace-wide, so this module cannot
//! call `unshare(2)` directly — that needs a raw system call. Instead it
//! wraps the command with the `unshare` utility from `util-linux`, which is a
//! plain, safe subprocess launch.
//!
//! This is strictly best effort. The wrap silently does nothing when:
//!
//! - the target is not Linux,
//! - the `unshare` binary is not on `PATH`, or
//! - the sandbox denies `CLONE_NEWNET` (common in a container that lacks
//!   `CAP_SYS_ADMIN` and has no unprivileged user namespaces).
//!
//! `DARK_OFFLINE=1` in the child environment is the guarantee that dark mode
//! actually offers. The network namespace is a defence in depth on top of
//! that, not a substitute for it.

/// Wraps `program` and `args` to run inside an empty network namespace, when
/// the current platform and sandbox allow it.
///
/// Returns `(program, args)` unchanged when the wrap is unavailable.
#[cfg(target_os = "linux")]
pub(crate) async fn wrap(program: String, args: Vec<String>) -> (String, Vec<String>) {
    if !available().await {
        return (program, args);
    }
    let mut wrapped = Vec::with_capacity(args.len() + 3);
    wrapped.push("--net".to_owned());
    wrapped.push("--".to_owned());
    wrapped.push(program);
    wrapped.extend(args);
    ("unshare".to_owned(), wrapped)
}

/// Returns `(program, args)` unchanged. Network namespaces are a Linux-only
/// concept, so no other target attempts the wrap.
#[cfg(not(target_os = "linux"))]
pub(crate) async fn wrap(program: String, args: Vec<String>) -> (String, Vec<String>) {
    (program, args)
}

/// Caches whether `unshare --net` works in this sandbox.
///
/// The check spawns a real process, so the module runs it at most once per
/// harness process and reuses the answer after that.
#[cfg(target_os = "linux")]
async fn available() -> bool {
    static AVAILABLE: tokio::sync::OnceCell<bool> = tokio::sync::OnceCell::const_new();
    *AVAILABLE
        .get_or_init(|| async {
            tokio::process::Command::new("unshare")
                .args(["--net", "--", "true"])
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .await
                .map(|status| status.success())
                .unwrap_or(false)
        })
        .await
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::wrap;

    #[tokio::test]
    async fn a_wrap_attempt_never_panics_and_returns_a_runnable_command() {
        // The sandbox this runs in may or may not permit CLONE_NEWNET. Either
        // way the function must return something the caller can spawn.
        let (program, args) = wrap("true".to_owned(), Vec::new()).await;
        assert!(program == "true" || program == "unshare");
        if program == "unshare" {
            assert_eq!(args, vec!["--net", "--", "true"]);
        } else {
            assert!(args.is_empty());
        }
    }

    #[tokio::test]
    async fn an_unavailable_wrap_leaves_the_program_and_arguments_untouched() {
        // This does not assert unavailability (the test host might allow it),
        // only that the untouched path preserves both program and args.
        let (program, args) = wrap("echo".to_owned(), vec!["hi".to_owned()]).await;
        if program == "echo" {
            assert_eq!(args, vec!["hi".to_owned()]);
        }
    }
}
