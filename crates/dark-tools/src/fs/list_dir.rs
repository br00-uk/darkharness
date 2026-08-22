//! The `list_dir` tool.

use async_trait::async_trait;
use dark_contract::{ErrCode, Error, Result, Tool, ToolCtx, ToolResult, ToolSchema, tool::tier};
use serde::Deserialize;
use serde_json::{Value, json};

use super::path;
use std::fmt::Write;

/// The largest number of entries one `list_dir` call returns.
pub(crate) const MAX_ENTRIES: usize = 2000;

#[derive(Debug, Deserialize)]
struct Args {
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    recursive: bool,
}

/// Lists the entries of one directory in the repository.
#[derive(Debug, Default)]
pub struct ListDir;

impl ListDir {
    /// Creates the tool.
    pub(crate) fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for ListDir {
    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "list_dir".to_string(),
            description: "Lists the files and directories under one directory in the \
                repository. A directory entry ends with a `/`."
                .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "The directory, relative to the repository root. \
                            Defaults to the root.",
                    },
                    "recursive": {
                        "type": "boolean",
                        "description": "List every descendant, not just direct children. \
                            Default false.",
                    },
                },
            }),
            tier: tier::ESSENTIAL,
            mutating: false,
        }
    }

    async fn invoke(&self, args: Value, ctx: &ToolCtx) -> Result<ToolResult> {
        let args: Args = serde_json::from_value(args).map_err(|err| {
            Error::new(
                ErrCode::ToolInvalidArgs,
                format!("list_dir arguments: {err}"),
            )
        })?;

        let requested = args.path.clone().unwrap_or_default();
        let resolved = path::resolve(&ctx.root, &requested)?;

        let metadata = tokio::fs::metadata(&resolved).await.map_err(|_| {
            Error::new(ErrCode::ToolNotFound, format!("{requested} does not exist"))
        })?;
        if !metadata.is_dir() {
            return Err(Error::new(
                ErrCode::ToolInvalidArgs,
                format!("{requested} is not a directory"),
            ));
        }

        let mut entries = Vec::new();
        collect(&resolved, &resolved, args.recursive, &mut entries)?;
        entries
            .sort_by(|a: &(String, bool), b: &(String, bool)| a.0.as_bytes().cmp(b.0.as_bytes()));

        let truncated = entries.len() > MAX_ENTRIES;
        entries.truncate(MAX_ENTRIES);

        let mut out = String::new();
        if entries.is_empty() {
            out.push_str("(empty directory)\n");
        }
        for (rel, is_dir) in &entries {
            if *is_dir {
                let _ = writeln!(out, "{rel}/");
            } else {
                let _ = writeln!(out, "{rel}");
            }
        }
        if truncated {
            let _ = writeln!(out, "\n… truncated at {MAX_ENTRIES} entries.");
        }

        Ok(ToolResult::ok(out))
    }
}

/// Walks `dir` (a real filesystem path) and pushes `(relative_path, is_dir)`
/// pairs, relative to `base`, using byte order for determinism.
fn collect(
    base: &std::path::Path,
    dir: &std::path::Path,
    recursive: bool,
    out: &mut Vec<(String, bool)>,
) -> Result<()> {
    let read = std::fs::read_dir(dir).map_err(|err| {
        Error::new(
            ErrCode::ToolFailed,
            format!("cannot list {}: {err}", dir.display()),
        )
    })?;

    for entry in read {
        let entry = entry.map_err(|err| {
            Error::new(
                ErrCode::ToolFailed,
                format!("cannot read a directory entry: {err}"),
            )
        })?;
        let file_type = entry.file_type().map_err(|err| {
            Error::new(
                ErrCode::ToolFailed,
                format!("cannot read a directory entry: {err}"),
            )
        })?;
        let full = entry.path();
        let rel = full
            .strip_prefix(base)
            .unwrap_or(&full)
            .to_string_lossy()
            .replace('\\', "/");

        if file_type.is_dir() {
            out.push((rel, true));
            if recursive {
                collect(base, &full, recursive, out)?;
            }
        } else {
            out.push((rel, false));
        }
    }
    Ok(())
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
    async fn lists_the_root_by_default() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "").unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        let tool = ListDir::new();

        let result = tool.invoke(json!({}), &ctx(dir.path())).await.unwrap();

        assert!(result.content.contains("a.txt"));
        assert!(result.content.contains("sub/"));
    }

    #[tokio::test]
    async fn non_recursive_by_default_omits_nested_files() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        std::fs::write(dir.path().join("sub/nested.txt"), "").unwrap();
        let tool = ListDir::new();

        let result = tool.invoke(json!({}), &ctx(dir.path())).await.unwrap();
        assert!(!result.content.contains("nested.txt"));
    }

    #[tokio::test]
    async fn recursive_lists_nested_files() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        std::fs::write(dir.path().join("sub/nested.txt"), "").unwrap();
        let tool = ListDir::new();

        let result = tool
            .invoke(json!({"recursive": true}), &ctx(dir.path()))
            .await
            .unwrap();
        assert!(result.content.contains("sub/nested.txt"));
    }

    #[tokio::test]
    async fn a_missing_directory_is_tool_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let tool = ListDir::new();

        let err = tool
            .invoke(json!({"path": "missing"}), &ctx(dir.path()))
            .await
            .unwrap_err();
        assert_eq!(err.code, ErrCode::ToolNotFound);
    }

    #[tokio::test]
    async fn a_path_outside_root_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let tool = ListDir::new();

        let err = tool
            .invoke(json!({"path": "../"}), &ctx(dir.path()))
            .await
            .unwrap_err();
        assert_eq!(err.code, ErrCode::ToolOutsideRoot);
    }

    #[tokio::test]
    async fn entries_sort_in_byte_order() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("b.txt"), "").unwrap();
        std::fs::write(dir.path().join("a.txt"), "").unwrap();
        let tool = ListDir::new();

        let result = tool.invoke(json!({}), &ctx(dir.path())).await.unwrap();
        let a_pos = result.content.find("a.txt").unwrap();
        let b_pos = result.content.find("b.txt").unwrap();
        assert!(a_pos < b_pos);
    }
}
