//! Qwen prompts, profiles, thinking control, and tool-call parsing.
//!
//! `dark-qwen` configures the harness for the Qwen model family, the only
//! family the harness supports. It has no engine of its own: it reads
//! [`dark_contract::Caps`] from whatever `dyn Engine` the caller holds, and
//! it never depends on `mistralrs` directly. See Rule 12 and Rule 17.
//!
//! | Module | Task unit | What it owns |
//! | --- | --- | --- |
//! | [`profile`] | `I1` | The model profile table: role, tool tier, thinking default, charting rights. |
//! | [`think`] | `I2` | Thinking control detection, the `ThinkMode::Auto` policy, and stripping `<think>` blocks. |
//! | [`toolcall`] | `I3` | Parsing a tool call from native chunks or from Hermes-style text, with repair and validation. |
//! | [`sampling`] | `I4` | Sampling defaults and the versioned system prompt fragments. |
//!
//! Two rules apply across every module here, because they come from PRD
//! Section 4.2 rather than from any one task unit:
//!
//! - A profile never stores a context length. Read
//!   [`dark_contract::Caps::granted_context`] fresh every turn; see
//!   [`profile::ProfileTable::resolve`].
//! - [`Message::reasoning`] never travels back to a model. Call
//!   [`think::prepare_outbound`] on the message history before building a
//!   [`dark_contract::Request`].
//!
//! [`Message::reasoning`]: dark_contract::Message::reasoning

pub mod profile;
pub mod sampling;
pub mod think;
pub mod toolcall;

pub use profile::{MicroRoleConfig, MicroRoles, Profile, ProfileTable, ResolvedProfile};
pub use sampling::{
    BASE_PROMPT, COMPACT_PROMPT, FULL_PROMPT, PromptFragment, YarnExtension, for_model,
    guard_against_greedy_thinking, is_greedy, not_thinking_defaults, system_prompt_for,
    thinking_defaults,
};
pub use think::{
    ThinkControl, ThinkResult, ThinkStripper, TurnPurpose, apply_marker, auto_policy, detect,
    detect_and_record, prepare_outbound, should_think, strip_think_blocks,
};
pub use toolcall::{
    FieldProblem, Interpreted, RawCall, TextRepair, ToolCallExtractor, ValueRepair, collect_native,
    describe_repairs, extract, interpret, interpret_stream, tool_grammar,
};
