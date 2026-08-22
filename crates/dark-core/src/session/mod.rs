//! The session: what one conversation holds between turns, and how the
//! harness records and replays it.
//!
//! [`Session`] is the state the turn loop (task unit `A2`) carries from one
//! turn to the next. [`TranscriptWriter`] persists the events that build
//! that state to `$DARK_HOME/sessions/<id>/transcript.jsonl` (section 5.3
//! of the build specification). [`Session::replay`] rebuilds `messages`
//! from a saved transcript; see the `transcript` submodule's documentation
//! for exactly what replay can and cannot reconstruct.
//!
//! This module never reads the `DARK_HOME` environment variable. Every
//! function that touches storage takes the sessions root as a parameter,
//! the same discipline `dark-agentsmd::resolve` uses for the home
//! directory.

mod transcript;

use std::path::{Path, PathBuf};

use dark_contract::{Message, Result};
use serde::{Deserialize, Serialize};
use ulid::Ulid;

pub use transcript::{TranscriptWriter, read_events, rebuild_messages, replay, transcript_path};

/// Token accounting for one session.
///
/// The two fields mirror [`dark_contract::Event::Budget`], which reports
/// the same numbers on the event bus.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Budget {
    /// Tokens in use.
    pub used: usize,
    /// Tokens that the resident set manager granted.
    pub granted: usize,
}

/// One conversation, and the state the turn loop needs to continue it.
///
/// A [`TranscriptWriter`] records the events that build this state.
/// [`Session::replay`] rebuilds `messages` from a saved transcript.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session {
    /// The session identifier. Names the transcript directory.
    pub id: Ulid,
    /// The repository root this session works in.
    pub root: PathBuf,
    /// The conversation so far, oldest first.
    pub messages: Vec<Message>,
    /// The session's token accounting.
    pub budget: Budget,
    /// Whether dark mode blocks network egress for this session.
    pub dark: bool,
    /// Whether a person can answer a confirmation now. See Rule 19.
    pub human_present: bool,
    /// How many tickets this session has resolved.
    ///
    /// `E_SESSION_RESOLUTION_LIMIT` fires once a session has already
    /// resolved a ticket and tries to resolve another.
    pub resolved_this_session: u32,
    // The hash of the context prefix that the turn in progress assembled.
    // Task unit A3 computes and stores this; it must stay fixed for the
    // whole turn (Rule 5), so only context assembly moves it, through
    // `set_prefix_hash`.
    prefix_hash: u64,
}

impl Session {
    /// Creates a new, empty session rooted at `root`.
    pub fn new(id: Ulid, root: PathBuf) -> Self {
        Self {
            id,
            root,
            messages: Vec::new(),
            budget: Budget::default(),
            dark: false,
            human_present: false,
            resolved_this_session: 0,
            prefix_hash: 0,
        }
    }

    /// Returns the hash of the context prefix that the turn in progress
    /// assembled.
    ///
    /// This value stays fixed for the whole turn (Rule 5): only a fresh
    /// call to context assembly, at the next turn's boundary, changes it.
    pub fn prefix_hash(&self) -> u64 {
        self.prefix_hash
    }

    /// Sets the prefix hash for the turn about to start.
    ///
    /// Only context assembly (task unit `A3`) should call this, and only
    /// at a turn boundary — never mid-turn (Rule 5). The field stays
    /// private so that no other code can build a `Session` by struct
    /// literal, or write to it, outside this method.
    pub fn set_prefix_hash(&mut self, hash: u64) {
        self.prefix_hash = hash;
    }

    /// Rebuilds a session's message list from its transcript.
    ///
    /// Only `messages` comes from the transcript. [`dark_contract::Event`]
    /// carries no snapshot of `budget`, `dark`, `human_present`, or
    /// `resolved_this_session`, so this starts a fresh session at `root`
    /// (see [`Session::new`]) and replaces its `messages` with the replay
    /// result.
    ///
    /// # Errors
    ///
    /// Returns an error when the transcript cannot be read. See
    /// [`replay`].
    pub async fn replay(sessions_root: &Path, id: Ulid, root: PathBuf) -> Result<Self> {
        let messages = transcript::replay(sessions_root, id).await?;
        Ok(Self {
            messages,
            ..Self::new(id, root)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dark_contract::{Event, ToolCall, ToolResultSummary};
    use dark_contract::{Role, RoleClass, Usage};
    use tempfile::TempDir;

    #[test]
    fn new_creates_an_empty_session_with_zeroed_state() {
        let id = Ulid::new();
        let root = PathBuf::from("/repo");
        let session = Session::new(id, root.clone());

        assert_eq!(session.id, id);
        assert_eq!(session.root, root);
        assert!(session.messages.is_empty());
        assert_eq!(session.budget, Budget::default());
        assert!(!session.dark);
        assert!(!session.human_present);
        assert_eq!(session.resolved_this_session, 0);
        assert_eq!(session.prefix_hash(), 0);
    }

    #[test]
    fn set_prefix_hash_updates_the_private_field() {
        let mut session = Session::new(Ulid::new(), PathBuf::from("/repo"));
        session.set_prefix_hash(42);
        assert_eq!(session.prefix_hash(), 42);
    }

    #[tokio::test]
    async fn replay_rebuilds_messages_and_defaults_the_rest() {
        let tmp = TempDir::new().unwrap();
        let id = Ulid::new();
        let root = PathBuf::from("/repo");

        let mut writer = TranscriptWriter::open(tmp.path(), id).await.unwrap();
        writer
            .record(&Event::TurnStart {
                turn: "t1".to_owned(),
                class: RoleClass::Worker,
                model: "test-model".to_owned(),
            })
            .await
            .unwrap();
        writer
            .record(&Event::TokenDelta {
                turn: "t1".to_owned(),
                text: "hi there".to_owned(),
            })
            .await
            .unwrap();
        writer
            .record(&Event::TurnEnd {
                turn: "t1".to_owned(),
                usage: Usage::default(),
                wall_ms: 5,
            })
            .await
            .unwrap();

        let session = Session::replay(tmp.path(), id, root.clone()).await.unwrap();

        assert_eq!(session.id, id);
        assert_eq!(session.root, root);
        assert_eq!(
            session.messages,
            vec![Message::text(Role::Assistant, "hi there")]
        );
        // Fields the transcript cannot carry stay at their fresh-session
        // defaults.
        assert_eq!(session.budget, Budget::default());
        assert!(!session.dark);
        assert!(!session.human_present);
        assert_eq!(session.resolved_this_session, 0);
    }

    #[test]
    fn rebuild_messages_is_reachable_from_the_session_module() {
        // Exercises the re-export directly, independent of the writer.
        let events = vec![
            Event::TurnStart {
                turn: "t1".to_owned(),
                class: RoleClass::Scout,
                model: "m".to_owned(),
            },
            Event::ToolCall {
                turn: "t1".to_owned(),
                call: ToolCall {
                    id: "c1".to_owned(),
                    name: "read_file".to_owned(),
                    args: serde_json::json!({}),
                },
            },
            Event::ToolResult {
                turn: "t1".to_owned(),
                call_id: "c1".to_owned(),
                result: ToolResultSummary {
                    name: "read_file".to_owned(),
                    is_error: false,
                    bytes: 1,
                    headline: "ok".to_owned(),
                    has_diff: false,
                },
                content: "ok".to_owned(),
            },
            Event::TurnEnd {
                turn: "t1".to_owned(),
                usage: Usage::default(),
                wall_ms: 1,
            },
        ];

        let messages = rebuild_messages(&events);
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].role, Role::Assistant);
        assert_eq!(messages[0].tool_calls.len(), 1);
        assert_eq!(messages[1].role, Role::Tool);
        assert_eq!(messages[1].tool_call_id.as_deref(), Some("c1"));
    }
}
