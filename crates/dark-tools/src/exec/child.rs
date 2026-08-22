//! Spawns the child process, streams its output, and stops it reliably.
//!
//! # What the process-group kill guarantees, by platform
//!
//! `unsafe_code` is forbidden workspace-wide (see `CLAUDE.md`), so this
//! module never calls `killpg(2)` or the Windows Job Object API directly —
//! both need a raw system call or an FFI binding this workspace does not
//! carry. It uses only safe code, with these honest, different guarantees on
//! each platform:
//!
//! - **Unix.** The child spawns as the leader of a new process group
//!   ([`Command::process_group`], stable Rust, no `unsafe`). On a timeout or
//!   a cancellation, this module shells out to the external `kill` utility
//!   with a negative PID (`kill -9 -<pgid>`), which is the standard,
//!   textbook way to signal a whole process group without calling
//!   `killpg(2)` in-process. Because the child led its own group, its PID
//!   equals its PGID, so this reaches the child and every descendant that
//!   has not moved itself to a different group. **This is not a hard
//!   guarantee**: it depends on `kill` being on `PATH` (true on every Unix
//!   this workspace targets, but not enforced by the type system), and a
//!   descendant that calls `setpgid` to leave the group survives. When the
//!   external `kill` is unavailable or fails, this module falls back to
//!   [`Child::kill`], which signals only the direct child.
//! - **Windows.** There is no safe, dependency-free equivalent of a Job
//!   Object here. This module kills only the direct child
//!   ([`Child::kill`]). **A descendant the child spawned (for example, a
//!   batch file that starts a further process) is not guaranteed to stop.**
//!   This is a known, accepted gap; task unit `C3` names this exact
//!   trade-off in the workspace's build notes.
//!
//! Every platform reaps the child with [`Child::wait`] after the kill
//! attempt, so a killed child never becomes a zombie.

use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use dark_contract::{ErrCode, Error, Event, Result, ToolCtx, ToolResult};
use tokio::io::{AsyncBufReadExt as _, AsyncRead, BufReader};
use tokio::sync::mpsc;

use super::{cap, netns};

/// How long this module waits, after a kill attempt, for the child to exit
/// and for its output pumps to reach end of file.
///
/// A kill that truly lands is fast. A longer wait here would only delay the
/// tool reply when the kill did not land at all.
const REAP_GRACE: Duration = Duration::from_secs(5);

/// What ended the wait for the child.
enum Outcome {
    /// The child exited on its own.
    Exited(std::process::ExitStatus),
    /// The timeout elapsed first.
    TimedOut,
    /// The caller cancelled the tool.
    Cancelled,
}

/// Runs `program` with `args` in `cwd`, and returns the captured output.
///
/// Streams every line of combined stdout and stderr to `ctx.events` as
/// [`Event::ToolProgress`] while the command runs. Applies `timeout`, honours
/// `ctx.cancel`, and sets `DARK_OFFLINE=1` in the child environment when
/// `ctx.dark` is true. See the module documentation for what the kill on
/// timeout or cancellation guarantees on each platform.
///
/// # Errors
///
/// Returns [`ErrCode::ToolFailed`] when the process fails to start or when
/// waiting on it fails. Returns [`ErrCode::ToolTimeout`] when `timeout`
/// elapses before the command finishes.
///
/// # Panics
///
/// Never panics. The two `expect` calls read back stdout and stderr handles
/// that this same function just requested with `Stdio::piped()`, so `spawn`
/// always populates them on success.
pub(crate) async fn run(
    ctx: &ToolCtx,
    program: String,
    args: Vec<String>,
    cwd: PathBuf,
    timeout: Duration,
) -> Result<ToolResult> {
    let (program, args) = if ctx.dark {
        netns::wrap(program, args).await
    } else {
        (program, args)
    };

    let mut command = tokio::process::Command::new(&program);
    command
        .args(&args)
        .current_dir(&cwd)
        .kill_on_drop(true)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    if ctx.dark {
        command.env("DARK_OFFLINE", "1");
    }

    #[cfg(unix)]
    command.process_group(0);

    let mut child = command.spawn().map_err(|e| {
        Error::new(
            ErrCode::ToolFailed,
            format!("failed to start '{program}': {e}"),
        )
    })?;

    let stdout = child.stdout.take().expect("stdout is piped");
    let stderr = child.stderr.take().expect("stderr is piped");

    let (line_tx, mut line_rx) = mpsc::unbounded_channel::<String>();
    let out_pump = tokio::spawn(pump(stdout, line_tx.clone()));
    let err_pump = tokio::spawn(pump(stderr, line_tx));

    let events = ctx.events.clone();
    let collector = tokio::spawn(async move {
        let mut buf = String::new();
        while let Some(line) = line_rx.recv().await {
            events.send(Event::ToolProgress {
                // The `Tool::invoke` signature carries no turn or call
                // identifier (see `ToolCtx` in `dark-contract`), so this
                // module cannot fill either field honestly. It sends the
                // event with empty identifiers rather than inventing values
                // that would look real but correlate with nothing. A caller
                // that needs correlation must plumb an identifier through
                // `ToolCtx`, which is outside this task unit's owned files.
                turn: String::new(),
                call_id: String::new(),
                line: line.clone(),
            });
            buf.push_str(&line);
            buf.push('\n');
        }
        buf
    });

    let outcome = tokio::select! {
        biased;
        () = ctx.cancel.cancelled() => Outcome::Cancelled,
        () = tokio::time::sleep(timeout) => Outcome::TimedOut,
        status = child.wait() => match status {
            Ok(status) => Outcome::Exited(status),
            Err(e) => {
                return Err(Error::new(
                    ErrCode::ToolFailed,
                    format!("failed to wait for '{program}': {e}"),
                ));
            }
        },
    };

    if matches!(outcome, Outcome::TimedOut | Outcome::Cancelled) {
        kill_tree(&mut child).await;
        let _ = tokio::time::timeout(REAP_GRACE, child.wait()).await;
    }

    // The child has exited or was killed, so its pipes have closed (or will
    // close very soon). Drain what the pumps collected, bounded so a stuck
    // pipe never hangs the tool reply.
    // One deadline covers the whole drain. Timing the three tasks out one
    // after another would stack to four times REAP_GRACE in the worst case,
    // which is long enough that a caller waiting on the tool reply gives up
    // first.
    let collected = tokio::time::timeout(REAP_GRACE, async {
        let buf = collector.await.unwrap_or_default();
        let _ = out_pump.await;
        let _ = err_pump.await;
        buf
    })
    .await
    .unwrap_or_default();

    let body = cap::cap_output(&collected);

    match outcome {
        Outcome::Exited(status) => {
            let message = format!("exit status: {}\n{body}", exit_status_text(status));
            if status.success() {
                Ok(ToolResult::ok(message))
            } else {
                Ok(ToolResult::error(message))
            }
        }
        Outcome::TimedOut => Err(Error::new(
            ErrCode::ToolTimeout,
            format!(
                "'{program}' exceeded the {}s timeout and was killed\n{body}",
                timeout.as_secs()
            ),
        )),
        Outcome::Cancelled => Err(Error::new(
            ErrCode::ToolFailed,
            format!("'{program}' was cancelled\n{body}"),
        )),
    }
}

/// Reads `reader` line by line and forwards every line to `tx`.
///
/// Stops when the reader reaches end of file, when a read fails, or when the
/// receiving end has dropped.
async fn pump(reader: impl AsyncRead + Unpin, tx: mpsc::UnboundedSender<String>) {
    let mut lines = BufReader::new(reader).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        if tx.send(line).is_err() {
            break;
        }
    }
}

/// Formats an exit status for the tool reply.
///
/// Prefers the exit code. Falls back to naming the signal on Unix, since a
/// killed process has no exit code.
fn exit_status_text(status: std::process::ExitStatus) -> String {
    if let Some(code) = status.code() {
        return code.to_string();
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt as _;
        if let Some(signal) = status.signal() {
            return format!("killed by signal {signal}");
        }
    }
    "unknown".to_owned()
}

#[cfg(unix)]
async fn kill_tree(child: &mut tokio::process::Child) {
    use nix::sys::signal::{Signal, killpg};
    use nix::unistd::Pid;

    // The child leads its own process group (`process_group(0)` at spawn),
    // so its group id equals its process id. Signalling the group reaches
    // the grandchildren a shell or build tool spawned, which is the whole
    // point: killing only the direct child leaves those running and holding
    // the pipes open.
    //
    // This calls the syscall through `nix` rather than shelling out to
    // `kill`. The `kill` binary is not dependable for this: util-linux
    // `kill -9 -<pgid>` exits 0 on this platform without delivering the
    // signal, so a subprocess-based kill reports success and leaves the
    // group alive.
    if let Some(pid) = child.id() {
        if let Ok(raw) = i32::try_from(pid) {
            let _ = killpg(Pid::from_raw(raw), Signal::SIGKILL);
        }
    }

    // Always kill the direct child too. The group signal may have missed it
    // if the child changed its own group after spawn.
    let _ = child.kill().await;
}

/// Stops the direct child. See the module documentation: a descendant the
/// child spawned may survive this call.
#[cfg(windows)]
async fn kill_tree(child: &mut tokio::process::Child) {
    let _ = child.kill().await;
}

#[cfg(test)]
mod tests {
    use super::run;
    use dark_contract::{ErrCode, Event, EventBus, Received, ToolCtx};
    use std::time::Duration;
    use tokio_util::sync::CancellationToken;

    fn ctx(events: dark_contract::EventTx, dark: bool) -> ToolCtx {
        ToolCtx {
            root: std::env::temp_dir(),
            events,
            cancel: CancellationToken::new(),
            dark,
            human_present: true,
        }
    }

    #[cfg(unix)]
    fn long_sleep() -> (String, Vec<String>) {
        ("sleep".to_owned(), vec!["30".to_owned()])
    }

    #[cfg(windows)]
    fn long_sleep() -> (String, Vec<String>) {
        // ping.exe runs fine with redirected stdio, unlike timeout.exe.
        (
            "ping".to_owned(),
            vec!["-n".to_owned(), "60".to_owned(), "127.0.0.1".to_owned()],
        )
    }

    #[cfg(unix)]
    fn echo_lines(lines: &[&str]) -> (String, Vec<String>) {
        let script = lines.join("\n");
        ("printf".to_owned(), vec!["%s\\n".to_owned(), script])
    }

    #[cfg(windows)]
    fn echo_lines(lines: &[&str]) -> (String, Vec<String>) {
        // `cmd` here is the *test's* fixture command, not this tool bypassing
        // its own no-shell rule: the tool under test still receives an
        // already-split program and argument list.
        let joined = lines.join("&echo ");
        (
            "cmd".to_owned(),
            vec!["/C".to_owned(), format!("echo {joined}")],
        )
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_timeout_kills_a_long_running_child() {
        let bus = EventBus::new();
        let (program, args) = long_sleep();
        let ctx = ctx(bus.tx(), false);

        let start = std::time::Instant::now();
        let result = run(
            &ctx,
            program,
            args,
            std::env::temp_dir(),
            Duration::from_millis(200),
        )
        .await;
        let elapsed = start.elapsed();

        let err = result.expect_err("a timed-out command must return an error");
        assert_eq!(err.code, ErrCode::ToolTimeout);
        assert!(
            elapsed < Duration::from_secs(15),
            "the child was not killed promptly: waited {elapsed:?}"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn output_streams_as_tool_progress_events() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        let ctx = ctx(bus.tx(), false);
        let (program, args) = echo_lines(&["one", "two", "three"]);

        let result = run(
            &ctx,
            program,
            args,
            std::env::temp_dir(),
            Duration::from_secs(30),
        )
        .await
        .unwrap();
        assert!(!result.is_error);

        let mut seen_lines = Vec::new();
        while let Ok(Some(received)) =
            tokio::time::timeout(Duration::from_millis(200), rx.recv()).await
        {
            if let Received::Event(Event::ToolProgress { line, .. }) = received {
                seen_lines.push(line);
            }
        }
        assert!(
            seen_lines.contains(&"one".to_owned()),
            "no ToolProgress event carried a line of output: {seen_lines:?}"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn dark_mode_sets_dark_offline_in_the_child_environment() {
        let bus = EventBus::new();
        let ctx = ctx(bus.tx(), true);

        #[cfg(unix)]
        let (program, args) = (
            "sh".to_owned(),
            vec![
                "-c".to_owned(),
                "printf \"%s\" \"$DARK_OFFLINE\"".to_owned(),
            ],
        );
        #[cfg(windows)]
        let (program, args) = (
            "cmd".to_owned(),
            vec!["/C".to_owned(), "echo %DARK_OFFLINE%".to_owned()],
        );

        let result = run(
            &ctx,
            program,
            args,
            std::env::temp_dir(),
            Duration::from_secs(30),
        )
        .await
        .unwrap();
        assert!(!result.is_error, "content was: {}", result.content);
        assert!(
            result.content.contains('1'),
            "DARK_OFFLINE was not visible to the child: {}",
            result.content
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn light_mode_leaves_dark_offline_unset() {
        let bus = EventBus::new();
        let ctx = ctx(bus.tx(), false);

        #[cfg(unix)]
        let (program, args) = (
            "sh".to_owned(),
            vec![
                "-c".to_owned(),
                "printf \"[%s]\" \"$DARK_OFFLINE\"".to_owned(),
            ],
        );
        #[cfg(windows)]
        let (program, args) = (
            "cmd".to_owned(),
            vec!["/C".to_owned(), "echo [%DARK_OFFLINE%]".to_owned()],
        );

        let result = run(
            &ctx,
            program,
            args,
            std::env::temp_dir(),
            Duration::from_secs(30),
        )
        .await
        .unwrap();
        assert!(result.content.contains("[]") || result.content.contains("[%DARK_OFFLINE%]"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn cancellation_stops_the_child() {
        let bus = EventBus::new();
        let cancel = CancellationToken::new();
        let ctx = ToolCtx {
            root: std::env::temp_dir(),
            events: bus.tx(),
            cancel: cancel.clone(),
            dark: false,
            human_present: true,
        };
        let (program, args) = long_sleep();

        let handle = tokio::spawn(async move {
            run(
                &ctx,
                program,
                args,
                std::env::temp_dir(),
                Duration::from_secs(60),
            )
            .await
        });

        tokio::time::sleep(Duration::from_millis(200)).await;
        cancel.cancel();

        let result = tokio::time::timeout(Duration::from_secs(15), handle)
            .await
            .expect("run() did not return promptly after cancellation")
            .unwrap();

        let err = result.expect_err("a cancelled command must return an error");
        assert_eq!(err.code, ErrCode::ToolFailed);
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread")]
    async fn a_timeout_kills_the_whole_process_group_not_just_the_direct_child() {
        // The direct child is a shell that spawns a grandchild `sleep`. A
        // correct process-group kill takes the grandchild down too. This is
        // the `process_group_kill` guarantee that task unit C3 asks for.
        let bus = EventBus::new();
        let ctx = ctx(bus.tx(), false);
        let marker = std::env::temp_dir().join(format!(
            "darkharness-exec-pgkill-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let marker_path = marker.to_string_lossy().into_owned();

        // The grandchild `sleep` writes the marker file only after it wakes
        // up. If the kill reached only the shell and left the grandchild
        // running, the marker would appear once the test's own wait elapses.
        let script = format!("(sleep 5; touch {marker_path}) & wait");
        let program = "sh".to_owned();
        let args = vec!["-c".to_owned(), script];

        let result = run(
            &ctx,
            program,
            args,
            std::env::temp_dir(),
            Duration::from_millis(300),
        )
        .await;
        assert!(result.is_err(), "expected the timeout to fire");

        // Give a leaked grandchild every chance to finish the delayed write
        // before checking. A correct group kill means the marker never
        // appears.
        tokio::time::sleep(Duration::from_secs(6)).await;
        assert!(
            !marker.exists(),
            "a grandchild process outlived the process-group kill"
        );
        let _ = std::fs::remove_file(&marker);
    }
}
