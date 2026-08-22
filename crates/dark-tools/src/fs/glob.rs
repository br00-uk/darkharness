//! The `glob` tool.

use async_trait::async_trait;
use dark_contract::{ErrCode, Error, Result, Tool, ToolCtx, ToolResult, ToolSchema, tool::tier};
use globset::Glob;
use serde::Deserialize;
use serde_json::{Value, json};
use std::fmt::Write;

/// The largest number of matches one `glob` call returns.
pub(crate) const MAX_MATCHES: usize = 1000;

#[derive(Debug, Deserialize)]
struct Args {
    pattern: String,
}

/// Finds files under the repository root by glob pattern.
///
/// The walk stays inside the repository root by construction, honours
/// `.gitignore`, and skips hidden entries (including `.git`), the same way
/// [`ignore::WalkBuilder`] behaves by default.
#[derive(Debug, Default)]
pub struct GlobTool;

impl GlobTool {
    /// Creates the tool.
    pub(crate) fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for GlobTool {
    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "glob".to_string(),
            description: "Finds files under the repository root that match a glob pattern, \
                for example `src/**/*.rs`. Honours .gitignore."
                .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "pattern": {
                        "type": "string",
                        "description": "A glob pattern, relative to the repository root.",
                    },
                },
                "required": ["pattern"],
            }),
            tier: tier::STANDARD,
            mutating: false,
        }
    }

    async fn invoke(&self, args: Value, ctx: &ToolCtx) -> Result<ToolResult> {
        let args: Args = serde_json::from_value(args).map_err(|err| {
            Error::new(ErrCode::ToolInvalidArgs, format!("glob arguments: {err}"))
        })?;

        let matcher = Glob::new(&args.pattern)
            .map_err(|err| {
                Error::new(ErrCode::ToolInvalidArgs, format!("bad glob pattern: {err}"))
            })?
            .compile_matcher();

        let root = ctx.root.clone();
        let pattern_matches = tokio::task::spawn_blocking(move || walk(&root, &matcher))
            .await
            .map_err(|err| {
                Error::new(
                    ErrCode::ToolFailed,
                    format!("the glob walk did not finish: {err}"),
                )
            })??;

        let mut paths = pattern_matches;
        paths.sort_by(|a: &String, b: &String| a.as_bytes().cmp(b.as_bytes()));
        let truncated = paths.len() > MAX_MATCHES;
        paths.truncate(MAX_MATCHES);

        let mut out = String::new();
        if paths.is_empty() {
            out.push_str("(no matches)\n");
        }
        for m in &paths {
            out.push_str(m);
            out.push('\n');
        }
        if truncated {
            let _ = write!(out, "\n… truncated at {MAX_MATCHES} matches.\n");
        }

        Ok(ToolResult::ok(out))
    }
}

fn walk(root: &std::path::Path, matcher: &globset::GlobMatcher) -> Result<Vec<String>> {
    let mut found = Vec::new();
    for entry in ignore::WalkBuilder::new(root).build() {
        let entry = entry.map_err(|err| {
            Error::new(
                ErrCode::ToolFailed,
                format!("cannot walk the repository: {err}"),
            )
        })?;
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let Ok(rel) = entry.path().strip_prefix(root) else {
            continue;
        };
        let normalized = rel.to_string_lossy().replace('\\', "/");
        if matcher.is_match(&normalized) {
            found.push(normalized);
        }
    }
    Ok(found)
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
    async fn matches_files_by_extension() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.rs"), "").unwrap();
        std::fs::write(dir.path().join("b.txt"), "").unwrap();
        let tool = GlobTool::new();

        let result = tool
            .invoke(json!({"pattern": "*.rs"}), &ctx(dir.path()))
            .await
            .unwrap();
        assert!(result.content.contains("a.rs"));
        assert!(!result.content.contains("b.txt"));
    }

    #[tokio::test]
    async fn double_star_matches_nested_files() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src/nested")).unwrap();
        std::fs::write(dir.path().join("src/nested/deep.rs"), "").unwrap();
        let tool = GlobTool::new();

        let result = tool
            .invoke(json!({"pattern": "**/*.rs"}), &ctx(dir.path()))
            .await
            .unwrap();
        assert!(result.content.contains("src/nested/deep.rs"));
    }

    #[tokio::test]
    async fn gitignored_files_are_excluded() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(".gitignore"), "ignored.rs\n").unwrap();
        std::fs::write(dir.path().join("ignored.rs"), "").unwrap();
        std::fs::write(dir.path().join("kept.rs"), "").unwrap();
        // ignore::WalkBuilder only reads .gitignore inside a git worktree by
        // default, so mark the temp dir as one.
        std::fs::create_dir(dir.path().join(".git")).unwrap();
        let tool = GlobTool::new();

        let result = tool
            .invoke(json!({"pattern": "*.rs"}), &ctx(dir.path()))
            .await
            .unwrap();
        assert!(result.content.contains("kept.rs"));
        assert!(!result.content.contains("ignored.rs"));
    }

    #[tokio::test]
    async fn an_invalid_pattern_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let tool = GlobTool::new();

        let err = tool
            .invoke(json!({"pattern": "["}), &ctx(dir.path()))
            .await
            .unwrap_err();
        assert_eq!(err.code, ErrCode::ToolInvalidArgs);
    }

    #[tokio::test]
    async fn no_matches_says_so() {
        let dir = tempfile::tempdir().unwrap();
        let tool = GlobTool::new();

        let result = tool
            .invoke(json!({"pattern": "*.nonexistent"}), &ctx(dir.path()))
            .await
            .unwrap();
        assert!(result.content.contains("no matches"));
    }
}
