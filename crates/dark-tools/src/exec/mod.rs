//! The `run_command` tool.
//!
//! This tool runs a command directly, the way `exec(3)` would, instead of
//! handing a whole line to a shell. Splitting the command in the harness,
//! rather than in a shell, keeps a shell metacharacter in the model's
//! argument text from ever being interpreted. A caller that truly needs a
//! shell — for a pipe, a redirect, or a glob — sets `shell = true`
//! explicitly, and that case needs a person present to confirm it.
//!
//! Submodules:
//!
//! - [`args`] parses and validates the JSON arguments.
//! - [`split`] tokenizes a command line the way a shell would, without
//!   invoking one.
//! - [`sandbox`] confines the working directory to the repository root.
//! - [`netns`] best-effort network isolation for dark mode, on Linux only.
//! - [`child`] spawns the process, streams its output, applies the timeout,
//!   and stops it. See its module documentation for the exact, honest
//!   guarantee that the process-group kill gives on each platform.
//! - [`cap`] caps the captured output, keeping the head and the tail.

mod args;
mod cap;
mod child;
mod netns;
mod sandbox;
mod split;

use std::time::Duration;

use async_trait::async_trait;
use dark_contract::tool::tier;
use dark_contract::{ErrCode, Error, Result, Tool, ToolCtx, ToolResult, ToolSchema};

use self::args::ExecArgs;

/// The `run_command` tool.
///
/// Tier 1: a below-8B model needs a way to run the test suite as much as it
/// needs to read and write a file. See task unit `C4`.
#[derive(Debug, Default)]
pub struct ExecTool;

impl ExecTool {
    /// Creates the tool.
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for ExecTool {
    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "run_command".to_owned(),
            description: "Runs a command. Splits the command line into a program \
                and its arguments. Does not use a shell unless shell is true, and \
                that case needs a person present. The working directory stays at \
                or below the repository root. The default timeout is 120 seconds."
                .to_owned(),
            parameters: args::schema(),
            tier: tier::ESSENTIAL,
            mutating: true,
        }
    }

    async fn invoke(&self, args: serde_json::Value, ctx: &ToolCtx) -> Result<ToolResult> {
        let args = ExecArgs::parse(args)?;

        if args.shell && !ctx.human_present {
            return Err(Error::new(
                ErrCode::PolicyConfirmRequired,
                "shell = true needs a person present to confirm it",
            )
            .with_remedy("Run this again with a person present, or split the command instead."));
        }

        let (program, prog_args) = if args.shell {
            // A person is present at this point. Log the shell run loudly, so
            // the louder confirmation that step 3 of task unit C3 asks for
            // shows up in the transcript even though this call site has no
            // synchronous confirm round trip to offer.
            ctx.events.notice(format!(
                "SHELL COMMAND (person confirmed): {}",
                args.command
            ));
            shell_argv(&args.command)
        } else {
            let mut words = split::split(&args.command)?;
            if words.is_empty() {
                return Err(Error::new(
                    ErrCode::ToolInvalidArgs,
                    "the command has no program to run",
                ));
            }
            let program = words.remove(0);
            (program, words)
        };

        let cwd = sandbox::resolve_cwd(&ctx.root, args.cwd.as_deref())?;
        let timeout = Duration::from_secs(args.timeout_secs);

        child::run(ctx, program, prog_args, cwd, timeout).await
    }
}

/// Returns the shell program and the flag that runs a string, for Unix.
#[cfg(unix)]
fn shell_argv(command: &str) -> (String, Vec<String>) {
    (
        "/bin/sh".to_owned(),
        vec!["-c".to_owned(), command.to_owned()],
    )
}

/// Returns the shell program and the flag that runs a string, for Windows.
#[cfg(windows)]
fn shell_argv(command: &str) -> (String, Vec<String>) {
    ("cmd".to_owned(), vec!["/C".to_owned(), command.to_owned()])
}

#[cfg(test)]
mod tests {
    use super::ExecTool;
    use dark_contract::{ErrCode, EventBus, Tool, ToolCtx};
    use serde_json::json;
    use tokio_util::sync::CancellationToken;

    fn ctx(dark: bool, human_present: bool) -> (EventBus, ToolCtx) {
        let bus = EventBus::new();
        let ctx = ToolCtx {
            root: std::env::temp_dir(),
            events: bus.tx(),
            cancel: CancellationToken::new(),
            dark,
            human_present,
        };
        (bus, ctx)
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_plain_command_runs_without_a_shell() {
        let (_bus, ctx) = ctx(false, true);
        let tool = ExecTool::new();
        let result = tool
            .invoke(json!({"command": "echo hello"}), &ctx)
            .await
            .unwrap();
        assert!(!result.is_error, "content: {}", result.content);
        assert!(result.content.contains("hello"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn shell_metacharacters_are_inert_without_shell_true() {
        // Without a shell, "|" is just an argument to `echo`, not a pipe.
        let (_bus, ctx) = ctx(false, true);
        let tool = ExecTool::new();
        let result = tool
            .invoke(json!({"command": "echo a | b"}), &ctx)
            .await
            .unwrap();
        assert!(!result.is_error);
        assert!(result.content.contains("a | b"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn shell_true_without_a_person_present_is_denied() {
        let (_bus, ctx) = ctx(false, false);
        let tool = ExecTool::new();
        let err = tool
            .invoke(json!({"command": "echo hi", "shell": true}), &ctx)
            .await
            .unwrap_err();
        assert_eq!(err.code, ErrCode::PolicyConfirmRequired);
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread")]
    async fn shell_true_with_a_person_present_uses_a_real_shell() {
        let (_bus, ctx) = ctx(false, true);
        let tool = ExecTool::new();
        // A pipe only means something to a shell. Success proves the shell ran.
        let result = tool
            .invoke(
                json!({"command": "echo hi | tr a-z A-Z", "shell": true}),
                &ctx,
            )
            .await
            .unwrap();
        assert!(!result.is_error, "content: {}", result.content);
        assert!(result.content.contains("HI"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_cwd_outside_the_root_is_rejected_before_spawning() {
        let (_bus, ctx) = ctx(false, true);
        let tool = ExecTool::new();
        let err = tool
            .invoke(
                json!({"command": "echo hi", "cwd": "../../../../etc"}),
                &ctx,
            )
            .await
            .unwrap_err();
        assert_eq!(err.code, ErrCode::ToolOutsideRoot);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn invalid_arguments_are_rejected_before_spawning() {
        let (_bus, ctx) = ctx(false, true);
        let tool = ExecTool::new();
        let err = tool.invoke(json!({}), &ctx).await.unwrap_err();
        assert_eq!(err.code, ErrCode::ToolInvalidArgs);
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread")]
    async fn a_result_over_the_cap_is_elided_with_head_and_tail() {
        let (_bus, ctx) = ctx(false, true);
        let tool = ExecTool::new();
        // 10000 lines, most five or six characters wide with the newline:
        // comfortably over the 30000-character cap.
        let result = tool
            .invoke(json!({"command": "seq 1 10000"}), &ctx)
            .await
            .unwrap();
        assert!(result.content.contains("elided"), "{}", result.content);
        assert!(result.content.contains("exit status: 0"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn the_schema_names_the_tool_run_command() {
        let tool = ExecTool::new();
        let schema = tool.schema();
        assert_eq!(schema.name, "run_command");
        assert!(schema.mutating);
        assert_eq!(schema.tier, dark_contract::tool::tier::ESSENTIAL);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_nonzero_exit_is_reported_as_a_tool_error_not_a_hard_failure() {
        // A failing test run is normal, useful information for the model,
        // not an infrastructure failure. It comes back as a `ToolResult`
        // with `is_error` set, not as `Err`, so the model sees the output.
        let (_bus, ctx) = ctx(false, true);
        let tool = ExecTool::new();
        let result = tool
            .invoke(json!({"command": "sh -c 'exit 3'"}), &ctx)
            .await
            .unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("exit status: 3"));
    }
}
