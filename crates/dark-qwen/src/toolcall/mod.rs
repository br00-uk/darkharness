//! Tool call parsing for Qwen models.
//!
//! Two paths produce a [`ToolCall`]. When [`dark_contract::Caps::native_tools`]
//! is true, the engine already parsed the call and streams it as
//! [`Chunk::ToolCallDelta`] fragments; [`collect_native`] reassembles them.
//! Otherwise the model writes the call as text, in the form
//! `<tool_call>{"name": "...", "arguments": {...}}</tool_call>`, and
//! [`interpret_stream`] runs the streaming extractor, the text repairs, and
//! schema validation over it. See task unit `I3`.
//!
//! A call that fails to parse or fails validation never fails the turn: it
//! becomes a [`Message`] with [`Role::Tool`] that names the field and states
//! the expected type, so a small model can recover. See task unit `I3`,
//! steps 5 and 7.

mod repair;
mod stream;
mod validate;

use dark_contract::{Chunk, ErrCode, Error, Grammar, Message, Result, ToolCall, ToolSchema};

pub use repair::TextRepair;
pub use stream::{RawCall, ToolCallExtractor, extract};
pub use validate::{FieldProblem, ValueRepair, describe_repairs};

/// The outcome of interpreting one raw tool call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Interpreted {
    /// The call parsed and validated. The harness may run it.
    Call {
        /// The parsed, validated call.
        call: ToolCall,
        /// A description of every repair the parser applied, in order.
        repairs: Vec<String>,
    },
    /// The call did not parse or did not validate.
    ///
    /// The harness sends `reply` back as the answer to this call instead of
    /// running anything. An unanswered call breaks the chat template, so a
    /// failure here still produces a reply. See task unit `A2`.
    Failed {
        /// The `Role::Tool` reply that explains the problem.
        reply: Message,
        /// A description of every repair the parser applied before it gave
        /// up.
        repairs: Vec<String>,
    },
}

impl Interpreted {
    /// Returns the parsed call, when interpretation succeeded.
    #[must_use]
    pub fn call(&self) -> Option<&ToolCall> {
        match self {
            Self::Call { call, .. } => Some(call),
            Self::Failed { .. } => None,
        }
    }

    /// Returns the reply this outcome answers with, whether the call
    /// succeeded or failed.
    ///
    /// A successful call has no reply of its own here: the harness attaches
    /// one after it runs the tool. This returns `None` in that case.
    #[must_use]
    pub fn failure_reply(&self) -> Option<&Message> {
        match self {
            Self::Call { .. } => None,
            Self::Failed { reply, .. } => Some(reply),
        }
    }
}

/// Builds the call identifier for the `index`-th call the streaming
/// extractor found in one message.
///
/// Hermes-style text calls carry no engine-issued identifier, unlike a
/// native tool call's [`Chunk::ToolCallDelta::id`]. The index is stable
/// across a single message, so the identifier is stable too.
fn hermes_call_id(index: usize) -> String {
    format!("hermes-{index}")
}

/// Runs the text repairs and schema validation over one [`RawCall`] and
/// returns the outcome.
///
/// # Errors
///
/// This function returns no `Result`; every failure becomes
/// [`Interpreted::Failed`] instead, so a caller can always answer the call.
/// See task unit `I3`, step 5.
#[must_use]
pub fn interpret(raw: &RawCall, schemas: &[ToolSchema]) -> Interpreted {
    let call_id = hermes_call_id(raw.index);

    if !raw.complete {
        tracing::debug!(call = %call_id, "a tool call was still open at end of stream");
        return Interpreted::Failed {
            reply: Message::tool_reply(
                call_id,
                "The tool call ended before its closing brace. Send one complete call.",
            ),
            repairs: Vec::new(),
        };
    }

    let (parsed, text_repairs) = repair::strip_and_parse(&raw.json_text);
    let mut repairs: Vec<String> = text_repairs
        .iter()
        .map(validate::describe_text_repair)
        .collect();
    for description in &repairs {
        tracing::debug!(call = %call_id, repair = %description, "repaired a tool call");
    }

    let value = match parsed {
        Ok(value) => value,
        Err(err) => {
            return Interpreted::Failed {
                reply: Message::tool_reply(
                    call_id,
                    format!("The tool call is not valid JSON: {err}. Send one JSON object."),
                ),
                repairs,
            };
        }
    };

    let Some(name) = value
        .get("name")
        .and_then(|v| v.as_str())
        .map(str::to_owned)
    else {
        return Interpreted::Failed {
            reply: Message::tool_reply(
                call_id,
                "The tool call has no `name` field. Add the tool name.",
            ),
            repairs,
        };
    };

    let Some(schema) = schemas.iter().find(|schema| schema.name == name) else {
        return Interpreted::Failed {
            reply: Message::tool_reply(
                call_id,
                format!("No tool named `{name}` exists. Use one of the tools the harness listed."),
            ),
            repairs,
        };
    };

    let arguments = value
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| serde_json::json!({}));
    let (arguments, value_repairs, problem) = validate::validate_and_repair(arguments, schema);
    for repair in &value_repairs {
        let description = validate::describe_value_repair(repair);
        tracing::debug!(call = %call_id, repair = %description, "repaired a tool call argument");
        repairs.push(description);
    }

    if let Some(problem) = problem {
        return Interpreted::Failed {
            reply: Message::tool_reply(call_id, problem.message()),
            repairs,
        };
    }

    Interpreted::Call {
        call: ToolCall {
            id: call_id,
            name,
            args: arguments,
        },
        repairs,
    }
}

/// Extracts and interprets every Hermes-style `<tool_call>` block in `text`,
/// against `schemas`.
///
/// Use this when [`dark_contract::Caps::native_tools`] is false. Returns
/// the prose outside every block alongside one [`Interpreted`] outcome per
/// call, in the order the calls appeared. See task unit `I3`, step 2.
#[must_use]
pub fn interpret_stream(text: &str, schemas: &[ToolSchema]) -> (String, Vec<Interpreted>) {
    let (prose, raw_calls) = stream::extract(text);
    let interpreted = raw_calls
        .iter()
        .map(|raw| interpret(raw, schemas))
        .collect();
    (prose, interpreted)
}

/// Reassembles [`Chunk::ToolCallDelta`] fragments into calls and validates
/// each one.
///
/// Use this when [`dark_contract::Caps::native_tools`] is true: the engine
/// has already split the call from the model's prose, so this only
/// accumulates the fragments and runs schema validation. See task unit
/// `I3`, step 1.
///
/// # Errors
///
/// Returns [`ErrCode::ToolInvalidArgs`] when a call's accumulated argument
/// text is not valid JSON. A schema mismatch does not error here: it comes
/// back as [`Interpreted::Failed`], the same as the text path, so an
/// unanswered call never breaks the chat template.
pub fn collect_native(chunks: &[Chunk], schemas: &[ToolSchema]) -> Result<Vec<Interpreted>> {
    let mut parts: Vec<(Option<String>, Option<String>, String)> = Vec::new();

    for chunk in chunks {
        if let Chunk::ToolCallDelta {
            index,
            id,
            name,
            args_fragment,
        } = chunk
        {
            if parts.len() <= *index {
                parts.resize(*index + 1, (None, None, String::new()));
            }
            let entry = &mut parts[*index];
            if id.is_some() {
                entry.0.clone_from(id);
            }
            if name.is_some() {
                entry.1.clone_from(name);
            }
            entry.2.push_str(args_fragment);
        }
    }

    parts
        .into_iter()
        .enumerate()
        .map(|(index, (id, name, args_text))| {
            let call_id = id.unwrap_or_else(|| hermes_call_id(index));
            let name = name.ok_or_else(|| {
                Error::new(
                    ErrCode::ToolInvalidArgs,
                    format!("call {call_id} arrived with no tool name"),
                )
            })?;
            let args: serde_json::Value = serde_json::from_str(&args_text).map_err(|err| {
                Error::new(
                    ErrCode::ToolInvalidArgs,
                    format!("call {call_id} sent arguments that are not valid JSON: {err}"),
                )
            })?;

            let Some(schema) = schemas.iter().find(|schema| schema.name == name) else {
                return Ok(Interpreted::Failed {
                    reply: Message::tool_reply(
                        call_id,
                        format!("No tool named `{name}` exists. Use one of the tools the harness listed."),
                    ),
                    repairs: Vec::new(),
                });
            };

            let (args, value_repairs, problem) = validate::validate_and_repair(args, schema);
            let repairs: Vec<String> = value_repairs.iter().map(validate::describe_value_repair).collect();

            if let Some(problem) = problem {
                return Ok(Interpreted::Failed {
                    reply: Message::tool_reply(call_id, problem.message()),
                    repairs,
                });
            }

            Ok(Interpreted::Call {
                call: ToolCall {
                    id: call_id,
                    name,
                    args,
                },
                repairs,
            })
        })
        .collect()
}

/// Builds the grammar that constrains decoding to `schema`'s arguments.
///
/// Grammar-constrained decoding for tool arguments is the default: it turns
/// a retry loop into a guarantee, and local grammar constraint is cheap.
/// See task unit `I3`, step 8.
#[must_use]
pub fn tool_grammar(schema: &ToolSchema) -> Grammar {
    Grammar::JsonSchema(schema.parameters.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read_file_schema() -> ToolSchema {
        ToolSchema {
            name: "read_file".to_owned(),
            description: "Reads a file.".to_owned(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string"},
                    "limit": {"type": "integer", "default": 2000}
                },
                "required": ["path"]
            }),
            tier: 1,
            mutating: false,
        }
    }

    #[test]
    fn a_well_formed_call_interprets_cleanly() {
        let (prose, outcomes) = interpret_stream(
            r#"Sure. <tool_call>{"name": "read_file", "arguments": {"path": "a.rs"}}</tool_call>"#,
            &[read_file_schema()],
        );
        assert!(prose.contains("Sure."));
        assert_eq!(outcomes.len(), 1);
        let call = outcomes[0].call().expect("call interprets");
        assert_eq!(call.name, "read_file");
        assert_eq!(call.args["path"], "a.rs");
        assert_eq!(call.args["limit"], 2000);
    }

    #[test]
    fn many_calls_in_one_message_all_interpret() {
        let (_, outcomes) = interpret_stream(
            r#"<tool_call>{"name": "read_file", "arguments": {"path": "a.rs"}}</tool_call>
               <tool_call>{"name": "read_file", "arguments": {"path": "b.rs"}}</tool_call>"#,
            &[read_file_schema()],
        );
        assert_eq!(outcomes.len(), 2);
        assert!(outcomes.iter().all(|o| o.call().is_some()));
        assert_ne!(
            outcomes[0].call().unwrap().id,
            outcomes[1].call().unwrap().id
        );
    }

    #[test]
    fn a_missing_required_field_never_fails_the_turn() {
        let (_, outcomes) = interpret_stream(
            r#"<tool_call>{"name": "read_file", "arguments": {}}</tool_call>"#,
            &[read_file_schema()],
        );
        assert_eq!(outcomes.len(), 1);
        assert!(outcomes[0].call().is_none());
        let reply = outcomes[0].failure_reply().expect("a failure reply exists");
        assert_eq!(reply.role, dark_contract::Role::Tool);
        let text = reply.text_content();
        assert!(text.contains("path"), "{text}");
        assert!(text.contains("string"), "{text}");
    }

    #[test]
    fn an_unclosed_tag_at_end_of_stream_never_fails_the_turn() {
        let (_, outcomes) = interpret_stream(
            r#"<tool_call>{"name": "read_file", "arguments": {"path": "#,
            &[read_file_schema()],
        );
        assert_eq!(outcomes.len(), 1);
        assert!(outcomes[0].call().is_none());
        assert!(outcomes[0].failure_reply().is_some());
    }

    #[test]
    fn an_unknown_tool_name_never_fails_the_turn() {
        let (_, outcomes) = interpret_stream(
            r#"<tool_call>{"name": "delete_universe", "arguments": {}}</tool_call>"#,
            &[read_file_schema()],
        );
        assert!(outcomes[0].call().is_none());
        let text = outcomes[0].failure_reply().unwrap().text_content();
        assert!(text.contains("delete_universe"));
    }

    #[test]
    fn collect_native_reassembles_split_fragments() {
        let chunks = vec![
            Chunk::ToolCallDelta {
                index: 0,
                id: Some("call-1".to_owned()),
                name: Some("read_file".to_owned()),
                args_fragment: r#"{"path": "#.to_owned(),
            },
            Chunk::ToolCallDelta {
                index: 0,
                id: None,
                name: None,
                args_fragment: r#""a.rs"}"#.to_owned(),
            },
        ];
        let outcomes = collect_native(&chunks, &[read_file_schema()]).expect("collects");
        assert_eq!(outcomes.len(), 1);
        let call = outcomes[0].call().expect("call interprets");
        assert_eq!(call.id, "call-1");
        assert_eq!(call.args["path"], "a.rs");
    }

    #[test]
    fn collect_native_reports_bad_json_as_an_error_not_a_panic() {
        let chunks = vec![Chunk::ToolCallDelta {
            index: 0,
            id: Some("call-1".to_owned()),
            name: Some("read_file".to_owned()),
            args_fragment: "{not json".to_owned(),
        }];
        let err = collect_native(&chunks, &[read_file_schema()]).expect_err("bad JSON errors");
        assert_eq!(err.code, ErrCode::ToolInvalidArgs);
    }

    #[test]
    fn tool_grammar_wraps_the_schema_parameters() {
        let schema = read_file_schema();
        let grammar = tool_grammar(&schema);
        assert_eq!(grammar, Grammar::JsonSchema(schema.parameters.clone()));
    }
}
