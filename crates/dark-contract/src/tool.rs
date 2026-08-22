//! The tool interface.

use std::path::PathBuf;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::{EventTx, Result};

/// The tier that decides whether a model sees a tool.
///
/// A small model gets tier 1 only. See task unit `C4`.
pub mod tier {
    /// Tools that every model needs.
    pub const ESSENTIAL: u8 = 1;
    /// Tools that a mid-sized model handles well.
    pub const STANDARD: u8 = 2;
    /// Tools that only a large model should see.
    pub const ADVANCED: u8 = 3;
}

/// The description of a tool that the harness sends to a model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolSchema {
    /// The name that the model calls.
    pub name: String,
    /// What the tool does.
    pub description: String,
    /// The JSON schema for the arguments.
    pub parameters: serde_json::Value,
    /// 1 essential, 2 standard, 3 advanced. See [`tier`].
    pub tier: u8,
    /// Whether the tool changes anything. A mutating tool needs a policy check.
    pub mutating: bool,
}

/// What a tool produced.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ToolResult {
    /// The text that goes back to the model.
    pub content: String,
    /// A unified diff, when the tool changed a file.
    pub diff: Option<String>,
    /// Whether this result reports a failure.
    ///
    /// A failed tool still returns a `Role::Tool` message. An unanswered tool
    /// call breaks the chat template. See task unit `A2`.
    pub is_error: bool,
}

impl ToolResult {
    /// Creates a successful result.
    pub fn ok(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            diff: None,
            is_error: false,
        }
    }

    /// Creates a failed result.
    pub fn error(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            diff: None,
            is_error: true,
        }
    }

    /// Attaches a unified diff.
    #[must_use]
    pub fn with_diff(mut self, diff: impl Into<String>) -> Self {
        self.diff = Some(diff.into());
        self
    }
}

/// The compact form of a [`ToolResult`] that travels on the event bus.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolResultSummary {
    /// The tool name.
    pub name: String,
    /// Whether the call failed.
    pub is_error: bool,
    /// The size of the result text.
    pub bytes: usize,
    /// The first line of the result, for a one-line display.
    pub headline: String,
    /// Whether the tool produced a diff.
    pub has_diff: bool,
}

/// What a tool may use while it runs.
pub struct ToolCtx {
    /// The repository root. A tool never leaves this directory. See Rule 34.
    pub root: PathBuf,
    /// Where the tool sends progress events.
    pub events: EventTx,
    /// Cancels the tool.
    pub cancel: CancellationToken,
    /// Whether dark mode blocks network egress.
    pub dark: bool,
    /// Whether a person can answer a question now. See Rule 19.
    pub human_present: bool,
}

/// One action that a model can take.
#[async_trait]
pub trait Tool: Send + Sync {
    /// Returns the description that the model sees.
    fn schema(&self) -> ToolSchema;

    /// Runs the tool.
    ///
    /// # Errors
    ///
    /// Returns an error when the arguments are not valid, when the policy
    /// denies the action, or when the action fails.
    async fn invoke(&self, args: serde_json::Value, ctx: &ToolCtx) -> Result<ToolResult>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ok_and_error_set_the_flag() {
        assert!(!ToolResult::ok("done").is_error);
        assert!(ToolResult::error("no such file").is_error);
    }

    #[test]
    fn with_diff_attaches_the_diff() {
        let result = ToolResult::ok("wrote").with_diff("@@ -1 +1 @@");
        assert_eq!(result.diff.as_deref(), Some("@@ -1 +1 @@"));
    }

    #[test]
    fn the_trait_is_object_safe() {
        // The registry stores tools as `Box<dyn Tool>`.
        fn assert_object_safe(_: Option<&dyn Tool>) {}
        assert_object_safe(None);
    }
}
