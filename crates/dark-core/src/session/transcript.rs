//! The append-only transcript, and the rebuild of a session's messages from it.
//!
//! One line of `$DARK_HOME/sessions/<ulid>/transcript.jsonl` holds one
//! serialised [`Event`]. [`TranscriptWriter`] appends a line for every event
//! a session sees. [`replay`] reads that file back and rebuilds the message
//! list the events imply.
//!
//! # What replay rebuilds
//!
//! Replay reproduces the message list exactly: the person's messages from
//! [`Event::UserMessage`], each round trip's assistant message from its
//! token and reasoning deltas and its tool calls, and one `Role::Tool` reply
//! carrying the full content of each [`Event::ToolResult`].
//!
//! Two of those events were added after this module was first written.
//! Before them the transcript held no record of what a person typed, and it
//! held only a tool result's headline, so replay could not reproduce a whole
//! session from its own record. See `docs/adr/0004` for the third gap in the
//! same contract, which is still open.

use std::path::{Path, PathBuf};

use dark_contract::{ErrCode, Error, Event, Message, Result, Role, ToolCall};
use tokio::io::{AsyncWriteExt, BufWriter};
use ulid::Ulid;

/// Returns the transcript path for session `id` under `sessions_root`.
///
/// The caller passes `sessions_root`; this module never reads `DARK_HOME`
/// itself. In production `sessions_root` is `$DARK_HOME/sessions` (section
/// 5.3); a test passes a fixture directory instead.
pub fn transcript_path(sessions_root: &Path, id: Ulid) -> PathBuf {
    sessions_root.join(id.to_string()).join("transcript.jsonl")
}

/// Appends one JSON object per [`Event`] to a session's transcript.
///
/// Every call to [`TranscriptWriter::record`] appends its line to an
/// in-memory buffer. The writer flushes that buffer to disk only after
/// [`Event::TurnEnd`] and [`Event::MapChanged`]; every other event,
/// [`Event::TokenDelta`] included, stays buffered until the next flush
/// point. A turn that streams a thousand tokens costs one disk write, not a
/// thousand.
#[derive(Debug)]
pub struct TranscriptWriter {
    path: PathBuf,
    file: BufWriter<tokio::fs::File>,
}

impl TranscriptWriter {
    /// Opens the transcript for `id` under `sessions_root`, creating the
    /// session directory and the file when they do not exist, and
    /// appending to the file when it already does.
    ///
    /// # Errors
    ///
    /// Returns an error when the session directory or the transcript file
    /// cannot be created or opened.
    pub async fn open(sessions_root: &Path, id: Ulid) -> Result<Self> {
        let dir = sessions_root.join(id.to_string());
        tokio::fs::create_dir_all(&dir)
            .await
            .map_err(|err| io_error(&dir, &err))?;

        let path = dir.join("transcript.jsonl");
        let file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .await
            .map_err(|err| io_error(&path, &err))?;

        Ok(Self {
            path,
            file: BufWriter::new(file),
        })
    }

    /// Returns the transcript path this writer appends to.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Appends `event` and flushes to disk when the flush policy requires
    /// it: after [`Event::TurnEnd`] and after [`Event::MapChanged`]. Every
    /// other event, a token delta included, stays buffered.
    ///
    /// # Errors
    ///
    /// Returns an error when the event cannot be serialised, or when the
    /// write or the flush fails.
    pub async fn record(&mut self, event: &Event) -> Result<()> {
        let mut line = serde_json::to_string(event).map_err(|err| {
            Error::new(
                ErrCode::ToolFailed,
                format!(
                    "cannot serialise an event for {}: {err}",
                    self.path.display()
                ),
            )
        })?;
        line.push('\n');

        self.file
            .write_all(line.as_bytes())
            .await
            .map_err(|err| io_error(&self.path, &err))?;

        if matches!(event, Event::TurnEnd { .. } | Event::MapChanged { .. }) {
            self.flush().await?;
        }
        Ok(())
    }

    /// Flushes buffered writes to disk.
    ///
    /// # Errors
    ///
    /// Returns an error when the flush fails.
    pub async fn flush(&mut self) -> Result<()> {
        self.file
            .flush()
            .await
            .map_err(|err| io_error(&self.path, &err))
    }
}

/// Reads the transcript for `id` under `sessions_root` and rebuilds the
/// message list its events imply. See the module documentation for what
/// replay can and cannot reconstruct.
///
/// # Errors
///
/// Returns an error when the transcript does not exist or cannot be read,
/// or when a line other than the last one fails to parse — see
/// [`read_events`].
pub async fn replay(sessions_root: &Path, id: Ulid) -> Result<Vec<Message>> {
    let events = read_events(sessions_root, id).await?;
    Ok(rebuild_messages(&events))
}

/// Reads the raw events recorded for `id` under `sessions_root`.
///
/// Tolerates a truncated or partly written final line: a crash mid-write
/// can leave one incomplete JSON object at the end of the file, and this
/// function drops that line rather than failing the whole read. A line
/// other than the last one that fails to parse is not a crash artefact — it
/// is corruption — and this function returns an error for it.
///
/// # Errors
///
/// Returns an error when the transcript file does not exist or cannot be
/// read, or when a non-final line fails to parse.
pub async fn read_events(sessions_root: &Path, id: Ulid) -> Result<Vec<Event>> {
    let path = transcript_path(sessions_root, id);
    let content = tokio::fs::read_to_string(&path)
        .await
        .map_err(|err| io_error(&path, &err))?;
    parse_events(&path, &content)
}

/// Rebuilds the message list that `events` implies.
///
/// Records each [`Event::UserMessage`] as a `Role::User` message, folds each
/// turn's [`Event::TokenDelta`] and [`Event::ReasonDelta`] text into one
/// `Role::Assistant` message, attaches the turn's [`Event::ToolCall`] calls
/// to that message, and inserts one `Role::Tool` reply carrying the full
/// content of each [`Event::ToolResult`], in the order the events arrived.
/// Every other event kind carries no message content and contributes
/// nothing to the result.
pub fn rebuild_messages(events: &[Event]) -> Vec<Message> {
    let mut messages = Vec::new();
    let mut turn: Option<TurnAccumulator> = None;

    for event in events {
        match event {
            Event::TurnStart { .. } => {
                flush_turn(&mut turn, &mut messages);
                turn = Some(TurnAccumulator::default());
            }
            Event::UserMessage { text, .. } => {
                // The person's message opens a turn, so it lands before the
                // assistant text that answers it.
                flush_turn(&mut turn, &mut messages);
                messages.push(Message::text(Role::User, text.clone()));
            }
            Event::TokenDelta { text, .. } => {
                if let Some(acc) = turn.as_mut() {
                    acc.text.push_str(text);
                }
            }
            Event::ReasonDelta { text, .. } => {
                if let Some(acc) = turn.as_mut() {
                    acc.reasoning.get_or_insert_with(String::new).push_str(text);
                }
            }
            Event::ToolCall { call, .. } => {
                if let Some(acc) = turn.as_mut() {
                    acc.tool_calls.push(call.clone());
                }
            }
            Event::ToolResult {
                call_id, content, ..
            } => {
                // A tool result ends the assistant segment that requested
                // it and opens a fresh one for whatever text follows, so a
                // multi-round-trip turn (task unit A2) replays as one
                // assistant message per round trip, not one per turn.
                flush_turn(&mut turn, &mut messages);
                turn = Some(TurnAccumulator::default());
                messages.push(tool_reply_message(call_id, content));
            }
            Event::TurnEnd { .. } => {
                flush_turn(&mut turn, &mut messages);
            }
            _ => {}
        }
    }
    flush_turn(&mut turn, &mut messages);
    messages
}

/// Accumulates one round trip's streamed text, reasoning, and tool calls
/// until it flushes into a `Role::Assistant` message.
#[derive(Debug, Default)]
struct TurnAccumulator {
    text: String,
    reasoning: Option<String>,
    tool_calls: Vec<ToolCall>,
}

impl TurnAccumulator {
    fn is_empty(&self) -> bool {
        self.text.is_empty() && self.reasoning.is_none() && self.tool_calls.is_empty()
    }

    fn finish(self) -> Option<Message> {
        if self.is_empty() {
            return None;
        }
        let mut message = Message::text(Role::Assistant, self.text);
        message.reasoning = self.reasoning;
        message.tool_calls = self.tool_calls;
        Some(message)
    }
}

/// Takes `turn`, and when it holds any content, pushes it onto `messages`
/// as one `Role::Assistant` message.
fn flush_turn(turn: &mut Option<TurnAccumulator>, messages: &mut Vec<Message>) {
    if let Some(acc) = turn.take()
        && let Some(message) = acc.finish()
    {
        messages.push(message);
    }
}

/// Builds the `Role::Tool` reply for one tool result.
///
/// The reply carries the tool's full content, which is what the model saw
/// when the call first ran. [`Event::ToolResult`] carries both that content
/// and a compact summary; the summary is for a display, not for a replay.
fn tool_reply_message(call_id: &str, content: &str) -> Message {
    Message::tool_reply(call_id.to_owned(), content.to_owned())
}

/// Parses `content` as newline-delimited JSON events, tolerating a
/// truncated or partly written final line.
fn parse_events(path: &Path, content: &str) -> Result<Vec<Event>> {
    let mut lines: Vec<&str> = content.split('\n').collect();
    if lines.last() == Some(&"") {
        lines.pop();
    }
    let last_index = lines.len().checked_sub(1);

    let mut events = Vec::with_capacity(lines.len());
    for (index, line) in lines.iter().enumerate() {
        if line.is_empty() {
            continue;
        }
        match serde_json::from_str::<Event>(line) {
            Ok(event) => events.push(event),
            Err(err) => {
                if Some(index) == last_index {
                    tracing::warn!(
                        path = %path.display(),
                        error = %err,
                        "dropped a truncated final transcript line"
                    );
                    continue;
                }
                return Err(Error::new(
                    ErrCode::ToolFailed,
                    format!(
                        "line {} of {} is not valid JSON: {err}",
                        index + 1,
                        path.display()
                    ),
                ));
            }
        }
    }
    Ok(events)
}

/// Maps an I/O failure to the harness error taxonomy.
///
/// A missing transcript means the session does not exist, which
/// [`ErrCode::SessionNotFound`] names exactly. Every other I/O failure (a
/// permission error, a full disk) uses [`ErrCode::ToolFailed`], the
/// taxonomy's general-purpose code for a failure that no domain-specific
/// code covers — the same choice `dark-agentsmd::resolve` makes for a file
/// it cannot read.
fn io_error(path: &Path, err: &std::io::Error) -> Error {
    if err.kind() == std::io::ErrorKind::NotFound {
        Error::new(
            ErrCode::SessionNotFound,
            format!("no transcript at {}: {err}", path.display()),
        )
    } else {
        Error::new(
            ErrCode::ToolFailed,
            format!("cannot access {}: {err}", path.display()),
        )
    }
}

#[cfg(test)]
mod tests {
    use dark_contract::ToolResultSummary;

    use super::*;
    use dark_contract::{RoleClass, Usage};
    use tempfile::TempDir;

    fn turn_start(turn: &str) -> Event {
        Event::TurnStart {
            turn: turn.to_owned(),
            class: RoleClass::Worker,
            model: "test-model".to_owned(),
        }
    }

    fn token(turn: &str, text: &str) -> Event {
        Event::TokenDelta {
            turn: turn.to_owned(),
            text: text.to_owned(),
        }
    }

    fn turn_end(turn: &str) -> Event {
        Event::TurnEnd {
            turn: turn.to_owned(),
            usage: Usage::default(),
            wall_ms: 10,
        }
    }

    fn tool_call_event(turn: &str, id: &str, name: &str) -> Event {
        Event::ToolCall {
            turn: turn.to_owned(),
            call: ToolCall {
                id: id.to_owned(),
                name: name.to_owned(),
                args: serde_json::json!({}),
            },
        }
    }

    fn user_message(turn: &str, text: &str) -> Event {
        Event::UserMessage {
            turn: turn.to_owned(),
            text: text.to_owned(),
        }
    }

    /// `A1` done-when: replay reproduces the message list exactly. This is
    /// the whole shape of one turn — what the person asked, what the model
    /// answered, the call it made, and the tool's full output.
    #[tokio::test]
    async fn replay_reproduces_a_whole_turn_including_the_person_and_the_tool_output() {
        let long_output = "line one\nline two\nline three";
        let events = vec![
            user_message("t1", "read src/lib.rs for me"),
            turn_start("t1"),
            token("t1", "reading it now"),
            tool_call_event("t1", "c1", "read_file"),
            tool_result_event("t1", "c1", long_output),
            token("t1", "that file declares four modules"),
            turn_end("t1"),
        ];

        let messages = rebuild_messages(&events);

        assert_eq!(messages[0].role, Role::User);
        assert_eq!(messages[0].text_content(), "read src/lib.rs for me");

        assert_eq!(messages[1].role, Role::Assistant);
        assert_eq!(messages[1].text_content(), "reading it now");
        assert_eq!(messages[1].tool_calls.len(), 1);

        assert_eq!(messages[2].role, Role::Tool);
        assert_eq!(
            messages[2].text_content(),
            long_output,
            "a replayed tool reply must hold the whole output, not its headline"
        );

        assert_eq!(messages[3].role, Role::Assistant);
        assert_eq!(
            messages[3].text_content(),
            "that file declares four modules"
        );
        assert_eq!(messages.len(), 4);
    }

    #[tokio::test]
    async fn a_second_turn_replays_its_own_user_message() {
        let events = vec![
            user_message("t1", "first question"),
            turn_start("t1"),
            token("t1", "first answer"),
            turn_end("t1"),
            user_message("t2", "second question"),
            turn_start("t2"),
            token("t2", "second answer"),
            turn_end("t2"),
        ];

        let roles: Vec<Role> = rebuild_messages(&events).iter().map(|m| m.role).collect();
        assert_eq!(
            roles,
            vec![Role::User, Role::Assistant, Role::User, Role::Assistant]
        );
    }

    fn tool_result_event(turn: &str, id: &str, content: &str) -> Event {
        Event::ToolResult {
            turn: turn.to_owned(),
            call_id: id.to_owned(),
            result: ToolResultSummary {
                name: "read_file".to_owned(),
                is_error: false,
                bytes: content.len(),
                headline: content.lines().next().unwrap_or_default().to_owned(),
                has_diff: false,
            },
            content: content.to_owned(),
        }
    }

    #[tokio::test]
    async fn record_appends_one_json_line_per_event() {
        let tmp = TempDir::new().unwrap();
        let id = Ulid::new();
        let mut writer = TranscriptWriter::open(tmp.path(), id).await.unwrap();

        writer.record(&turn_start("t1")).await.unwrap();
        writer.record(&token("t1", "hi")).await.unwrap();
        writer.record(&turn_end("t1")).await.unwrap();

        let raw = tokio::fs::read_to_string(writer.path()).await.unwrap();
        let lines: Vec<&str> = raw.lines().collect();
        assert_eq!(lines.len(), 3);
        for line in &lines {
            serde_json::from_str::<Event>(line).unwrap();
        }
    }

    #[tokio::test]
    async fn record_does_not_flush_on_a_token_delta() {
        let tmp = TempDir::new().unwrap();
        let id = Ulid::new();
        let mut writer = TranscriptWriter::open(tmp.path(), id).await.unwrap();
        let path = writer.path().to_path_buf();

        writer.record(&token("t1", "hello")).await.unwrap();

        // An independent handle sees only what actually reached disk: a
        // token delta must leave the file untouched.
        let on_disk = tokio::fs::read_to_string(&path).await.unwrap();
        assert!(
            on_disk.is_empty(),
            "a token delta must not force a flush: found {on_disk:?}"
        );

        writer.flush().await.unwrap();
        let after_flush = tokio::fs::read_to_string(&path).await.unwrap();
        assert!(!after_flush.is_empty(), "an explicit flush must land");
    }

    #[tokio::test]
    async fn record_flushes_on_turn_end() {
        let tmp = TempDir::new().unwrap();
        let id = Ulid::new();
        let mut writer = TranscriptWriter::open(tmp.path(), id).await.unwrap();
        let path = writer.path().to_path_buf();

        writer.record(&token("t1", "hello")).await.unwrap();
        writer.record(&turn_end("t1")).await.unwrap();

        let on_disk = tokio::fs::read_to_string(&path).await.unwrap();
        assert_eq!(on_disk.lines().count(), 2, "TurnEnd must flush the buffer");
    }

    #[tokio::test]
    async fn record_flushes_on_map_changed() {
        let tmp = TempDir::new().unwrap();
        let id = Ulid::new();
        let mut writer = TranscriptWriter::open(tmp.path(), id).await.unwrap();
        let path = writer.path().to_path_buf();

        writer.record(&token("t1", "hello")).await.unwrap();
        writer
            .record(&Event::MapChanged {
                map_id: "m1".to_owned(),
            })
            .await
            .unwrap();

        let on_disk = tokio::fs::read_to_string(&path).await.unwrap();
        assert_eq!(
            on_disk.lines().count(),
            2,
            "MapChanged must flush the buffer"
        );
    }

    #[tokio::test]
    async fn replay_reproduces_the_message_list_exactly() {
        let tmp = TempDir::new().unwrap();
        let id = Ulid::new();
        let mut writer = TranscriptWriter::open(tmp.path(), id).await.unwrap();

        writer.record(&turn_start("t1")).await.unwrap();
        writer.record(&token("t1", "Hello")).await.unwrap();
        writer.record(&token("t1", ", world")).await.unwrap();
        writer
            .record(&tool_call_event("t1", "c1", "read_file"))
            .await
            .unwrap();
        writer
            .record(&tool_result_event("t1", "c1", "3 lines"))
            .await
            .unwrap();
        writer.record(&token("t1", "done")).await.unwrap();
        writer.record(&turn_end("t1")).await.unwrap();

        let messages = replay(tmp.path(), id).await.unwrap();

        let mut first = Message::text(Role::Assistant, "Hello, world");
        first.tool_calls = vec![ToolCall {
            id: "c1".to_owned(),
            name: "read_file".to_owned(),
            args: serde_json::json!({}),
        }];
        let second = Message::tool_reply("c1", "3 lines");
        let third = Message::text(Role::Assistant, "done");

        assert_eq!(messages, vec![first, second, third]);
    }

    #[tokio::test]
    async fn replay_skips_a_truncated_final_line() {
        let tmp = TempDir::new().unwrap();
        let id = Ulid::new();
        let mut writer = TranscriptWriter::open(tmp.path(), id).await.unwrap();
        writer.record(&turn_start("t1")).await.unwrap();
        writer.record(&token("t1", "safe")).await.unwrap();
        writer.record(&turn_end("t1")).await.unwrap();

        let path = writer.path().to_path_buf();
        drop(writer);

        // Simulate a crash mid-write: a partial JSON object, no closing
        // brace, no trailing newline.
        let mut raw = tokio::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .await
            .unwrap();
        raw.write_all(br#"{"Notice":"cut off mid-wri"#)
            .await
            .unwrap();
        raw.flush().await.unwrap();
        drop(raw);

        let messages = replay(tmp.path(), id).await.unwrap();
        assert_eq!(messages, vec![Message::text(Role::Assistant, "safe")]);
    }

    #[tokio::test]
    async fn replay_errors_on_corruption_before_the_final_line() {
        let tmp = TempDir::new().unwrap();
        let id = Ulid::new();
        let path = transcript_path(tmp.path(), id);
        tokio::fs::create_dir_all(path.parent().unwrap())
            .await
            .unwrap();
        tokio::fs::write(&path, b"not json at all\n{\"Notice\":\"ok\"}\n")
            .await
            .unwrap();

        let err = replay(tmp.path(), id).await.unwrap_err();
        assert_eq!(err.code, ErrCode::ToolFailed);
    }

    #[tokio::test]
    async fn replay_of_a_missing_session_reports_session_not_found() {
        let tmp = TempDir::new().unwrap();
        let err = replay(tmp.path(), Ulid::new()).await.unwrap_err();
        assert_eq!(err.code, ErrCode::SessionNotFound);
    }

    #[tokio::test]
    async fn an_empty_transcript_replays_as_no_messages() {
        let tmp = TempDir::new().unwrap();
        let id = Ulid::new();
        let _writer = TranscriptWriter::open(tmp.path(), id).await.unwrap();

        let messages = replay(tmp.path(), id).await.unwrap();
        assert!(messages.is_empty());
    }
}
