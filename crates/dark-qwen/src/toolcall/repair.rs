//! Text-level repairs applied to a tool call before it is parsed as JSON.
//!
//! These repairs undo two things a small model does under pressure: wrap
//! its JSON in a Markdown code fence, or double-encode it as a JSON string.
//! Apply them in the order the build specification gives, and log each one
//! that fires. See task unit `I3`, step 6.

/// One repair that [`strip_and_parse`] applied, in the order it applied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TextRepair {
    /// Removed a Markdown code fence around the JSON body.
    StrippedCodeFence,
    /// Unwrapped a JSON body that was itself encoded as a JSON string.
    UnescapedDoubleEncoding,
}

/// Removes a Markdown code fence around `text`, when one wraps it.
///
/// Handles a fence with or without a language tag, for example
/// ```` ```json ```` or plain ```` ``` ````.
fn strip_code_fence(text: &str) -> Option<String> {
    let trimmed = text.trim();
    let after_open = trimmed.strip_prefix("```")?;
    let after_open = after_open.trim_start_matches(|c: char| c.is_alphanumeric());
    let after_open = after_open.strip_prefix('\n').unwrap_or(after_open);
    let body = after_open.strip_suffix("```").unwrap_or(after_open);
    Some(body.trim().to_owned())
}

/// Parses `text` as JSON, unwrapping one layer of double-encoding first if
/// the direct parse fails or yields a bare JSON string.
///
/// A double-encoded call arrives as a JSON string whose content is itself
/// the call's JSON text, for example `"{\"name\": \"read_file\"}"` instead
/// of `{"name": "read_file"}`. Unwrap it once and parse the result.
fn parse_with_unescape(text: &str) -> (Result<serde_json::Value, serde_json::Error>, bool) {
    match serde_json::from_str::<serde_json::Value>(text) {
        Ok(serde_json::Value::String(inner)) => match serde_json::from_str(&inner) {
            Ok(value) => (Ok(value), true),
            Err(_) => (Ok(serde_json::Value::String(inner)), false),
        },
        Ok(value) => (Ok(value), false),
        Err(direct_err) => {
            // A model sometimes escapes an inner quote without wrapping the
            // whole body in an outer string. Undo that specific pattern and
            // retry once before giving up.
            let unescaped = text.replace("\\\"", "\"");
            if unescaped != text
                && let Ok(value) = serde_json::from_str(&unescaped)
            {
                return (Ok(value), true);
            }
            (Err(direct_err), false)
        }
    }
}

/// Applies the fence and double-encoding repairs, in order, then parses the
/// result as JSON.
///
/// # Errors
///
/// Returns the `serde_json` parse error when the text is not valid JSON
/// after both repairs.
pub(crate) fn strip_and_parse(
    raw: &str,
) -> (
    Result<serde_json::Value, serde_json::Error>,
    Vec<TextRepair>,
) {
    let mut repairs = Vec::new();

    let unfenced = match strip_code_fence(raw) {
        Some(body) => {
            repairs.push(TextRepair::StrippedCodeFence);
            body
        }
        None => raw.to_owned(),
    };

    let (parsed, unescaped) = parse_with_unescape(&unfenced);
    if unescaped {
        repairs.push(TextRepair::UnescapedDoubleEncoding);
    }

    (parsed, repairs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plain_object_needs_no_repair() {
        let (parsed, repairs) = strip_and_parse(r#"{"name": "a", "arguments": {}}"#);
        assert!(parsed.is_ok());
        assert!(repairs.is_empty());
    }

    #[test]
    fn a_json_fence_is_stripped() {
        let (parsed, repairs) =
            strip_and_parse("```json\n{\"name\": \"a\", \"arguments\": {}}\n```");
        assert!(parsed.is_ok());
        assert_eq!(repairs, vec![TextRepair::StrippedCodeFence]);
    }

    #[test]
    fn a_bare_fence_with_no_language_tag_is_stripped() {
        let (parsed, repairs) = strip_and_parse("```\n{\"name\": \"a\", \"arguments\": {}}\n```");
        assert!(parsed.is_ok());
        assert_eq!(repairs, vec![TextRepair::StrippedCodeFence]);
    }

    #[test]
    fn a_double_encoded_body_is_unescaped() {
        let raw = r#""{\"name\": \"a\", \"arguments\": {}}""#;
        let (parsed, repairs) = strip_and_parse(raw);
        let value = parsed.expect("unwraps to valid JSON");
        assert_eq!(value["name"], "a");
        assert_eq!(repairs, vec![TextRepair::UnescapedDoubleEncoding]);
    }

    #[test]
    fn both_repairs_can_apply_in_sequence() {
        let raw = "```json\n\"{\\\"name\\\": \\\"a\\\", \\\"arguments\\\": {}}\"\n```";
        let (parsed, repairs) = strip_and_parse(raw);
        assert!(parsed.is_ok());
        assert_eq!(
            repairs,
            vec![
                TextRepair::StrippedCodeFence,
                TextRepair::UnescapedDoubleEncoding
            ]
        );
    }

    #[test]
    fn genuinely_malformed_json_reports_an_error_not_a_panic() {
        let (parsed, _repairs) = strip_and_parse("{not json at all");
        assert!(parsed.is_err());
    }
}
