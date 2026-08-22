//! The file tools: `read_file`, `write_file`, `edit_file`, `apply_patch`,
//! `list_dir`, and `glob`.
//!
//! Every tool here resolves the path it receives through [`path::resolve`]
//! before it touches the filesystem, so a path outside the repository root,
//! or a symbolic link that leaves it, is always refused. See Rule 34 of the
//! build specification.
//!
//! `write_file`, `edit_file`, and `apply_patch` share one [`state::ReadState`]
//! so that a write to an existing file requires a prior `read_file` call in
//! the same session, and fails when the file changed on disk since that
//! read. [`file_tools`] builds that shared state once; call it once per
//! session, not once per call.

mod apply_patch;
mod atomic;
mod diff_util;
mod edit_file;
mod glob;
mod list_dir;
mod path;
mod read_file;
mod state;
mod write_file;

use std::sync::Arc;

use dark_contract::Tool;

pub use apply_patch::ApplyPatch;
pub use edit_file::EditFile;
pub use glob::GlobTool;
pub use list_dir::ListDir;
pub use read_file::ReadFile;
pub use state::ReadState;
pub use write_file::WriteFile;

/// Builds the six file tools for one session.
///
/// Call this once per session. The returned tools share one [`ReadState`],
/// which is what lets `write_file` and `edit_file` see that `read_file` has
/// already visited a given path. Building a fresh set of tools per call
/// would silently defeat that check.
#[must_use]
pub fn file_tools() -> Vec<Box<dyn Tool>> {
    let state = Arc::new(ReadState::new());
    vec![
        Box::new(ReadFile::new(state.clone())),
        Box::new(WriteFile::new(state.clone())),
        Box::new(EditFile::new(state.clone())),
        Box::new(ApplyPatch::new(state)),
        Box::new(ListDir::new()),
        Box::new(GlobTool::new()),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_tools_returns_the_six_tools_at_their_documented_tiers() {
        let tools = file_tools();
        let schemas: Vec<_> = tools.iter().map(|t| t.schema()).collect();

        let names: Vec<&str> = schemas.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "read_file",
                "write_file",
                "edit_file",
                "apply_patch",
                "list_dir",
                "glob"
            ]
        );

        let mutating: Vec<bool> = schemas.iter().map(|s| s.mutating).collect();
        assert_eq!(mutating, vec![false, true, true, true, false, false]);

        for schema in &schemas {
            assert!(
                !schema.description.is_empty(),
                "{} has no description",
                schema.name
            );
            assert!(
                schema.tier >= 1 && schema.tier <= 3,
                "{} has an out-of-range tier",
                schema.name
            );
        }
    }

    #[tokio::test]
    async fn read_file_and_write_file_share_the_session_read_state() {
        use dark_contract::{ErrCode, ToolCtx};
        use tokio_util::sync::CancellationToken;

        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "old").unwrap();

        let tools = file_tools();
        let read_file = tools
            .iter()
            .find(|t| t.schema().name == "read_file")
            .unwrap();
        let write_file = tools
            .iter()
            .find(|t| t.schema().name == "write_file")
            .unwrap();

        let bus = dark_contract::EventBus::new();
        let ctx = ToolCtx {
            root: dir.path().to_path_buf(),
            events: bus.tx(),
            cancel: CancellationToken::new(),
            dark: true,
            human_present: false,
        };

        // Writing before reading fails.
        let err = write_file
            .invoke(serde_json::json!({"path": "a.txt", "content": "new"}), &ctx)
            .await
            .unwrap_err();
        assert_eq!(err.code, ErrCode::ToolStale);

        // Reading, then writing, succeeds, because both tools share one
        // ReadState.
        read_file
            .invoke(serde_json::json!({"path": "a.txt"}), &ctx)
            .await
            .unwrap();
        let result = write_file
            .invoke(serde_json::json!({"path": "a.txt", "content": "new"}), &ctx)
            .await
            .unwrap();
        assert!(!result.is_error);
    }
}
