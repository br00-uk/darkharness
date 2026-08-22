//! The streaming state machine that finds `<tool_call>{...}</tool_call>`
//! blocks inside a text stream.
//!
//! Qwen emits a tool call as plain text when [`dark_contract::Caps::native_tools`]
//! is false. This module finds each block, using a JSON-aware scanner to
//! locate the matching closing brace rather than the closing tag, so a
//! literal `}` inside a string argument never ends the block early and a
//! missing closing tag at the end of a stream never loses a complete call.
//! See task unit `I3`, steps 2 and 3.

/// The tag that opens a tool call block.
const OPEN_TAG: &str = "<tool_call>";
/// The tag that closes a tool call block.
const CLOSE_TAG: &str = "</tool_call>";
/// The longest partial tag a chunk boundary can split off and still
/// complete on the next chunk.
const MAX_PARTIAL_TAG: usize = OPEN_TAG.len() - 1;

/// One tool call recovered from the stream, before JSON repair or schema
/// validation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawCall {
    /// The position of this call among every call the extractor found,
    /// counted from zero. Used to build a stable call identifier.
    pub index: usize,
    /// The text between the opening tag and the matched closing brace.
    pub json_text: String,
    /// The stream reached a balanced closing brace for this call.
    ///
    /// A call that reaches end of stream without one still appears here,
    /// with this set to `false`, so a caller can report why it failed
    /// rather than silently dropping it. See task unit `I3`, step 3.
    pub complete: bool,
}

/// Finds the offset just past the closing brace that balances the opening
/// brace at `start`.
///
/// Tracks whether the scan is inside a JSON string, so a `{` or `}` that
/// appears inside a string value never changes the nesting depth. This is
/// what lets a tool argument contain literal braces. See task unit `I3`,
/// step 3.
fn find_json_object_end(text: &str, start: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    debug_assert_eq!(bytes.get(start), Some(&b'{'));

    let mut depth: i32 = 0;
    let mut in_string = false;
    let mut escaped = false;

    for (offset, &byte) in bytes.iter().enumerate().skip(start) {
        if in_string {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_string = false;
            }
            continue;
        }

        match byte {
            b'"' => in_string = true,
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(offset + 1);
                }
            }
            _ => {}
        }
    }
    None
}

/// Finds the offset just past the closing quote that matches the opening
/// quote at `start`, respecting a backslash escape.
///
/// A double-encoded call wraps its whole JSON body in one more layer of
/// string quoting, so the value right after the tag is a JSON string, not a
/// JSON object. [`find_json_value_end`] dispatches here for that case; the
/// text-level repair that undoes the double encoding runs afterwards, once
/// the value has been extracted whole. See task unit `I3`, step 6, item 2.
fn find_json_string_end(text: &str, start: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    debug_assert_eq!(bytes.get(start), Some(&b'"'));

    let mut escaped = false;
    for (offset, &byte) in bytes.iter().enumerate().skip(start + 1) {
        if escaped {
            escaped = false;
            continue;
        }
        match byte {
            b'\\' => escaped = true,
            b'"' => return Some(offset + 1),
            _ => {}
        }
    }
    None
}

/// Finds the offset just past the end of the JSON value that starts at
/// `start`, whether that value is an object or a double-encoded string.
fn find_json_value_end(text: &str, start: usize) -> Option<usize> {
    match text.as_bytes().get(start) {
        Some(b'{') => find_json_object_end(text, start),
        Some(b'"') => find_json_string_end(text, start),
        _ => None,
    }
}

/// Finds the largest offset at or before `s.len() - retain` that falls on a
/// `char` boundary of `s`.
fn safe_flush_len(s: &str, retain: usize) -> usize {
    if s.len() <= retain {
        return 0;
    }
    let mut idx = s.len() - retain;
    while idx > 0 && !s.is_char_boundary(idx) {
        idx -= 1;
    }
    idx
}

/// A streaming extractor for `<tool_call>{...}</tool_call>` blocks.
///
/// Push fragments as they arrive, then call [`ToolCallExtractor::finish`]
/// once the stream ends. Prose outside any block accumulates separately
/// from the JSON bodies, so text before, between, and after tool calls is
/// never mixed into a call's arguments. See task unit `I3`, steps 2 and 3.
#[derive(Debug, Default)]
pub struct ToolCallExtractor {
    buffer: String,
    prose: String,
    calls: Vec<RawCall>,
}

impl ToolCallExtractor {
    /// Creates an empty extractor.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Feeds the next fragment of the stream into the extractor.
    pub fn push(&mut self, fragment: &str) {
        self.buffer.push_str(fragment);
        self.drain();
    }

    /// Extracts every complete call currently sitting in the buffer.
    fn drain(&mut self) {
        loop {
            let Some(tag_pos) = self.buffer.find(OPEN_TAG) else {
                let flush = safe_flush_len(&self.buffer, MAX_PARTIAL_TAG);
                self.prose.push_str(&self.buffer[..flush]);
                self.buffer.replace_range(..flush, "");
                break;
            };

            self.prose.push_str(&self.buffer[..tag_pos]);
            let after_tag = tag_pos + OPEN_TAG.len();
            let Some(value_offset) = self.buffer[after_tag..].find(|c: char| !c.is_whitespace())
            else {
                // Nothing but whitespace has arrived yet. Wait for more of
                // the stream, but keep everything from the tag onward
                // buffered.
                self.buffer.replace_range(..tag_pos, "");
                break;
            };
            let obj_start = after_tag + value_offset;

            let Some(obj_end) = find_json_value_end(&self.buffer, obj_start) else {
                // The value is not balanced yet, or does not open with `{`
                // or `"` at all. Wait for more data.
                self.buffer.replace_range(..tag_pos, "");
                break;
            };

            let json_text = self.buffer[obj_start..obj_end].to_owned();
            let mut consume_end = obj_end;
            let rest = &self.buffer[obj_end..];
            let trimmed = rest.trim_start();
            if let Some(after_close) = trimmed.strip_prefix(CLOSE_TAG) {
                consume_end += rest.len() - after_close.len();
            }

            let index = self.calls.len();
            self.calls.push(RawCall {
                index,
                json_text,
                complete: true,
            });
            self.buffer.replace_range(..consume_end, "");
        }
    }

    /// Consumes the extractor and returns the leftover prose together with
    /// every call it found.
    ///
    /// A stream that ends with an opening tag and an unbalanced or absent
    /// object still yields a [`RawCall`] for it, marked incomplete, so a
    /// caller can report a clear reason instead of dropping the call
    /// silently. See task unit `I3`, step 3.
    #[must_use]
    pub fn finish(mut self) -> (String, Vec<RawCall>) {
        if let Some(tag_pos) = self.buffer.find(OPEN_TAG) {
            self.prose.push_str(&self.buffer[..tag_pos]);
            let after_tag = tag_pos + OPEN_TAG.len();
            let remainder = self.buffer[after_tag..].to_owned();
            if !remainder.trim().is_empty() {
                let index = self.calls.len();
                self.calls.push(RawCall {
                    index,
                    json_text: remainder,
                    complete: false,
                });
            }
        } else {
            self.prose.push_str(&self.buffer);
        }
        (self.prose, self.calls)
    }
}

/// Extracts every `<tool_call>` block out of a complete text in one call.
///
/// A convenience wrapper over [`ToolCallExtractor`] for a caller that
/// already holds the whole text rather than a stream of fragments.
#[must_use]
pub fn extract(text: &str) -> (String, Vec<RawCall>) {
    let mut extractor = ToolCallExtractor::new();
    extractor.push(text);
    extractor.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_one_call_with_prose_on_both_sides() {
        let (prose, calls) = extract(
            r#"Sure, I'll check that. <tool_call>{"name": "read_file", "arguments": {"path": "a.rs"}}</tool_call> Done."#,
        );
        assert_eq!(calls.len(), 1);
        assert!(calls[0].complete);
        assert_eq!(
            calls[0].json_text,
            r#"{"name": "read_file", "arguments": {"path": "a.rs"}}"#
        );
        assert!(prose.contains("Sure, I'll check that."));
        assert!(prose.contains("Done."));
    }

    #[test]
    fn extracts_many_calls_in_one_message() {
        let (_, calls) = extract(
            r#"<tool_call>{"name": "a", "arguments": {}}</tool_call><tool_call>{"name": "b", "arguments": {}}</tool_call>"#,
        );
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].index, 0);
        assert_eq!(calls[1].index, 1);
        assert!(calls[0].json_text.contains("\"a\""));
        assert!(calls[1].json_text.contains("\"b\""));
    }

    #[test]
    fn a_nested_brace_inside_a_string_value_does_not_end_the_object_early() {
        let (_, calls) = extract(
            r#"<tool_call>{"name": "write_file", "arguments": {"body": "fn f() { return 1; }"}}</tool_call>"#,
        );
        assert_eq!(calls.len(), 1);
        assert!(calls[0].complete);
        assert!(calls[0].json_text.contains("fn f() { return 1; }"));
    }

    #[test]
    fn an_escaped_quote_inside_a_string_does_not_end_the_string_early() {
        let (_, calls) = extract(
            r#"<tool_call>{"name": "grep", "arguments": {"pattern": "say \"hi\" then }"}}</tool_call>"#,
        );
        assert_eq!(calls.len(), 1);
        assert!(calls[0].complete);
    }

    #[test]
    fn an_unclosed_tag_at_end_of_stream_still_yields_an_incomplete_call() {
        let (_, calls) = extract(r#"<tool_call>{"name": "read_file", "arguments": {"path": "#);
        assert_eq!(calls.len(), 1);
        assert!(!calls[0].complete);
    }

    #[test]
    fn a_missing_close_tag_after_a_balanced_object_still_extracts_it() {
        // The object is complete even though the model never emitted
        // </tool_call>.
        let (_, calls) = extract(r#"<tool_call>{"name": "read_file", "arguments": {}}"#);
        assert_eq!(calls.len(), 1);
        assert!(calls[0].complete);
    }

    #[test]
    fn a_tag_split_across_fragments_still_matches() {
        let mut extractor = ToolCallExtractor::new();
        extractor.push("before <tool_c");
        extractor.push(r#"all>{"name": "a", "arguments": {}}</tool_call> after"#);
        let (prose, calls) = extractor.finish();
        assert_eq!(calls.len(), 1);
        assert!(prose.contains("before"));
        assert!(prose.contains("after"));
    }

    #[test]
    fn plain_text_with_no_call_produces_no_calls() {
        let (prose, calls) = extract("just an ordinary reply, nothing to call");
        assert!(calls.is_empty());
        assert_eq!(prose, "just an ordinary reply, nothing to call");
    }
}
