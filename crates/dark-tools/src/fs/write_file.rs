//! The `write_file` tool.

use std::sync::Arc;

use async_trait::async_trait;
use dark_contract::{ErrCode, Error, Result, Tool, ToolCtx, ToolResult, ToolSchema, tool::tier};
use serde::Deserialize;
use serde_json::{Value, json};

use super::atomic;
use super::diff_util;
use super::path;
use super::state::ReadState;

#[derive(Debug, Deserialize)]
struct Args {
    path: String,
    content: String,
}

/// Replaces a file's whole content, or creates a new file.
///
/// Rule 34 and task unit `C1` require every mutating file tool to refuse a
/// path outside the repository root, and to refuse an existing file that
/// this session has not read (or that changed on disk since it did).
#[derive(Debug)]
pub struct WriteFile {
    state: Arc<ReadState>,
}

impl WriteFile {
    /// Creates the tool, sharing `state` with the other file tools in this
    /// session.
    pub(crate) fn new(state: Arc<ReadState>) -> Self {
        Self { state }
    }
}

#[async_trait]
impl Tool for WriteFile {
    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "write_file".to_string(),
            description: "Writes the whole content of a file, creating it if it does not \
                exist. Read an existing file in this session before you overwrite it."
                .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "The file path, relative to the repository root.",
                    },
                    "content": {
                        "type": "string",
                        "description": "The full new content of the file.",
                    },
                },
                "required": ["path", "content"],
            }),
            tier: tier::ESSENTIAL,
            mutating: true,
        }
    }

    async fn invoke(&self, args: Value, ctx: &ToolCtx) -> Result<ToolResult> {
        let args: Args = serde_json::from_value(args).map_err(|err| {
            Error::new(
                ErrCode::ToolInvalidArgs,
                format!("write_file arguments: {err}"),
            )
        })?;

        let resolved = path::resolve(&ctx.root, &args.path)?;

        let existing = tokio::fs::read(&resolved).await.ok();
        let mode = if existing.is_some() {
            tokio::fs::metadata(&resolved)
                .await
                .ok()
                .map(|m| m.permissions())
        } else {
            None
        };

        if let Some(existing_bytes) = &existing {
            self.state.check_fresh(&resolved, existing_bytes)?;
        }

        let old_text = existing
            .as_deref()
            .map(String::from_utf8_lossy)
            .unwrap_or_default()
            .into_owned();

        let new_bytes = args.content.clone().into_bytes();
        atomic::write(resolved.clone(), new_bytes.clone(), mode).await?;
        self.state.record(&resolved, &new_bytes);

        let diff = diff_util::render(&args.path, &old_text, &args.content);
        let verb = if existing.is_some() {
            "wrote"
        } else {
            "created"
        };
        let mut result = ToolResult::ok(format!("{verb} {}", args.path));
        if let Some(diff) = diff {
            result = result.with_diff(diff);
        }
        Ok(result)
    }
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

    #[tokio::test]
    async fn creates_a_new_file_without_a_prior_read() {
        let dir = tempfile::tempdir().unwrap();
        let tool = WriteFile::new(Arc::new(ReadState::new()));

        let result = tool
            .invoke(
                json!({"path": "new.txt", "content": "hello\n"}),
                &ctx(dir.path()),
            )
            .await
            .unwrap();

        assert!(!result.is_error);
        assert_eq!(
            std::fs::read_to_string(dir.path().join("new.txt")).unwrap(),
            "hello\n"
        );
        let diff = result.diff.unwrap();
        assert!(diff.contains("+hello"));
    }

    #[tokio::test]
    async fn overwriting_an_existing_file_without_a_read_is_stale() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "old").unwrap();
        let tool = WriteFile::new(Arc::new(ReadState::new()));

        let err = tool
            .invoke(json!({"path": "a.txt", "content": "new"}), &ctx(dir.path()))
            .await
            .unwrap_err();
        assert_eq!(err.code, ErrCode::ToolStale);
        assert_eq!(
            std::fs::read_to_string(dir.path().join("a.txt")).unwrap(),
            "old"
        );
    }

    #[tokio::test]
    async fn overwriting_after_a_read_succeeds() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "old").unwrap();
        let state = Arc::new(ReadState::new());
        let reader = ReadFile::new(state.clone());
        let writer = WriteFile::new(state);

        reader
            .invoke(json!({"path": "a.txt"}), &ctx(dir.path()))
            .await
            .unwrap();
        let result = writer
            .invoke(json!({"path": "a.txt", "content": "new"}), &ctx(dir.path()))
            .await
            .unwrap();

        assert!(!result.is_error);
        assert_eq!(
            std::fs::read_to_string(dir.path().join("a.txt")).unwrap(),
            "new"
        );
    }

    #[tokio::test]
    async fn a_change_on_disk_after_the_read_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "old").unwrap();
        let state = Arc::new(ReadState::new());
        let reader = ReadFile::new(state.clone());
        let writer = WriteFile::new(state);

        reader
            .invoke(json!({"path": "a.txt"}), &ctx(dir.path()))
            .await
            .unwrap();
        std::fs::write(dir.path().join("a.txt"), "changed by someone else").unwrap();

        let err = writer
            .invoke(json!({"path": "a.txt", "content": "new"}), &ctx(dir.path()))
            .await
            .unwrap_err();
        assert_eq!(err.code, ErrCode::ToolStale);
        assert_eq!(
            std::fs::read_to_string(dir.path().join("a.txt")).unwrap(),
            "changed by someone else"
        );
    }

    #[tokio::test]
    async fn a_path_outside_root_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let tool = WriteFile::new(Arc::new(ReadState::new()));

        let err = tool
            .invoke(
                json!({"path": "../evil.txt", "content": "x"}),
                &ctx(dir.path()),
            )
            .await
            .unwrap_err();
        assert_eq!(err.code, ErrCode::ToolOutsideRoot);
    }

    #[tokio::test]
    async fn a_write_creates_missing_parent_directories() {
        let dir = tempfile::tempdir().unwrap();
        let tool = WriteFile::new(Arc::new(ReadState::new()));

        tool.invoke(
            json!({"path": "a/b/c.txt", "content": "nested"}),
            &ctx(dir.path()),
        )
        .await
        .unwrap();

        assert_eq!(
            std::fs::read_to_string(dir.path().join("a/b/c.txt")).unwrap(),
            "nested"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn overwriting_preserves_the_file_mode() {
        use std::os::unix::fs::PermissionsExt as _;

        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("script.sh");
        std::fs::write(&target, "old").unwrap();
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o755)).unwrap();

        let state = Arc::new(ReadState::new());
        ReadFile::new(state.clone())
            .invoke(json!({"path": "script.sh"}), &ctx(dir.path()))
            .await
            .unwrap();
        WriteFile::new(state)
            .invoke(
                json!({"path": "script.sh", "content": "new"}),
                &ctx(dir.path()),
            )
            .await
            .unwrap();

        let mode = std::fs::metadata(&target).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o755);
    }
}
