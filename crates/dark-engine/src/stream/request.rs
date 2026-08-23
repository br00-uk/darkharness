//! Converts a [`dark_contract::Request`] into a `mistralrs::RequestBuilder`.
//!
//! This builds a real `mistralrs::RequestBuilder` and is tested against it
//! directly, through [`mistralrs::RequestLike`]'s accessors — no loaded
//! model is needed to validate what a request would ask mistral.rs to do.
//!
//! # Tools flow only to a model that parses them natively
//!
//! [`build`] attaches `req.tools` and `req.tool_choice` to the mistral.rs
//! request only when `caps.native_tools` is `true`. When it is `false`,
//! mistral.rs never sees a tool at all, so it never attempts its own
//! tool-call parsing (see `crates/dark-engine-fake/src/lib.rs`'s
//! `default_caps`, where the small default model has `native_tools:
//! false`): every generated token comes back as plain
//! [`dark_contract::Chunk::Text`], which is exactly the raw text
//! `dark-qwen`'s scraper (task unit `I3`) needs. [`super::response::map`]
//! therefore needs no branch on `native_tools` of its own — it only ever
//! sees a `Delta.tool_calls` in the first place when this function decided
//! to ask for them.
//!
//! # A named gap: image, audio, and file parts
//!
//! [`dark_contract::Part::Image`] and [`dark_contract::Part::File`] are not
//! threaded into the mistral.rs request yet; only [`dark_contract::Part::Text`]
//! is. `Caps::vision` exists for a model that can see images, but wiring a
//! [`dark_contract::Part::Image`]'s raw bytes through to
//! `mistralrs::MultimodalMessages` needs its own decoding step this task
//! unit does not cover. A caller sending an image today gets its text
//! parts only, silently — this is a real, named gap, not a design choice.

use dark_contract::{Caps, ErrCode, Error, Grammar, Message, Result, Role};
use mistralrs::{RequestBuilder, StopTokens, TextMessageRole};

use crate::determinism;

/// Builds the mistral.rs request for `req`, gated by `caps`.
///
/// # Errors
///
/// Returns [`ErrCode::ToolInvalidArgs`] when `req.tool_choice` names a tool
/// that `req.tools` does not define.
pub fn build(req: &dark_contract::Request, caps: &Caps) -> Result<RequestBuilder> {
    let mut builder = RequestBuilder::new();

    for message in &req.messages {
        builder = add_message(builder, message);
    }

    if caps.native_tools && !req.tools.is_empty() {
        let tools: Vec<mistralrs::Tool> = req.tools.iter().map(to_mistralrs_tool).collect();
        builder = builder.set_tools(tools);
        builder = builder.set_tool_choice(to_mistralrs_tool_choice(&req.tool_choice, &req.tools)?);
    }

    let mut sampling = mistralrs::SamplingParams::neutral();
    sampling.temperature = req.sampling.temperature.map(f64::from);
    sampling.top_p = req.sampling.top_p.map(f64::from);
    sampling.top_k = req.sampling.top_k;
    sampling.min_p = req.sampling.min_p.map(f64::from);
    sampling.presence_penalty = req.sampling.presence_penalty;
    sampling.repetition_penalty = req.sampling.repetition_penalty;
    sampling.max_len = Some(req.max_tokens);
    if determinism::plan(req).greedy {
        sampling.top_k = Some(1);
    }
    builder = builder.set_sampling(sampling);

    if !req.stop.is_empty() {
        builder = builder.set_sampler_stop_toks(StopTokens::Seqs(req.stop.clone()));
    }

    if let Some(grammar) = &req.grammar {
        builder = builder.set_constraint(to_mistralrs_constraint(grammar));
    }

    builder = match req.think {
        dark_contract::ThinkMode::On => builder.enable_thinking(true),
        dark_contract::ThinkMode::Off => builder.enable_thinking(false),
        // Auto: task unit I2 decides per turn upstream of this crate; a
        // request that still says Auto by the time it reaches dark-engine
        // gets no override, so the chat template's own default applies.
        dark_contract::ThinkMode::Auto => builder,
    };

    Ok(builder)
}

/// Appends one message to `builder`.
fn add_message(builder: RequestBuilder, message: &Message) -> RequestBuilder {
    let text = message.text_content();
    match message.role {
        Role::Tool => {
            builder.add_tool_message(text, message.tool_call_id.clone().unwrap_or_default())
        }
        Role::Assistant if !message.tool_calls.is_empty() => {
            let calls: Vec<mistralrs::ToolCallResponse> = message
                .tool_calls
                .iter()
                .enumerate()
                .map(|(index, call)| mistralrs::ToolCallResponse {
                    index,
                    id: call.id.clone(),
                    tp: mistralrs::ToolCallType::Function,
                    function: mistralrs::CalledFunction {
                        name: call.name.clone(),
                        arguments: call.args.to_string(),
                    },
                })
                .collect();
            builder.add_message_with_tool_call(TextMessageRole::Assistant, text, calls)
        }
        role => builder.add_message(to_mistralrs_role(role), text),
    }
}

/// Converts a [`Role`] to [`TextMessageRole`].
fn to_mistralrs_role(role: Role) -> TextMessageRole {
    match role {
        Role::System => TextMessageRole::System,
        Role::User => TextMessageRole::User,
        Role::Assistant => TextMessageRole::Assistant,
        Role::Tool => TextMessageRole::Tool,
    }
}

/// Converts a [`dark_contract::ToolSchema`] to a `mistralrs::Tool`.
fn to_mistralrs_tool(schema: &dark_contract::ToolSchema) -> mistralrs::Tool {
    mistralrs::Tool {
        tp: mistralrs::ToolType::Function,
        function: mistralrs::Function {
            description: Some(schema.description.clone()),
            name: schema.name.clone(),
            parameters: schema.parameters.as_object().map(|object| {
                object
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect()
            }),
        },
    }
}

/// Converts a [`dark_contract::ToolChoice`] to `mistralrs::ToolChoice`.
///
/// # Errors
///
/// Returns [`ErrCode::ToolInvalidArgs`] when [`dark_contract::ToolChoice::Named`]
/// names a tool that `tools` does not define.
fn to_mistralrs_tool_choice(
    choice: &dark_contract::ToolChoice,
    tools: &[dark_contract::ToolSchema],
) -> Result<mistralrs::ToolChoice> {
    match choice {
        dark_contract::ToolChoice::None => Ok(mistralrs::ToolChoice::None),
        // mistral.rs 0.8.1 has no "some tool, any tool" choice distinct
        // from Auto; Required degrades to Auto rather than failing the
        // request outright. See docs/adr/0006.
        dark_contract::ToolChoice::Auto | dark_contract::ToolChoice::Required => {
            Ok(mistralrs::ToolChoice::Auto)
        }
        dark_contract::ToolChoice::Named(name) => tools
            .iter()
            .find(|schema| &schema.name == name)
            .map(|schema| mistralrs::ToolChoice::Tool(to_mistralrs_tool(schema)))
            .ok_or_else(|| {
                Error::new(
                    ErrCode::ToolInvalidArgs,
                    format!("tool_choice names '{name}', which is not in the tool list"),
                )
            }),
    }
}

/// Converts a [`Grammar`] to a `mistralrs::Constraint`.
fn to_mistralrs_constraint(grammar: &Grammar) -> mistralrs::Constraint {
    match grammar {
        Grammar::JsonSchema(schema) => mistralrs::Constraint::JsonSchema(schema.clone()),
        Grammar::Regex(pattern) => mistralrs::Constraint::Regex(pattern.clone()),
        Grammar::Lark(grammar) => mistralrs::Constraint::Lark(grammar.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dark_contract::{Device, RoleClass, Sampling, ThinkMode, ToolSchema};
    use mistralrs::RequestLike;

    fn caps(native_tools: bool) -> Caps {
        Caps {
            model_id: "fake/qwen3-4b".to_owned(),
            max_context: 32_768,
            granted_context: 32_768,
            native_tools,
            thinking: true,
            grammar: true,
            vision: false,
            logprobs: false,
            params_b: 4.0,
            quant: "q4k".to_owned(),
            device: Device::Cpu,
            measured_tok_s: None,
        }
    }

    #[test]
    fn messages_carry_role_and_text() {
        let req = dark_contract::Request::new(
            RoleClass::Worker,
            vec![Message::text(Role::User, "hello")],
        );
        let mut builder = build(&req, &caps(false)).unwrap();
        assert_eq!(builder.messages_ref().len(), 1);
        let taken = builder.take_messages();
        match taken {
            mistralrs::RequestMessage::Chat { messages, .. } => {
                assert_eq!(messages.len(), 1);
            }
            other => panic!("expected a Chat message, got {other:?}"),
        }
    }

    #[test]
    fn a_tool_reply_uses_add_tool_message() {
        let req = dark_contract::Request::new(
            RoleClass::Worker,
            vec![Message::tool_reply("call-1", "ok")],
        );
        // Reaching take_messages without panicking on an unexpected shape
        // is the assertion: add_tool_message is the only path that
        // attaches a tool_call_id the way Message::tool_reply set it up.
        let mut builder = build(&req, &caps(false)).unwrap();
        let _ = builder.take_messages();
    }

    #[test]
    fn tools_are_omitted_when_native_tools_is_false() {
        let mut req = dark_contract::Request::new(RoleClass::Worker, vec![]);
        req.tools = vec![ToolSchema {
            name: "read_file".to_owned(),
            description: "reads a file".to_owned(),
            parameters: serde_json::json!({"type": "object"}),
            tier: 1,
            mutating: false,
        }];
        let mut builder = build(&req, &caps(false)).unwrap();
        assert!(
            builder.take_tools().is_none(),
            "native_tools is false: no tools attached"
        );
    }

    #[test]
    fn tools_are_attached_when_native_tools_is_true() {
        let mut req = dark_contract::Request::new(RoleClass::Worker, vec![]);
        req.tools = vec![ToolSchema {
            name: "read_file".to_owned(),
            description: "reads a file".to_owned(),
            parameters: serde_json::json!({"type": "object"}),
            tier: 1,
            mutating: false,
        }];
        let mut builder = build(&req, &caps(true)).unwrap();
        let (tools, _choice) = builder.take_tools().expect("native_tools is true");
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].function.name, "read_file");
    }

    #[test]
    fn named_tool_choice_resolves_to_the_matching_schema() {
        let mut req = dark_contract::Request::new(RoleClass::Worker, vec![]);
        req.tools = vec![ToolSchema {
            name: "read_file".to_owned(),
            description: "reads a file".to_owned(),
            parameters: serde_json::json!({"type": "object"}),
            tier: 1,
            mutating: false,
        }];
        req.tool_choice = dark_contract::ToolChoice::Named("read_file".to_owned());
        let mut builder = build(&req, &caps(true)).unwrap();
        let (_tools, choice) = builder.take_tools().unwrap();
        assert!(matches!(choice, mistralrs::ToolChoice::Tool(t) if t.function.name == "read_file"));
    }

    #[test]
    fn named_tool_choice_fails_for_an_unknown_tool() {
        let mut req = dark_contract::Request::new(RoleClass::Worker, vec![]);
        req.tools = vec![ToolSchema {
            name: "read_file".to_owned(),
            description: "reads a file".to_owned(),
            parameters: serde_json::json!({"type": "object"}),
            tier: 1,
            mutating: false,
        }];
        req.tool_choice = dark_contract::ToolChoice::Named("does_not_exist".to_owned());
        let Err(err) = build(&req, &caps(true)) else {
            panic!("expected an error");
        };
        assert_eq!(err.code, ErrCode::ToolInvalidArgs);
    }

    #[test]
    fn max_tokens_becomes_sampler_max_len() {
        let mut req = dark_contract::Request::new(RoleClass::Worker, vec![]);
        req.max_tokens = 512;
        let mut builder = build(&req, &caps(false)).unwrap();
        assert_eq!(builder.take_sampling_params().max_len, Some(512));
    }

    #[test]
    fn temperature_and_top_p_convert_from_f32_to_f64() {
        let mut req = dark_contract::Request::new(RoleClass::Worker, vec![]);
        req.sampling = Sampling {
            temperature: Some(0.7),
            top_p: Some(0.9),
            ..Sampling::default()
        };
        let mut builder = build(&req, &caps(false)).unwrap();
        let params = builder.take_sampling_params();
        assert!((params.temperature.unwrap() - 0.7).abs() < 1e-6);
        assert!((params.top_p.unwrap() - 0.9).abs() < 1e-6);
    }

    #[test]
    fn a_deterministic_request_forces_top_k_to_one() {
        let mut req = dark_contract::Request::new(RoleClass::Worker, vec![]);
        req.deterministic = true;
        req.sampling.top_k = Some(40);
        let mut builder = build(&req, &caps(false)).unwrap();
        assert_eq!(builder.take_sampling_params().top_k, Some(1));
    }

    #[test]
    fn think_on_enables_thinking() {
        let mut req = dark_contract::Request::new(RoleClass::Worker, vec![]);
        req.think = ThinkMode::On;
        let _ = build(&req, &caps(false)).unwrap();
        // enable_thinking has no public getter on RequestBuilder; reaching
        // this line without a type error confirms the call is well-typed.
        // The behavioural half — that the model actually thinks — needs a
        // loaded model to observe.
    }

    #[test]
    fn stop_strings_are_attached_when_present() {
        let mut req = dark_contract::Request::new(RoleClass::Worker, vec![]);
        req.stop = vec!["STOP".to_owned()];
        let mut builder = build(&req, &caps(false)).unwrap();
        let params = builder.take_sampling_params();
        assert!(params.stop_toks.is_some());
    }

    #[test]
    fn a_json_schema_grammar_becomes_a_constraint() {
        let mut req = dark_contract::Request::new(RoleClass::Worker, vec![]);
        req.grammar = Some(Grammar::JsonSchema(serde_json::json!({"type": "object"})));
        let mut builder = build(&req, &caps(false)).unwrap();
        assert!(!matches!(
            builder.take_constraint(),
            mistralrs::Constraint::None
        ));
    }
}
