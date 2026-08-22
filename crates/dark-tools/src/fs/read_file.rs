//! The `read_file` tool.

use std::sync::Arc;

use async_trait::async_trait;
use dark_contract::{ErrCode, Error, Result, Tool, ToolCtx, ToolResult, ToolSchema, tool::tier};
use serde::Deserialize;
use serde_json::{Value, json};

use super::path;
use super::state::ReadState;
use std::fmt::Write;

/// The maximum number of lines that one `read_file` call returns.
pub(crate) const MAX_LINES: usize = 2000;

#[derive(Debug, Deserialize)]
struct Args {
    path: String,
    #[serde(default)]
    offset: Option<usize>,
    #[serde(default)]
    limit: Option<usize>,
}

/// Reads a file, or a window of its lines, from the repository.
#[derive(Debug)]
pub struct ReadFile {
    state: Arc<ReadState>,
}

impl ReadFile {
    /// Creates the tool, sharing `state` with the other file tools in this
    /// session.
    pub(crate) fn new(state: Arc<ReadState>) -> Self {
        Self { state }
    }
}

#[async_trait]
impl Tool for ReadFile {
    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "read_file".to_string(),
            description: "Reads a file from the repository. Returns at most 2000 lines. \
                Pass offset and limit to page through a longer file."
                .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "The file path, relative to the repository root.",
                    },
                    "offset": {
                        "type": "integer",
                        "minimum": 0,
                        "description": "The 0-based line to start from. Default 0.",
                    },
                    "limit": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": MAX_LINES,
                        "description": "The number of lines to return. Default and maximum 2000.",
                    },
                },
                "required": ["path"],
            }),
            tier: tier::ESSENTIAL,
            mutating: false,
        }
    }

    async fn invoke(&self, args: Value, ctx: &ToolCtx) -> Result<ToolResult> {
        let args: Args = serde_json::from_value(args).map_err(|err| {
            Error::new(
                ErrCode::ToolInvalidArgs,
                format!("read_file arguments: {err}"),
            )
        })?;

        let resolved = path::resolve(&ctx.root, &args.path)?;

        let metadata = tokio::fs::metadata(&resolved).await.map_err(|_| {
            Error::new(
                ErrCode::ToolNotFound,
                format!("{} does not exist", args.path),
            )
        })?;
        if !metadata.is_file() {
            return Err(Error::new(
                ErrCode::ToolInvalidArgs,
                format!("{} is not a file", args.path),
            ));
        }

        let bytes = tokio::fs::read(&resolved).await.map_err(|err| {
            Error::new(
                ErrCode::ToolFailed,
                format!("cannot read {}: {err}", args.path),
            )
        })?;
        if bytes.contains(&0) {
            return Err(Error::new(
                ErrCode::ToolFailed,
                format!(
                    "{} looks like a binary file; read_file handles text only",
                    args.path
                ),
            ));
        }

        let text = String::from_utf8_lossy(&bytes);
        let lines: Vec<&str> = text.lines().collect();
        let total = lines.len();

        let offset = args.offset.unwrap_or(0);
        let limit = args.limit.unwrap_or(MAX_LINES).clamp(1, MAX_LINES);
        let start = offset.min(total);
        let end = start.saturating_add(limit).min(total);

        let mut out = String::new();
        if total == 0 {
            out.push_str("(empty file)\n");
        } else if start >= total {
            let _ = writeln!(
                out,
                "offset {offset} is past the end of the file ({total} lines total)."
            );
        } else {
            for (i, line) in lines[start..end].iter().enumerate() {
                let n = start + i + 1;
                let _ = writeln!(out, "{n:>6}\t{line}");
            }
            if end < total {
                let _ = write!(
                    out,
                    "\n… {} more line(s). Pass offset={end} to continue.\n",
                    total - end
                );
            }
        }

        self.state.record(&resolved, &bytes);

        Ok(ToolResult::ok(out))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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

    #[tokio::test]
    async fn reads_a_small_file_with_line_numbers() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "one\ntwo\nthree\n").unwrap();
        let tool = ReadFile::new(Arc::new(ReadState::new()));

        let result = tool
            .invoke(json!({"path": "a.txt"}), &ctx(dir.path()))
            .await
            .unwrap();

        assert!(!result.is_error);
        assert!(result.content.contains("     1\tone"));
        assert!(result.content.contains("     2\ttwo"));
        assert!(result.content.contains("     3\tthree"));
    }

    #[tokio::test]
    async fn a_missing_file_is_tool_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let tool = ReadFile::new(Arc::new(ReadState::new()));

        let err = tool
            .invoke(json!({"path": "missing.txt"}), &ctx(dir.path()))
            .await
            .unwrap_err();
        assert_eq!(err.code, ErrCode::ToolNotFound);
    }

    #[tokio::test]
    async fn a_path_outside_root_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let tool = ReadFile::new(Arc::new(ReadState::new()));

        let err = tool
            .invoke(json!({"path": "../evil.txt"}), &ctx(dir.path()))
            .await
            .unwrap_err();
        assert_eq!(err.code, ErrCode::ToolOutsideRoot);
    }

    #[tokio::test]
    async fn offset_and_limit_page_through_a_file() {
        let dir = tempfile::tempdir().unwrap();
        let mut content = String::new();
        for n in 1..=10 {
            let _ = writeln!(content, "line{n}");
        }
        std::fs::write(dir.path().join("a.txt"), content).unwrap();
        let tool = ReadFile::new(Arc::new(ReadState::new()));

        let result = tool
            .invoke(
                json!({"path": "a.txt", "offset": 5, "limit": 2}),
                &ctx(dir.path()),
            )
            .await
            .unwrap();

        assert!(result.content.contains("     6\tline6"));
        assert!(result.content.contains("     7\tline7"));
        assert!(!result.content.contains("line8"));
        assert!(result.content.contains("more line"));
    }

    #[tokio::test]
    async fn limit_is_capped_at_max_lines() {
        let dir = tempfile::tempdir().unwrap();
        let mut content = String::new();
        for n in 1..=(MAX_LINES + 50) {
            let _ = writeln!(content, "l{n}");
        }
        std::fs::write(dir.path().join("big.txt"), content).unwrap();
        let tool = ReadFile::new(Arc::new(ReadState::new()));

        let result = tool
            .invoke(
                json!({"path": "big.txt", "limit": MAX_LINES + 50}),
                &ctx(dir.path()),
            )
            .await
            .unwrap();

        let line_count = result.content.lines().filter(|l| l.contains('\t')).count();
        assert_eq!(line_count, MAX_LINES);
    }

    #[tokio::test]
    async fn a_successful_read_is_recorded_for_staleness_checks() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "hello").unwrap();
        let state = Arc::new(ReadState::new());
        let tool = ReadFile::new(state.clone());

        tool.invoke(json!({"path": "a.txt"}), &ctx(dir.path()))
            .await
            .unwrap();

        let resolved = dir.path().join("a.txt");
        assert!(state.check_fresh(&resolved, b"hello").is_ok());
    }

    #[tokio::test]
    async fn an_empty_file_reads_as_empty() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("empty.txt"), "").unwrap();
        let tool = ReadFile::new(Arc::new(ReadState::new()));

        let result = tool
            .invoke(json!({"path": "empty.txt"}), &ctx(dir.path()))
            .await
            .unwrap();
        assert!(result.content.contains("empty file"));
    }
}
