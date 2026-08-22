//! `AGENTS.md` discovery and instruction chain resolution.
//!
//! This crate resolves the darkharness instruction chain: `AGENTS.md`
//! files from `~/.darkharness`, the repository root, and the directories
//! between the root and wherever a turn is working, plus the `CLAUDE.md`
//! / `GEMINI.md` fallback and the `AGENTS.override.md` convention. See
//! task unit K1 and Rule 22 to Rule 25 of the build specification.
//!
//! # Prefix stability
//!
//! The resolved chain is part of the context prefix, and the prefix must
//! not change during a turn (Rule 5, Rule 22): a changed prefix forces the
//! engine to re-run its full prefill, which costs 15 to 30 seconds on a
//! 32B model. Call [`resolve`] exactly once, at the start of a turn, and
//! keep the [`ResolvedChain`] it returns for every round-trip in that
//! turn. [`ResolvedChain::prefix_text`] is a pure function of that value,
//! so it renders identical bytes no matter how many times, or when, a
//! caller calls it.
//!
//! A nested instruction file that a tool call uncovers *during* the turn
//! must never edit that already-built prefix. Pass the touched path to
//! [`discover_for_tail`] instead: it hands back tail content and a notice,
//! and a [`TailTracker`] keeps a round-trip from repeating a notice that
//! an earlier round-trip in the same turn already produced (Rule 23).
//!
//! # Precedence
//!
//! `AGENTS.md` is repository policy. It sits below two narrower sources,
//! which a caller composing the full prefix (see task unit A3) must place
//! so they win a conflict: a wayfinder map note is effort policy for one
//! session, and the person's current message is the narrowest source of
//! all. Neither is part of what this crate resolves.
//!
//! # Home directory
//!
//! [`resolve`] takes the home directory as a parameter. It never reads
//! `$HOME` or an equivalent itself, so a caller — in production, the
//! binary entry point; in a test, a fixture directory — controls where
//! `.darkharness/AGENTS.md` is read from.

pub mod chain;
pub mod config;
pub mod config_block;
pub mod explain;
pub mod resolve;
pub mod working_set;

pub use chain::{
    ChainEntry, ChainRole, ChainSource, FileKind, ResolvedChain, TailAddition, tail_text,
};
pub use config::{AgentsMdConfig, OnOverflow};
pub use resolve::{TailTracker, TokenCounter, discover_for_tail, resolve};
pub use working_set::WorkingSet;
