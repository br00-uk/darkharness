//! Drives another coding agent over the Agent Client Protocol.
//!
//! # What this crate is for
//!
//! darkharness runs a local model, and that stays true. This crate adds
//! a second way to run a session: hand the work to a coding agent the
//! person has already installed — Claude Code, opencode, Gemini CLI,
//! Codex and the rest — while darkharness keeps everything around it.
//!
//! The protocol is JSON-RPC 2.0 over the subprocess's standard input and
//! output. Nothing here opens a socket, so Rule 13 (only `dark-airlock`
//! constructs an HTTP client) is untouched. What the subprocess itself
//! does with the network is its own affair, and this crate records what
//! is known about that rather than pretending to control it. See
//! [`discover`].
//!
//! # Why the protocol fits this harness
//!
//! The Agent Client Protocol turns the usual control flow around. In a
//! local session `dark-core` owns the turn loop and calls out to tools.
//! In an ACP session the *agent* owns its loop, and calls back to the
//! client for permission, for file reads and writes, and to run
//! commands. darkharness already owns every one of those:
//!
//! | What the protocol asks of a client | What answers it here |
//! | --- | --- |
//! | `session/request_permission` | `dark_core::policy::Policy` (task unit `A4`) |
//! | `fs/read_text_file`, `fs/write_text_file` | `dark-tools`, with Rule 34 root containment |
//! | `terminal/*` | `dark-tools`, with dark mode's network namespace |
//! | showing progress | `dark_contract::Event` and `dark-tui` |
//! | recording the session | `dark_core::session::TranscriptWriter` |
//!
//! So a foreign agent runs inside this harness's permission policy and
//! its sandbox, and its session replays like any other. That is the
//! reason to speak the protocol rather than shell out to the agent's own
//! command line.
//!
//! # What this crate does not do
//!
//! It holds no session state and no policy of its own. It reports what
//! it finds and what the agent says; `dark-cli` composes it with the
//! policy, the tools and the event bus, the same as everything else.

pub mod bridge;
pub mod discover;
pub mod session;

pub use bridge::{PermissionAsk, chosen_option, to_prompt};
pub use discover::{Agent, Launch};
pub use session::{Decide, Outcome, Report, check_dark_mode, run_prompt};
