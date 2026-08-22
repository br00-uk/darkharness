//! Thinking control for Qwen models.
//!
//! Qwen exposes three ways to turn thinking on or off. This module detects
//! which one the loaded chat template honours, applies the
//! [`ThinkMode::Auto`] policy table for a turn, and strips `<think>` blocks
//! out of a stream into [`Message::reasoning`]. See task unit `I2`.
//!
//! Thinking text never travels back to a model: [`prepare_outbound`] drops
//! [`Message::reasoning`] from every message before a caller builds a
//! [`dark_contract::Request`]. Sending it back would grow the cached prefix
//! and would replay the model's own thinking as if a person had written it.
//! See PRD Section 4.2, Rule 5, and task unit `I2`, step 5.

use dark_contract::{Caps, Message};

/// A tag that opens a thinking block.
const OPEN_TAG: &str = "<think>";
/// A tag that closes a thinking block.
const CLOSE_TAG: &str = "</think>";
/// The longest partial tag that a chunk boundary can split off.
///
/// Both tags are ASCII, so this is a byte count as well as a character
/// count. `</think>` is the longer of the two, at 8 bytes; a boundary can
/// therefore split off at most 7 bytes of it and still complete on the next
/// chunk.
const MAX_PARTIAL_TAG: usize = CLOSE_TAG.len() - 1;

/// How the loaded chat template turns thinking on or off.
///
/// Detect this once per loaded model with [`detect`]. See task unit `I2`,
/// step 1.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThinkControl {
    /// The chat template reads a boolean template flag, for example
    /// `enable_thinking`, when it renders the prompt.
    TemplateFlag,
    /// The chat template reads a `/think` or `/no_think` marker in the last
    /// user turn.
    Marker,
    /// Neither construct is present in the template text. The harness falls
    /// back to the engine's own generation parameter.
    EngineParameter,
}

/// Inspects chat template text and returns the control method it honours.
///
/// The check looks for the two template-visible constructs in order: a
/// template flag reference first, then a literal marker. Absent both, the
/// harness falls back to [`ThinkControl::EngineParameter`], which every
/// engine implementation accepts regardless of the template.
#[must_use]
pub fn detect(chat_template: &str) -> ThinkControl {
    if chat_template.contains("enable_thinking") {
        ThinkControl::TemplateFlag
    } else if chat_template.contains("/think") || chat_template.contains("/no_think") {
        ThinkControl::Marker
    } else {
        ThinkControl::EngineParameter
    }
}

/// Detects the control method for `chat_template` and records support for
/// thinking in `caps.thinking`.
///
/// [`detect`] always returns a usable method, so this always sets
/// `caps.thinking` to `true`. Call it once, after the engine loads the
/// model and before the first turn. See task unit `I2`, step 2.
pub fn detect_and_record(chat_template: &str, caps: &mut Caps) -> ThinkControl {
    let control = detect(chat_template);
    caps.thinking = true;
    control
}

/// Injects a `/think` or `/no_think` marker into `text`, for
/// [`ThinkControl::Marker`] templates.
///
/// Appends the marker on its own line so it never merges into the last word
/// of the turn.
#[must_use]
pub fn apply_marker(text: &str, thinking: bool) -> String {
    let marker = if thinking { "/think" } else { "/no_think" };
    if text.is_empty() {
        marker.to_owned()
    } else {
        format!("{text}\n{marker}")
    }
}

/// The purpose of a turn, for the [`ThinkMode::Auto`] policy table.
///
/// See task unit `I2`, step 3.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnPurpose {
    /// The turn charts or edits a wayfinder map.
    Charting,
    /// The turn narrates a seam report.
    SeamNarration,
    /// The turn diagnoses a failure.
    Debugging,
    /// The turn compresses a map into a digest.
    DigestCompression,
    /// The turn chooses one label from a fixed set.
    Classification,
    /// The turn only emits a tool call and nothing else.
    ToolCallOnly,
}

/// Applies the [`ThinkMode::Auto`] policy table for `purpose`.
///
/// | Turn purpose | Thinking |
/// | --- | --- |
/// | Charting conversation | On |
/// | Seam narration | On |
/// | Debugging | On |
/// | Digest compression | Off |
/// | Classification | Off |
/// | A turn that only emits a tool call | Off |
///
/// Thinking on a tool-selection turn costs hundreds of tokens and reaches
/// the same call. Locally those tokens cost seconds, not money, but they
/// still cost the person waiting. See task unit `I2`, step 3.
#[must_use]
pub fn auto_policy(purpose: TurnPurpose) -> bool {
    match purpose {
        TurnPurpose::Charting | TurnPurpose::SeamNarration | TurnPurpose::Debugging => true,
        TurnPurpose::DigestCompression
        | TurnPurpose::Classification
        | TurnPurpose::ToolCallOnly => false,
    }
}

/// Resolves whether a turn should think.
///
/// [`dark_contract::ThinkMode::Auto`] defers to [`auto_policy`]. Every other
/// mode is explicit and passes straight through.
#[must_use]
pub fn should_think(requested: dark_contract::ThinkMode, purpose: TurnPurpose) -> bool {
    match requested {
        dark_contract::ThinkMode::Auto => auto_policy(purpose),
        dark_contract::ThinkMode::On => true,
        dark_contract::ThinkMode::Off => false,
    }
}

/// The result of stripping `<think>` blocks out of a text.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ThinkResult {
    /// The text with every complete thinking block removed.
    pub visible: String,
    /// The concatenated content of every thinking block, when any appeared.
    pub reasoning: Option<String>,
    /// The stream ended while a thinking block was still open.
    ///
    /// [`ThinkResult::reasoning`] still carries whatever thinking text
    /// arrived before the stream ended. See task unit `I2`, step 4.
    pub unclosed_block: bool,
}

/// Finds the largest byte offset at or before `buffer.len() - retain` that
/// falls on a `char` boundary.
///
/// Flushing up to this offset is always safe: it never splits a multi-byte
/// character, and it always leaves at least `retain` bytes buffered, which
/// is enough for a tag split across a chunk boundary to complete on the next
/// chunk.
fn safe_flush_len(buffer: &str, retain: usize) -> usize {
    if buffer.len() <= retain {
        return 0;
    }
    let mut idx = buffer.len() - retain;
    while idx > 0 && !buffer.is_char_boundary(idx) {
        idx -= 1;
    }
    idx
}

/// A streaming state machine that strips `<think>` blocks out of a text
/// stream as it arrives.
///
/// Push each fragment as it arrives, then call [`ThinkStripper::finish`]
/// once the stream ends. The stripper buffers up to 7 bytes so a tag split
/// across two fragments still matches. See task unit `I2`, step 4.
#[derive(Debug, Default)]
pub struct ThinkStripper {
    in_think: bool,
    visible: String,
    reasoning: String,
    buffer: String,
}

impl ThinkStripper {
    /// Creates an empty stripper, outside any thinking block.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Feeds the next fragment of the stream into the stripper.
    pub fn push(&mut self, fragment: &str) {
        self.buffer.push_str(fragment);
        loop {
            if self.in_think {
                if let Some(pos) = self.buffer.find(CLOSE_TAG) {
                    self.reasoning.push_str(&self.buffer[..pos]);
                    self.buffer.replace_range(..pos + CLOSE_TAG.len(), "");
                    self.in_think = false;
                    continue;
                }
                let flush = safe_flush_len(&self.buffer, MAX_PARTIAL_TAG);
                self.reasoning.push_str(&self.buffer[..flush]);
                self.buffer.replace_range(..flush, "");
                break;
            }

            if let Some(pos) = self.buffer.find(OPEN_TAG) {
                self.visible.push_str(&self.buffer[..pos]);
                self.buffer.replace_range(..pos + OPEN_TAG.len(), "");
                self.in_think = true;
                continue;
            }
            let flush = safe_flush_len(&self.buffer, MAX_PARTIAL_TAG);
            self.visible.push_str(&self.buffer[..flush]);
            self.buffer.replace_range(..flush, "");
            break;
        }
    }

    /// Consumes the stripper and returns the accumulated result.
    ///
    /// When the stream ended inside an open thinking block, the leftover
    /// buffer joins [`ThinkResult::reasoning`] and
    /// [`ThinkResult::unclosed_block`] is set. See task unit `I2`, step 4.
    #[must_use]
    pub fn finish(mut self) -> ThinkResult {
        let unclosed_block = self.in_think && !self.buffer.is_empty();
        if self.in_think {
            self.reasoning.push_str(&self.buffer);
        } else {
            self.visible.push_str(&self.buffer);
        }
        ThinkResult {
            visible: self.visible,
            reasoning: (!self.reasoning.is_empty()).then_some(self.reasoning),
            unclosed_block,
        }
    }
}

/// Strips `<think>` blocks out of a complete text in one call.
///
/// This is a convenience wrapper over [`ThinkStripper`] for callers that
/// already hold the whole text, rather than a stream of fragments.
#[must_use]
pub fn strip_think_blocks(text: &str) -> ThinkResult {
    let mut stripper = ThinkStripper::new();
    stripper.push(text);
    stripper.finish()
}

/// Drops [`Message::reasoning`] from every message.
///
/// Call this on the message history immediately before building a
/// [`dark_contract::Request`]. Thinking text is not part of the message
/// history and a model never sees its own past thinking again. See PRD
/// Rule 5 and task unit `I2`, step 5.
#[must_use]
pub fn prepare_outbound(mut messages: Vec<Message>) -> Vec<Message> {
    for message in &mut messages {
        message.reasoning = None;
    }
    messages
}

#[cfg(test)]
mod tests {
    use super::*;
    use dark_contract::ThinkMode;

    #[test]
    fn detects_a_template_flag() {
        let template = "{% if enable_thinking %}...{% endif %}";
        assert_eq!(detect(template), ThinkControl::TemplateFlag);
    }

    #[test]
    fn detects_a_marker() {
        let template = "Reply with /think or /no_think as the user asks.";
        assert_eq!(detect(template), ThinkControl::Marker);
    }

    #[test]
    fn falls_back_to_the_engine_parameter() {
        let template = "{{ messages | tojson }}";
        assert_eq!(detect(template), ThinkControl::EngineParameter);
    }

    #[test]
    fn detect_and_record_sets_caps_thinking() {
        let mut caps = fake_caps();
        caps.thinking = false;
        let control = detect_and_record("enable_thinking", &mut caps);
        assert_eq!(control, ThinkControl::TemplateFlag);
        assert!(caps.thinking);
    }

    #[test]
    fn apply_marker_appends_on_its_own_line() {
        assert_eq!(apply_marker("fix the bug", true), "fix the bug\n/think");
        assert_eq!(apply_marker("fix the bug", false), "fix the bug\n/no_think");
        assert_eq!(apply_marker("", true), "/think");
    }

    #[test]
    fn auto_policy_matches_the_build_specification_table() {
        assert!(auto_policy(TurnPurpose::Charting));
        assert!(auto_policy(TurnPurpose::SeamNarration));
        assert!(auto_policy(TurnPurpose::Debugging));
        assert!(!auto_policy(TurnPurpose::DigestCompression));
        assert!(!auto_policy(TurnPurpose::Classification));
        assert!(!auto_policy(TurnPurpose::ToolCallOnly));
    }

    #[test]
    fn should_think_defers_to_auto_policy_only_on_auto() {
        assert!(should_think(ThinkMode::On, TurnPurpose::ToolCallOnly));
        assert!(!should_think(ThinkMode::Off, TurnPurpose::Charting));
        assert!(should_think(ThinkMode::Auto, TurnPurpose::Charting));
        assert!(!should_think(ThinkMode::Auto, TurnPurpose::ToolCallOnly));
    }

    #[test]
    fn strips_one_block_in_one_call() {
        let result = strip_think_blocks("before <think>hmm, let me see</think> after");
        assert_eq!(result.visible, "before  after");
        assert_eq!(result.reasoning.as_deref(), Some("hmm, let me see"));
        assert!(!result.unclosed_block);
    }

    #[test]
    fn strips_many_blocks_and_keeps_visible_text_in_order() {
        let result = strip_think_blocks("a<think>one</think>b<think>two</think>c");
        assert_eq!(result.visible, "abc");
        assert_eq!(result.reasoning.as_deref(), Some("onetwo"));
    }

    #[test]
    fn text_with_no_block_is_untouched() {
        let result = strip_think_blocks("plain text, nothing to strip");
        assert_eq!(result.visible, "plain text, nothing to strip");
        assert_eq!(result.reasoning, None);
        assert!(!result.unclosed_block);
    }

    #[test]
    fn a_stream_cut_inside_a_block_still_returns_its_reasoning() {
        let mut stripper = ThinkStripper::new();
        stripper.push("before <think>partial thou");
        let result = stripper.finish();
        assert_eq!(result.visible, "before ");
        assert_eq!(result.reasoning.as_deref(), Some("partial thou"));
        assert!(result.unclosed_block);
    }

    #[test]
    fn an_open_tag_split_across_two_fragments_still_matches() {
        let mut stripper = ThinkStripper::new();
        stripper.push("before <thi");
        stripper.push("nk>hidden</think> after");
        let result = stripper.finish();
        assert_eq!(result.visible, "before  after");
        assert_eq!(result.reasoning.as_deref(), Some("hidden"));
        assert!(!result.unclosed_block);
    }

    #[test]
    fn a_close_tag_split_across_many_single_byte_fragments_still_matches() {
        let mut stripper = ThinkStripper::new();
        for fragment in ["<think>", "abc", "</", "th", "ink", ">", "tail"] {
            stripper.push(fragment);
        }
        let result = stripper.finish();
        assert_eq!(result.visible, "tail");
        assert_eq!(result.reasoning.as_deref(), Some("abc"));
        assert!(!result.unclosed_block);
    }

    #[test]
    fn a_multibyte_character_at_a_flush_boundary_is_never_split() {
        // Each push is one character. The flush logic must never panic on a
        // char boundary, even for multi-byte characters near the tag-sized
        // retain window.
        let mut stripper = ThinkStripper::new();
        for ch in "visible caf\u{e9} text \u{4e2d}\u{6587} words".chars() {
            let mut buf = [0u8; 4];
            stripper.push(ch.encode_utf8(&mut buf));
        }
        let result = stripper.finish();
        assert_eq!(
            result.visible,
            "visible caf\u{e9} text \u{4e2d}\u{6587} words"
        );
        assert_eq!(result.reasoning, None);
    }

    #[test]
    fn prepare_outbound_drops_reasoning_from_every_message() {
        let messages = vec![
            Message {
                reasoning: Some("secret thoughts".to_owned()),
                ..Message::text(dark_contract::Role::Assistant, "the answer")
            },
            Message::text(dark_contract::Role::User, "a question"),
        ];
        let prepared = prepare_outbound(messages);
        assert!(prepared.iter().all(|m| m.reasoning.is_none()));
        assert_eq!(prepared[0].text_content(), "the answer");
    }

    fn fake_caps() -> Caps {
        Caps {
            model_id: "fake/qwen3-4b".to_owned(),
            max_context: 32_768,
            granted_context: 32_768,
            native_tools: false,
            thinking: false,
            grammar: true,
            vision: false,
            logprobs: false,
            params_b: 4.0,
            quant: "q4k".to_owned(),
            device: dark_contract::Device::Cpu,
            measured_tok_s: None,
        }
    }
}
