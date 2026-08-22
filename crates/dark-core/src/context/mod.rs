//! Context assembly: the stable prefix and the changing tail.
//!
//! Task unit `A3`. See `PRD.md` section 4.2 and Appendix B.
//!
//! The engine caches the key-value tensors for the context prefix. A stable
//! prefix keeps a round-trip fast; a changed prefix forces a full prefill,
//! which costs 15 to 30 seconds on a 32B model (Rule 5). This module exists
//! to make that stability checkable rather than merely hoped for:
//!
//! - [`prefix::assemble_prefix`] builds the five prefix sections, in the
//!   fixed order the build specification names, from caller-supplied text.
//!   It is pure: the same input always produces byte-identical output.
//! - [`prefix::PrefixTracker`] hashes the assembled prefix once per turn and
//!   names the section that changed when it differs from the previous
//!   turn's, so a silent full prefill never happens without a notice.
//! - [`tail::assemble_tail`] builds the six tail sections. A turn loop calls
//!   it again as tool results arrive; only the tail grows.
//! - [`budget`] turns the Appendix B table into [`budget::BudgetCheck`]
//!   values a caller compares real, tokenizer-measured counts against.
//! - [`tokens::count_tokens`] is the one place this module counts tokens. It
//!   always calls [`dark_contract::Engine::tokenize`]; nothing in this crate
//!   estimates by character count.
//! - [`compact`] selects the oldest third of unpinned history to fold, and
//!   folds it into one summary message once the caller has that message's
//!   text. Do step 5 says compaction uses the scout micro-role: this module
//!   builds the [`dark_contract::Request`] that asks for the summary, but it
//!   never sends one, so `context/` needs no streaming dependency of its
//!   own. See [`compact`]'s module documentation.
//! - [`tail::evict_lexicon_chunks`] drops whole Lexicon chunks before any
//!   history compacts, per Do step 7.
//!
//! This module does not depend on `dark-agentsmd` or `dark-cartograph` (see
//! `CLAUDE.md`'s dependency rules). [`prefix::PrefixInputs`] takes the
//! `AGENTS.md` chain and the map digest as borrowed text that the caller
//! already rendered, so `context/` stays a pure assembler.
//!
//! ```
//! use dark_contract::{EventBus, RoleClass};
//! use dark_core::context::{assemble_prefix, count_message_tokens, PrefixInputs, PrefixTracker};
//! use dark_engine_fake::FakeEngine;
//!
//! # #[tokio::main(flavor = "current_thread")]
//! # async fn main() {
//! let engine = FakeEngine::with_replies(Vec::<String>::new());
//! let bus = EventBus::new();
//! let mut tracker = PrefixTracker::new();
//!
//! let inputs = PrefixInputs {
//!     system_prompt: "you are dark, a local coding harness",
//!     agents_chain: "root AGENTS.md rules",
//!     environment_date: "2026-08-22",
//!     map_digest: None,
//!     ticket_body: None,
//! };
//!
//! let assembled = assemble_prefix(&inputs);
//! tracker.observe(&inputs, &bus.tx()); // first turn: no notice, nothing to compare against
//!
//! let tokens = count_message_tokens(&engine, RoleClass::Worker, &assembled.messages()).unwrap();
//! assert!(tokens > 0);
//! # }
//! ```

pub mod budget;
pub mod compact;
pub mod prefix;
pub mod tail;
pub mod tokens;

pub use budget::{ALL_PARTS, BudgetCheck, Part, TOTAL_AT_32K};
pub use compact::{
    FoldSelection, apply_summary, build_summary_request, compaction_threshold, select_fold_range,
    should_compact,
};
pub use prefix::{
    AssembledPrefix, PREFIX_PARTS, PrefixHash, PrefixInputs, PrefixSection, PrefixTracker,
    assemble_prefix,
};
pub use tail::{
    AssembledTail, LexiconChunk, LexiconEviction, TailInputs, assemble_tail, evict_lexicon_chunks,
};
pub use tokens::{count_message_tokens, count_tokens};
