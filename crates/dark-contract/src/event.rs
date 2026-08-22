//! Events that flow out of the harness, and intents that flow in.
//!
//! The bus carries events on two broadcast channels. One channel carries
//! [`Event::TokenDelta`] and [`Event::ReasonDelta`]. The second channel
//! carries every other event. A slow subscriber therefore loses streaming
//! text but never loses a tool result, a turn boundary, or an error.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

use crate::{ErrCode, ResidencySnapshot, RoleClass, ToolCall, ToolResultSummary, Usage};

/// The default capacity of the lossy channel.
pub const LOSSY_CAPACITY: usize = 1024;

/// The default capacity of the reliable channel.
///
/// This channel is larger because dropping from it loses real information.
pub const RELIABLE_CAPACITY: usize = 8192;

/// What a person must confirm before a mutating action runs.
///
/// Each variant carries the exact change, never a summary. See task unit `A4`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConfirmPrompt {
    /// A file write. `diff` is the exact unified diff.
    Write {
        /// The file that changes.
        path: PathBuf,
        /// The exact unified diff.
        diff: String,
    },
    /// A command. `command` is the exact command line.
    Exec {
        /// The exact command.
        command: String,
        /// The working directory.
        cwd: PathBuf,
        /// Whether a shell interprets the command.
        shell: bool,
    },
    /// Any other action.
    Other {
        /// One line that names the action.
        summary: String,
        /// The full detail.
        detail: String,
    },
}

/// The answer to a [`ConfirmPrompt`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Allow {
    /// Allow this one action.
    Once,
    /// Allow this action and every later action of the same shape.
    Always,
    /// Refuse the action.
    Deny,
}

/// Something that happened.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum Event {
    /// A session started.
    SessionStart {
        /// The session identifier.
        id: String,
        /// The repository root.
        root: PathBuf,
    },
    /// A turn started.
    TurnStart {
        /// The turn identifier.
        turn: String,
        /// The role class that serves this turn.
        class: RoleClass,
        /// The model that serves this turn.
        model: String,
    },
    /// The text a person submitted, recorded as it enters the turn.
    ///
    /// [`Intent::Submit`] runs the other way, from the terminal application
    /// into the harness, and is never written to the transcript. Without
    /// this event a replay rebuilds assistant and tool messages but never a
    /// `Role::User` one, so a session cannot be reconstructed from its own
    /// record. See task unit `A1`.
    UserMessage {
        /// The turn this message opens.
        turn: String,
        /// What the person wrote.
        text: String,
    },
    /// Visible output. This event travels on the lossy channel.
    TokenDelta {
        /// The turn identifier.
        turn: String,
        /// The next piece of text.
        text: String,
    },
    /// Thinking output. This event travels on the lossy channel.
    ReasonDelta {
        /// The turn identifier.
        turn: String,
        /// The next piece of thinking text.
        text: String,
    },
    /// A model load is in progress.
    ModelLoading {
        /// The model that is loading.
        model: String,
        /// Progress between 0.0 and 1.0.
        progress: f32,
    },
    /// The model asked for a tool call.
    ToolCall {
        /// The turn identifier.
        turn: String,
        /// The call.
        call: ToolCall,
    },
    /// A running tool produced a line of output.
    ToolProgress {
        /// The turn identifier.
        turn: String,
        /// The call identifier.
        call_id: String,
        /// One line of output.
        line: String,
    },
    /// A tool finished.
    ToolResult {
        /// The turn identifier.
        turn: String,
        /// The call identifier.
        call_id: String,
        /// The compact result, for a one-line display.
        result: ToolResultSummary,
        /// The full text that goes back to the model.
        ///
        /// [`ToolResultSummary`] keeps only a headline, which is what a
        /// display needs. A transcript needs the whole text, or a replayed
        /// `Role::Tool` message loses the tool's output. See task unit `A1`.
        content: String,
    },
    /// A turn finished.
    TurnEnd {
        /// The turn identifier.
        turn: String,
        /// The token counts.
        usage: Usage,
        /// How long the turn took.
        wall_ms: u64,
    },
    /// The context budget changed.
    Budget {
        /// Tokens in use.
        used: usize,
        /// Tokens that the resident set manager granted.
        granted: usize,
    },
    /// The resident set changed.
    Residency(ResidencySnapshot),
    /// Dark mode turned on or off.
    DarkChanged {
        /// Whether the harness now blocks network egress.
        dark: bool,
    },
    /// A map changed.
    MapChanged {
        /// The map identifier.
        map_id: String,
    },
    /// Repository analysis finished.
    ExploreDone {
        /// The tree hash that the analysis covered.
        tree_sha: String,
        /// Where the harness wrote the result.
        path: PathBuf,
    },
    /// Pack indexing is in progress.
    IndexProgress {
        /// The pack name.
        pack: String,
        /// Chunks indexed so far.
        done: usize,
        /// Chunks in total.
        total: usize,
    },
    /// A mutating action needs a confirmation.
    ConfirmReq {
        /// The identifier that the matching [`Intent::Confirm`] carries.
        id: String,
        /// What the person must confirm.
        prompt: ConfirmPrompt,
    },
    /// Something failed.
    Error {
        /// The stable code.
        code: ErrCode,
        /// The message.
        msg: String,
        /// The action that clears the error.
        remedy: Option<String>,
    },
    /// A message for the person that is not an error.
    Notice(String),
}

impl Event {
    /// Returns true when this event travels on the lossy channel.
    ///
    /// Only streaming text is lossy. Losing a token delta costs nothing that
    /// the transcript cannot rebuild. Losing a tool result costs correctness.
    pub fn is_lossy(&self) -> bool {
        matches!(self, Self::TokenDelta { .. } | Self::ReasonDelta { .. })
    }
}

/// Something that the person asked for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum Intent {
    /// Send this text as an input message.
    Submit(String),
    /// Cancel the running turn.
    Cancel,
    /// Answer a [`Event::ConfirmReq`].
    Confirm {
        /// The identifier from the request.
        id: String,
        /// The answer.
        allow: Allow,
    },
    /// Run an in-session command, for example `/plan`.
    Command(String),
    /// Enter or leave dark mode.
    GoDark(bool),
    /// Leave the application.
    Quit,
}

/// Sends events on the correct channel.
#[derive(Debug, Clone)]
pub struct EventTx {
    lossy: broadcast::Sender<Event>,
    reliable: broadcast::Sender<Event>,
}

impl EventTx {
    /// Sends an event.
    ///
    /// The event goes on the lossy channel when [`Event::is_lossy`] is true,
    /// and on the reliable channel otherwise. A send with no subscriber is not
    /// an error, so a headless run never fails for want of a listener.
    pub fn send(&self, event: Event) {
        let channel = if event.is_lossy() {
            &self.lossy
        } else {
            &self.reliable
        };
        let _ = channel.send(event);
    }

    /// Sends a [`Event::Notice`].
    pub fn notice(&self, text: impl Into<String>) {
        self.send(Event::Notice(text.into()));
    }

    /// Sends an [`Event::Error`] built from an [`crate::Error`].
    pub fn error(&self, err: &crate::Error) {
        self.send(Event::Error {
            code: err.code,
            msg: err.message.clone(),
            remedy: err.remedy.clone(),
        });
    }
}

/// What [`EventRx::recv`] returned.
#[derive(Debug, Clone, PartialEq)]
pub enum Received {
    /// One event.
    Event(Event),
    /// The subscriber was too slow and missed this many events.
    ///
    /// The terminal application shows a warning glyph. See task unit `H4`.
    Lagged(u64),
}

/// Receives events from both channels.
#[derive(Debug)]
pub struct EventRx {
    lossy: broadcast::Receiver<Event>,
    reliable: broadcast::Receiver<Event>,
    lossy_closed: bool,
    reliable_closed: bool,
}

impl EventRx {
    /// Waits for the next event.
    ///
    /// Returns `None` when both channels have closed. The reliable channel has
    /// priority, so a burst of token deltas never starves a tool result.
    pub async fn recv(&mut self) -> Option<Received> {
        loop {
            if self.lossy_closed && self.reliable_closed {
                return None;
            }

            tokio::select! {
                biased;

                result = self.reliable.recv(), if !self.reliable_closed => {
                    match result {
                        Ok(event) => return Some(Received::Event(event)),
                        Err(broadcast::error::RecvError::Lagged(n)) => {
                            return Some(Received::Lagged(n));
                        }
                        Err(broadcast::error::RecvError::Closed) => {
                            self.reliable_closed = true;
                        }
                    }
                }

                result = self.lossy.recv(), if !self.lossy_closed => {
                    match result {
                        Ok(event) => return Some(Received::Event(event)),
                        Err(broadcast::error::RecvError::Lagged(n)) => {
                            return Some(Received::Lagged(n));
                        }
                        Err(broadcast::error::RecvError::Closed) => {
                            self.lossy_closed = true;
                        }
                    }
                }
            }
        }
    }
}

/// Owns both broadcast channels.
#[derive(Debug)]
pub struct EventBus {
    lossy: broadcast::Sender<Event>,
    reliable: broadcast::Sender<Event>,
}

impl EventBus {
    /// Creates a bus with the default capacities.
    pub fn new() -> Self {
        Self::with_capacity(LOSSY_CAPACITY, RELIABLE_CAPACITY)
    }

    /// Creates a bus with explicit capacities.
    ///
    /// # Panics
    ///
    /// Panics when either capacity is zero.
    pub fn with_capacity(lossy: usize, reliable: usize) -> Self {
        assert!(
            lossy > 0 && reliable > 0,
            "channel capacity must be greater than zero"
        );
        Self {
            lossy: broadcast::channel(lossy).0,
            reliable: broadcast::channel(reliable).0,
        }
    }

    /// Returns a sender.
    pub fn tx(&self) -> EventTx {
        EventTx {
            lossy: self.lossy.clone(),
            reliable: self.reliable.clone(),
        }
    }

    /// Returns a receiver that sees events sent from now on.
    pub fn subscribe(&self) -> EventRx {
        EventRx {
            lossy: self.lossy.subscribe(),
            reliable: self.reliable.subscribe(),
            lossy_closed: false,
            reliable_closed: false,
        }
    }
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn token(text: &str) -> Event {
        Event::TokenDelta {
            turn: "t1".into(),
            text: text.into(),
        }
    }

    fn notice(text: &str) -> Event {
        Event::Notice(text.into())
    }

    #[test]
    fn only_streaming_text_is_lossy() {
        assert!(token("a").is_lossy());
        assert!(
            Event::ReasonDelta {
                turn: "t1".into(),
                text: "a".into()
            }
            .is_lossy()
        );
        assert!(!notice("a").is_lossy());
        assert!(!Event::DarkChanged { dark: true }.is_lossy());
    }

    #[tokio::test]
    async fn a_subscriber_receives_from_both_channels() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        let tx = bus.tx();

        tx.send(notice("hello"));
        tx.send(token("world"));

        let mut seen = Vec::new();
        for _ in 0..2 {
            if let Some(Received::Event(event)) = rx.recv().await {
                seen.push(event);
            }
        }
        assert!(seen.contains(&notice("hello")));
        assert!(seen.contains(&token("world")));
    }

    #[tokio::test]
    async fn a_slow_subscriber_loses_only_the_lossy_channel() {
        // This is the property the two-channel split exists for.
        let bus = EventBus::with_capacity(2, 64);
        let mut rx = bus.subscribe();
        let tx = bus.tx();

        // Overflow the lossy channel many times over.
        for i in 0..50 {
            tx.send(token(&i.to_string()));
        }
        // The reliable channel stays well inside its capacity.
        tx.send(Event::DarkChanged { dark: true });

        let mut lagged = false;
        let mut got_dark_changed = false;
        let mut delivered_tokens = 0_usize;

        // Drain what is buffered. The bus is still alive, so stop at the first
        // timeout rather than waiting for the channels to close.
        while let Ok(Some(received)) =
            tokio::time::timeout(std::time::Duration::from_millis(50), rx.recv()).await
        {
            match received {
                Received::Lagged(_) => lagged = true,
                Received::Event(Event::DarkChanged { dark }) => {
                    assert!(dark);
                    got_dark_changed = true;
                }
                Received::Event(Event::TokenDelta { .. }) => delivered_tokens += 1,
                Received::Event(_) => {}
            }
        }

        assert!(
            got_dark_changed,
            "the reliable event must survive the flood"
        );
        assert!(lagged, "the lossy channel should report a lag");
        assert!(
            delivered_tokens <= 2,
            "only the lossy capacity should survive, got {delivered_tokens}"
        );
    }

    #[tokio::test]
    async fn recv_returns_none_once_the_bus_drops() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        drop(bus);
        assert!(rx.recv().await.is_none());
    }

    #[test]
    fn sending_with_no_subscriber_is_not_an_error() {
        // A headless run has no listener and must not fail.
        let bus = EventBus::new();
        bus.tx().notice("nobody is listening");
    }

    #[test]
    fn error_events_carry_the_code_and_the_remedy() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        let err = crate::Error::new(ErrCode::PolicyDark, "blocked");
        bus.tx().error(&err);

        match rx.reliable.try_recv().unwrap() {
            Event::Error { code, msg, remedy } => {
                assert_eq!(code, ErrCode::PolicyDark);
                assert_eq!(msg, "blocked");
                assert_eq!(
                    remedy.as_deref(),
                    Some("Run /golight to allow the network.")
                );
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }
}
