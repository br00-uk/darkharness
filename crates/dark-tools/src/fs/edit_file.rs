//! The `edit_file` tool.
//!
//! `edit_file` requires an exact, single match for `old_string`. A zero-match
//! call is the common failure, and the highest-value behaviour in this task
//! unit turns it into a recoverable turn: the tool reports the three
//! candidate locations that come closest to `old_string`, with their line
//! numbers, instead of a bare failure.

use std::sync::Arc;

use async_trait::async_trait;
use dark_contract::{ErrCode, Error, Result, Tool, ToolCtx, ToolResult, ToolSchema, tool::tier};
use serde::Deserialize;
use serde_json::{Value, json};
use similar::TextDiff;

use super::atomic;
use super::diff_util;
use super::path;
use super::state::ReadState;
use std::fmt::Write;

/// The number of near-miss candidates that a zero-match call reports.
const CANDIDATE_COUNT: usize = 3;

#[derive(Debug, Deserialize)]
struct Args {
    path: String,
    old_string: String,
    new_string: String,
}

/// Replaces one exact occurrence of `old_string` with `new_string` in a file.
#[derive(Debug)]
pub struct EditFile {
    state: Arc<ReadState>,
}

impl EditFile {
    /// Creates the tool, sharing `state` with the other file tools in this
    /// session.
    pub(crate) fn new(state: Arc<ReadState>) -> Self {
        Self { state }
    }
}

#[async_trait]
impl Tool for EditFile {
    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "edit_file".to_string(),
            description: "Replaces one exact occurrence of old_string with new_string in a \
                file. old_string must match exactly once. Read the file in this session first."
                .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "The file path, relative to the repository root.",
                    },
                    "old_string": {
                        "type": "string",
                        "description": "The exact text to find. It must match exactly once.",
                    },
                    "new_string": {
                        "type": "string",
                        "description": "The text that replaces old_string.",
                    },
                },
                "required": ["path", "old_string", "new_string"],
            }),
            tier: tier::ESSENTIAL,
            mutating: true,
        }
    }

    async fn invoke(&self, args: Value, ctx: &ToolCtx) -> Result<ToolResult> {
        let args: Args = serde_json::from_value(args).map_err(|err| {
            Error::new(
                ErrCode::ToolInvalidArgs,
                format!("edit_file arguments: {err}"),
            )
        })?;

        if args.old_string.is_empty() {
            return Err(Error::new(
                ErrCode::ToolInvalidArgs,
                "old_string must not be empty",
            ));
        }

        let resolved = path::resolve(&ctx.root, &args.path)?;

        let mode = tokio::fs::metadata(&resolved)
            .await
            .ok()
            .map(|m| m.permissions());
        let bytes = tokio::fs::read(&resolved).await.map_err(|_| {
            Error::new(
                ErrCode::ToolNotFound,
                format!("{} does not exist", args.path),
            )
        })?;

        self.state.check_fresh(&resolved, &bytes)?;

        let old_text = String::from_utf8(bytes).map_err(|_| {
            Error::new(
                ErrCode::ToolFailed,
                format!("{} is not valid UTF-8", args.path),
            )
        })?;

        let count = old_text.matches(args.old_string.as_str()).count();
        match count {
            0 => Err(no_match_error(&args.path, &old_text, &args.old_string)),
            1 => {
                let new_text = old_text.replacen(&args.old_string, &args.new_string, 1);
                let new_bytes = new_text.clone().into_bytes();
                atomic::write(resolved.clone(), new_bytes.clone(), mode).await?;
                self.state.record(&resolved, &new_bytes);

                let diff = diff_util::render(&args.path, &old_text, &new_text);
                let mut result = ToolResult::ok(format!("edited {}", args.path));
                if let Some(diff) = diff {
                    result = result.with_diff(diff);
                }
                Ok(result)
            }
            n => Err(Error::new(
                ErrCode::ToolAmbiguous,
                format!(
                    "old_string matches {n} times in {}; it must match exactly once",
                    args.path
                ),
            )),
        }
    }
}

/// One line where `old_string` came closest to matching.
struct Candidate {
    line: usize,
    text: String,
    score: f32,
}

/// Builds the zero-match error, listing the [`CANDIDATE_COUNT`] locations in
/// `content` most similar to `needle`.
fn no_match_error(display_path: &str, content: &str, needle: &str) -> Error {
    let candidates = nearest_candidates(content, needle, CANDIDATE_COUNT);

    let mut message = format!("old_string did not match anywhere in {display_path}.\n");
    if candidates.is_empty() {
        message.push_str("The file has no comparable lines.\n");
    } else {
        message.push_str("Nearest candidates:\n");
        for (i, candidate) in candidates.iter().enumerate() {
            let pct = (candidate.score * 100.0).round();
            let _ = write!(
                message,
                "  {}. line {} ({pct:.0}% similar):\n     {}\n",
                i + 1,
                candidate.line,
                first_line(&candidate.text)
            );
        }
    }

    Error::new(ErrCode::ToolFailed, message)
        .with_remedy("Copy the exact text from one of the candidates above, then try again.")
}

fn first_line(text: &str) -> &str {
    text.lines().next().unwrap_or(text)
}

/// Scores every window of `content` that is the same number of lines as
/// `needle`, by character similarity, and returns the highest-scoring `n`,
/// most-similar-first, with no two candidates overlapping.
///
/// The comparison is by character, not by line. A line-based diff treats a
/// whole line as one atomic token, so a window that differs from the needle
/// by a single character scores exactly the same as one that shares nothing
/// with it: both are simply "not equal". Every candidate then ties on zero
/// and the order degenerates to file order, which is the opposite of what
/// this function is for. Comparing characters is what makes a near miss
/// rank above an unrelated line.
fn nearest_candidates(content: &str, needle: &str, n: usize) -> Vec<Candidate> {
    let lines: Vec<&str> = content.lines().collect();
    if lines.is_empty() {
        return Vec::new();
    }

    let needle_lines = needle.lines().count().max(1);
    let window = needle_lines.min(lines.len());

    let mut scored: Vec<Candidate> = Vec::with_capacity(lines.len());
    for start in 0..=(lines.len() - window) {
        let window_text = lines[start..start + window].join("\n");
        let score = TextDiff::from_chars(needle, window_text.as_str()).ratio();
        scored.push(Candidate {
            line: start + 1,
            text: window_text,
            score,
        });
    }

    scored.sort_by(|a, b| b.score.total_cmp(&a.score).then(a.line.cmp(&b.line)));

    let mut picked: Vec<Candidate> = Vec::with_capacity(n);
    for candidate in scored {
        let overlaps = picked
            .iter()
            .any(|p: &Candidate| candidate.line.abs_diff(p.line) < window);
        if !overlaps {
            picked.push(candidate);
            if picked.len() == n {
                break;
            }
        }
    }
    picked
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::read_file::ReadFile;
    use tokio_util::sync::CancellationToken;

    fn ctx(root: &std::path::Path) -> ToolCtx {
        let bus = dark_contract::EventBus::new();
        ToolCtx {
            root: root.to_path_buf(),
            events: bus.tx(),
            cancel: CancellationToken::new(),
            dark: true,
            human_present: false,
        }
    }

    fn read_then_edit_tools() -> (ReadFile, EditFile) {
        let state = Arc::new(ReadState::new());
        (ReadFile::new(state.clone()), EditFile::new(state))
    }

    #[tokio::test]
    async fn a_single_exact_match_is_replaced() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "fn foo() {}\n").unwrap();
        let (reader, editor) = read_then_edit_tools();

        reader
            .invoke(json!({"path": "a.txt"}), &ctx(dir.path()))
            .await
            .unwrap();
        let result = editor
            .invoke(
                json!({"path": "a.txt", "old_string": "foo", "new_string": "bar"}),
                &ctx(dir.path()),
            )
            .await
            .unwrap();

        assert!(!result.is_error);
        assert_eq!(
            std::fs::read_to_string(dir.path().join("a.txt")).unwrap(),
            "fn bar() {}\n"
        );
        assert!(result.diff.unwrap().contains("-fn foo"));
    }

    #[tokio::test]
    async fn zero_matches_reports_the_three_nearest_candidates() {
        let dir = tempfile::tempdir().unwrap();
        let content = "fn foo() {}\nfn foobar() {}\nfn baz() {}\nfn foo_bar() {}\n";
        std::fs::write(dir.path().join("a.txt"), content).unwrap();
        let (reader, editor) = read_then_edit_tools();

        reader
            .invoke(json!({"path": "a.txt"}), &ctx(dir.path()))
            .await
            .unwrap();
        let err = editor
            .invoke(
                json!({"path": "a.txt", "old_string": "fn fooo() {}", "new_string": "x"}),
                &ctx(dir.path()),
            )
            .await
            .unwrap_err();

        assert_eq!(err.code, ErrCode::ToolFailed);
        assert!(err.message.contains("Nearest candidates"));
        assert_eq!(err.message.matches("line ").count(), 3);
        // The unchanged on-disk content proves the failed edit made no change.
        assert_eq!(
            std::fs::read_to_string(dir.path().join("a.txt")).unwrap(),
            content
        );
    }

    #[tokio::test]
    async fn multiple_matches_is_ambiguous() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "x = 1\nx = 1\n").unwrap();
        let (reader, editor) = read_then_edit_tools();

        reader
            .invoke(json!({"path": "a.txt"}), &ctx(dir.path()))
            .await
            .unwrap();
        let err = editor
            .invoke(
                json!({"path": "a.txt", "old_string": "x = 1", "new_string": "x = 2"}),
                &ctx(dir.path()),
            )
            .await
            .unwrap_err();

        assert_eq!(err.code, ErrCode::ToolAmbiguous);
    }

    #[tokio::test]
    async fn editing_without_a_prior_read_is_stale() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "hello").unwrap();
        let editor = EditFile::new(Arc::new(ReadState::new()));

        let err = editor
            .invoke(
                json!({"path": "a.txt", "old_string": "hello", "new_string": "goodbye"}),
                &ctx(dir.path()),
            )
            .await
            .unwrap_err();
        assert_eq!(err.code, ErrCode::ToolStale);
    }

    #[tokio::test]
    async fn editing_a_file_changed_since_the_read_is_stale() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "hello").unwrap();
        let (reader, editor) = read_then_edit_tools();
        reader
            .invoke(json!({"path": "a.txt"}), &ctx(dir.path()))
            .await
            .unwrap();
        std::fs::write(dir.path().join("a.txt"), "someone else edited this").unwrap();

        let err = editor
            .invoke(
                json!({"path": "a.txt", "old_string": "hello", "new_string": "goodbye"}),
                &ctx(dir.path()),
            )
            .await
            .unwrap_err();
        assert_eq!(err.code, ErrCode::ToolStale);
    }

    #[tokio::test]
    async fn a_missing_file_is_tool_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let editor = EditFile::new(Arc::new(ReadState::new()));

        let err = editor
            .invoke(
                json!({"path": "missing.txt", "old_string": "a", "new_string": "b"}),
                &ctx(dir.path()),
            )
            .await
            .unwrap_err();
        assert_eq!(err.code, ErrCode::ToolNotFound);
    }

    #[test]
    fn nearest_candidates_ranks_the_closest_line_first() {
        let content = "alpha\nbetaX\ngamma\ndelta\n";
        let candidates = nearest_candidates(content, "beta", 3);
        assert_eq!(candidates.first().unwrap().line, 2);
    }

    #[test]
    fn nearest_candidates_handles_a_needle_longer_than_the_file() {
        let content = "one line only\n";
        let candidates = nearest_candidates(content, "one line only\nplus another\n", 3);
        assert_eq!(candidates.len(), 1);
    }

    #[test]
    fn nearest_candidates_on_empty_content_is_empty() {
        assert!(nearest_candidates("", "anything", 3).is_empty());
    }
}
