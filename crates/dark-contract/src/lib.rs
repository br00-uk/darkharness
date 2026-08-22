//! Shared types, traits, and events for the darkharness workspace.
//!
//! Every other crate depends on this one. This crate depends on no other
//! workspace crate, so it defines the seam that the whole harness is built
//! around: `dark-tui` sends [`Intent`] values and renders [`Event`] values,
//! `dark-core` runs the turn loop against the [`Engine`] trait, and only
//! `dark-engine` knows that mistral.rs exists.
//!
//! Keep this crate free of heavy dependencies. See Rule 15.
//!
//! ```
//! use dark_contract::{EventBus, Event, Received};
//!
//! # #[tokio::main(flavor = "current_thread")]
//! # async fn main() {
//! let bus = EventBus::new();
//! let mut rx = bus.subscribe();
//! bus.tx().notice("ready");
//!
//! match rx.recv().await {
//!     Some(Received::Event(Event::Notice(text))) => assert_eq!(text, "ready"),
//!     other => panic!("unexpected: {other:?}"),
//! }
//! # }
//! ```

pub mod engine;
pub mod error;
pub mod event;
pub mod message;
pub mod tool;

pub use engine::{
    Caps, Chunk, ChunkStream, Device, EmbedPurpose, Engine, FinishReason, Grammar, Request,
    ResidencySnapshot, ResidentModel, RoleClass, Sampling, Scored, SlotState, ThinkMode,
    ToolChoice, Usage,
};
pub use error::{ErrCode, ErrDomain, Error, Result};
pub use event::{
    Allow, ConfirmPrompt, Event, EventBus, EventRx, EventTx, Intent, LOSSY_CAPACITY,
    RELIABLE_CAPACITY, Received,
};
pub use message::{Message, Part, Role, ToolCall};
pub use tool::{Tool, ToolCtx, ToolResult, ToolResultSummary, ToolSchema};
