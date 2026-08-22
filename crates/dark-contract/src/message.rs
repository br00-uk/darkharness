//! The message types that make up a conversation.

use std::path::PathBuf;

use bytes::Bytes;
use serde::{Deserialize, Serialize};

/// Who produced a message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    /// The harness itself. This role carries the prefix.
    System,
    /// The person.
    User,
    /// The model.
    Assistant,
    /// The result of a tool call.
    Tool,
}

/// One piece of message content.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Part {
    /// Plain text.
    Text(String),
    /// An image that the model can see.
    Image {
        /// The raw image bytes.
        data: Bytes,
        /// The MIME type, for example `image/png`.
        mime: String,
    },
    /// A file reference. The harness resolves the path when it builds context.
    File {
        /// The path to the file.
        path: PathBuf,
        /// The MIME type.
        mime: String,
    },
}

/// One call that the model asked the harness to make.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCall {
    /// The identifier that ties this call to its reply.
    pub id: String,
    /// The tool name.
    pub name: String,
    /// The arguments, as the model produced them.
    pub args: serde_json::Value,
}

/// One message in a conversation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Message {
    /// Who produced this message.
    pub role: Role,
    /// The content.
    pub parts: Vec<Part>,
    /// The calls that this message requests.
    pub tool_calls: Vec<ToolCall>,
    /// The call that this message answers, when the role is [`Role::Tool`].
    pub tool_call_id: Option<String>,
    /// Thinking text.
    ///
    /// `dark-qwen` lifts this out of `<think>` blocks. The harness never sends
    /// this field to a model. See Rule 5 and task unit `I2`.
    pub reasoning: Option<String>,
    /// A pinned message goes in the prefix. See Rule 5.
    pub pinned: bool,
}

impl Message {
    /// Creates a message that holds one text part.
    pub fn text(role: Role, text: impl Into<String>) -> Self {
        Self {
            role,
            parts: vec![Part::Text(text.into())],
            tool_calls: Vec::new(),
            tool_call_id: None,
            reasoning: None,
            pinned: false,
        }
    }

    /// Creates a reply to a tool call.
    pub fn tool_reply(call_id: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            tool_call_id: Some(call_id.into()),
            ..Self::text(Role::Tool, text)
        }
    }

    /// Marks this message as part of the prefix.
    #[must_use]
    pub fn pinned(mut self) -> Self {
        self.pinned = true;
        self
    }

    /// Returns the concatenated text of every [`Part::Text`] in this message.
    pub fn text_content(&self) -> String {
        self.parts
            .iter()
            .filter_map(|part| match part {
                Part::Text(text) => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_builds_a_single_part_message() {
        let msg = Message::text(Role::User, "hello");
        assert_eq!(msg.role, Role::User);
        assert_eq!(msg.text_content(), "hello");
        assert!(!msg.pinned);
    }

    #[test]
    fn tool_reply_carries_the_call_id() {
        let msg = Message::tool_reply("call-1", "ok");
        assert_eq!(msg.role, Role::Tool);
        assert_eq!(msg.tool_call_id.as_deref(), Some("call-1"));
    }

    #[test]
    fn pinned_marks_the_message_for_the_prefix() {
        assert!(Message::text(Role::System, "rules").pinned().pinned);
    }

    #[test]
    fn text_content_skips_non_text_parts() {
        let msg = Message {
            parts: vec![
                Part::Text("a".into()),
                Part::File {
                    path: "x.rs".into(),
                    mime: "text/plain".into(),
                },
                Part::Text("b".into()),
            ],
            ..Message::text(Role::User, "")
        };
        assert_eq!(msg.text_content(), "ab");
    }

    #[test]
    fn a_message_survives_a_json_round_trip() {
        // The transcript stores one JSON object for each event. See task unit A1.
        let msg = Message::text(Role::Assistant, "hi").pinned();
        let json = serde_json::to_string(&msg).unwrap();
        assert_eq!(serde_json::from_str::<Message>(&json).unwrap(), msg);
    }
}
