//! Maps a `mistralrs::Response` to zero or more [`dark_contract::Chunk`]s
//! (task unit `B4`, steps 1 to 3).
//!
//! [`map`] is a pure function of a real `mistralrs::Response`, so a test
//! builds one by hand — no loaded model needed — and checks what comes
//! out. mistral.rs finalises a tool call before it ever appears in
//! `Delta.tool_calls` (verified against `mistralrs-core`'s streaming
//! sampler, which only populates that field once its own parser considers
//! the call complete; see `docs/adr/0006`): there is no partial-JSON
//! fragment to accumulate the way an OpenAI-style stream sends one.
//! [`map`] still emits one [`dark_contract::Chunk::ToolCallDelta`] per call
//! rather than a whole-call variant, because [`dark_contract::Chunk`]'s
//! shape is the one seam every caller already accumulates by index (see
//! `dark_engine_fake::collect_tool_calls`); a single-fragment call is the
//! simplest case that accumulator handles, not a special one.

use dark_contract::{Chunk, FinishReason, Usage};

/// Maps one `mistralrs::Response` to the [`Chunk`]s it produces.
///
/// A chat stream from [`super::live::stream_chat_request`] only ever
/// yields [`mistralrs::Response::Chunk`] and, on the final message,
/// [`mistralrs::Response::Done`] (mistral.rs delivers the last chunk's
/// content through `Chunk`, not `Done` — `Done`'s
/// [`mistralrs::ChatCompletionResponse`] carries the *complete* message
/// again, which this function does not re-emit as text, to avoid
/// duplicating it). Every other [`mistralrs::Response`] variant belongs to
/// a request kind [`super::request::build`] never issues (image
/// generation, speech, raw logits, embeddings), so this function maps
/// those to no chunks at all rather than guessing at a translation.
#[must_use]
pub fn map(response: &mistralrs::Response) -> Vec<Chunk> {
    match response {
        mistralrs::Response::Chunk(chunk) => map_chunk(chunk),
        mistralrs::Response::Done(done) => map_done(done),
        mistralrs::Response::ModelError(message, done) => {
            tracing::warn!(%message, "mistral.rs reported a model error mid-stream");
            vec![
                Chunk::Usage(to_dark_usage(&done.usage)),
                Chunk::Done(FinishReason::Error),
            ]
        }
        mistralrs::Response::InternalError(err) => {
            tracing::error!(%err, "mistral.rs internal error");
            vec![Chunk::Done(FinishReason::Error)]
        }
        mistralrs::Response::ValidationError(err) => {
            tracing::warn!(%err, "mistral.rs rejected the request");
            vec![Chunk::Done(FinishReason::Error)]
        }
        // Completion, image, speech, raw, and embedding responses: no
        // request this crate builds asks for one of these on the chat
        // path, so there is nothing to translate.
        _ => Vec::new(),
    }
}

/// Maps a streaming [`mistralrs::ChatCompletionChunkResponse`].
fn map_chunk(chunk: &mistralrs::ChatCompletionChunkResponse) -> Vec<Chunk> {
    let mut chunks = Vec::new();
    let Some(choice) = chunk.choices.first() else {
        return chunks;
    };

    if let Some(reasoning) = &choice.delta.reasoning_content
        && !reasoning.is_empty()
    {
        chunks.push(Chunk::Reasoning(reasoning.clone()));
    }
    if let Some(content) = &choice.delta.content
        && !content.is_empty()
    {
        chunks.push(Chunk::Text(content.clone()));
    }
    if let Some(tool_calls) = &choice.delta.tool_calls {
        for call in tool_calls {
            chunks.push(Chunk::ToolCallDelta {
                index: call.index,
                id: Some(call.id.clone()),
                name: Some(call.function.name.clone()),
                args_fragment: call.function.arguments.clone(),
            });
        }
    }
    if let Some(usage) = &chunk.usage {
        chunks.push(Chunk::Usage(to_dark_usage(usage)));
    }
    if let Some(reason) = &choice.finish_reason {
        chunks.push(Chunk::Done(to_finish_reason(reason)));
    }
    chunks
}

/// Maps a non-streaming [`mistralrs::ChatCompletionResponse`], for the
/// [`mistralrs::Response::Done`] and [`mistralrs::Response::ModelError`]
/// variants.
fn map_done(done: &mistralrs::ChatCompletionResponse) -> Vec<Chunk> {
    let mut chunks = vec![Chunk::Usage(to_dark_usage(&done.usage))];
    let Some(choice) = done.choices.first() else {
        chunks.push(Chunk::Done(FinishReason::Stop));
        return chunks;
    };
    chunks.push(Chunk::Done(to_finish_reason(&choice.finish_reason)));
    chunks
}

/// Converts mistral.rs's `finish_reason` string to [`FinishReason`].
///
/// mistral.rs's own values are `"stop"`, `"length"`, `"tool_calls"`, and
/// `"canceled"` (verified against `mistralrs_core::sequence::StopReason`'s
/// `Display` impl); anything else maps to [`FinishReason::Error`] rather
/// than panicking on a string this crate does not recognise.
fn to_finish_reason(reason: &str) -> FinishReason {
    match reason {
        "stop" => FinishReason::Stop,
        "length" => FinishReason::Length,
        "tool_calls" => FinishReason::ToolCalls,
        "canceled" => FinishReason::Cancelled,
        other => {
            tracing::warn!(reason = other, "unrecognised mistral.rs finish reason");
            FinishReason::Error
        }
    }
}

/// Converts a `mistralrs::Usage` to [`Usage`].
///
/// mistral.rs reports no cached-token count and does not separate
/// reasoning tokens from completion tokens at this layer, so both map to
/// `0` here — a real figure for either would need to come from a lower
/// layer this crate does not have access to yet. See `docs/adr/0006`.
fn to_dark_usage(usage: &mistralrs::Usage) -> Usage {
    Usage {
        prompt_tokens: usage.prompt_tokens,
        completion_tokens: usage.completion_tokens,
        reasoning_tokens: 0,
        cached_tokens: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mistralrs::{
        CalledFunction, ChatCompletionChunkResponse, ChatCompletionResponse, Choice, ChunkChoice,
        Delta, ResponseMessage, ToolCallResponse, ToolCallType,
    };

    fn chunk_choice(delta: Delta, finish_reason: Option<&str>) -> ChatCompletionChunkResponse {
        ChatCompletionChunkResponse {
            id: "chatcmpl-1".to_owned(),
            choices: vec![ChunkChoice {
                finish_reason: finish_reason.map(ToOwned::to_owned),
                index: 0,
                delta,
                logprobs: None,
            }],
            created: 0,
            model: "test-model".to_owned(),
            system_fingerprint: "local".to_owned(),
            object: "chat.completion.chunk".to_owned(),
            usage: None,
        }
    }

    fn text_delta(text: &str) -> Delta {
        Delta {
            content: Some(text.to_owned()),
            role: "assistant".to_owned(),
            tool_calls: None,
            reasoning_content: None,
        }
    }

    fn usage(prompt: usize, completion: usize) -> mistralrs::Usage {
        mistralrs::Usage {
            completion_tokens: completion,
            prompt_tokens: prompt,
            total_tokens: prompt + completion,
            avg_tok_per_sec: 0.0,
            avg_prompt_tok_per_sec: 0.0,
            avg_compl_tok_per_sec: 0.0,
            total_time_sec: 0.0,
            total_prompt_time_sec: 0.0,
            total_completion_time_sec: 0.0,
        }
    }

    #[test]
    fn text_content_maps_to_chunk_text() {
        let response = mistralrs::Response::Chunk(chunk_choice(text_delta("hello"), None));
        let chunks = map(&response);
        assert_eq!(chunks, vec![Chunk::Text("hello".to_owned())]);
    }

    #[test]
    fn reasoning_content_maps_to_chunk_reasoning_before_text() {
        let delta = Delta {
            content: Some("the answer".to_owned()),
            role: "assistant".to_owned(),
            tool_calls: None,
            reasoning_content: Some("thinking it through".to_owned()),
        };
        let response = mistralrs::Response::Chunk(chunk_choice(delta, None));
        let chunks = map(&response);
        assert_eq!(
            chunks,
            vec![
                Chunk::Reasoning("thinking it through".to_owned()),
                Chunk::Text("the answer".to_owned()),
            ]
        );
    }

    #[test]
    fn an_empty_delta_produces_no_chunks() {
        let response = mistralrs::Response::Chunk(chunk_choice(
            Delta {
                content: None,
                role: "assistant".to_owned(),
                tool_calls: None,
                reasoning_content: None,
            },
            None,
        ));
        assert_eq!(map(&response), Vec::new());
    }

    #[test]
    fn a_tool_call_maps_to_one_tool_call_delta_with_the_full_arguments() {
        let delta = Delta {
            content: None,
            role: "assistant".to_owned(),
            tool_calls: Some(vec![ToolCallResponse {
                index: 0,
                id: "call-1".to_owned(),
                tp: ToolCallType::Function,
                function: CalledFunction {
                    name: "read_file".to_owned(),
                    arguments: r#"{"path":"src/lib.rs"}"#.to_owned(),
                },
            }]),
            reasoning_content: None,
        };
        let response = mistralrs::Response::Chunk(chunk_choice(delta, Some("tool_calls")));
        let chunks = map(&response);
        assert_eq!(
            chunks,
            vec![
                Chunk::ToolCallDelta {
                    index: 0,
                    id: Some("call-1".to_owned()),
                    name: Some("read_file".to_owned()),
                    args_fragment: r#"{"path":"src/lib.rs"}"#.to_owned(),
                },
                Chunk::Done(FinishReason::ToolCalls),
            ]
        );
    }

    #[test]
    fn two_tool_calls_keep_their_own_index() {
        let delta = Delta {
            content: None,
            role: "assistant".to_owned(),
            tool_calls: Some(vec![
                ToolCallResponse {
                    index: 0,
                    id: "call-1".to_owned(),
                    tp: ToolCallType::Function,
                    function: CalledFunction {
                        name: "a".to_owned(),
                        arguments: "{}".to_owned(),
                    },
                },
                ToolCallResponse {
                    index: 1,
                    id: "call-2".to_owned(),
                    tp: ToolCallType::Function,
                    function: CalledFunction {
                        name: "b".to_owned(),
                        arguments: "{}".to_owned(),
                    },
                },
            ]),
            reasoning_content: None,
        };
        let response = mistralrs::Response::Chunk(chunk_choice(delta, None));
        let chunks = map(&response);
        let indices: Vec<usize> = chunks
            .iter()
            .filter_map(|chunk| match chunk {
                Chunk::ToolCallDelta { index, .. } => Some(*index),
                _ => None,
            })
            .collect();
        assert_eq!(indices, vec![0, 1]);
    }

    #[test]
    fn finish_reasons_map_to_the_matching_variant() {
        for (mistral_reason, expected) in [
            ("stop", FinishReason::Stop),
            ("length", FinishReason::Length),
            ("tool_calls", FinishReason::ToolCalls),
            ("canceled", FinishReason::Cancelled),
            ("something-unrecognised", FinishReason::Error),
        ] {
            let response =
                mistralrs::Response::Chunk(chunk_choice(text_delta(""), Some(mistral_reason)));
            let chunks = map(&response);
            assert!(
                chunks.contains(&Chunk::Done(expected)),
                "{mistral_reason} did not map to {expected:?}: got {chunks:?}"
            );
        }
    }

    #[test]
    fn usage_carries_prompt_and_completion_tokens() {
        let mut response = chunk_choice(text_delta(""), None);
        response.usage = Some(usage(120, 30));
        let chunks = map(&mistralrs::Response::Chunk(response));
        assert!(chunks.contains(&Chunk::Usage(Usage {
            prompt_tokens: 120,
            completion_tokens: 30,
            reasoning_tokens: 0,
            cached_tokens: 0,
        })));
    }

    #[test]
    fn done_maps_usage_and_finish_reason_from_the_first_choice() {
        let done = ChatCompletionResponse {
            id: "chatcmpl-1".to_owned(),
            choices: vec![Choice {
                finish_reason: "stop".to_owned(),
                index: 0,
                message: ResponseMessage {
                    content: Some("final answer".to_owned()),
                    role: "assistant".to_owned(),
                    tool_calls: None,
                    reasoning_content: None,
                },
                logprobs: None,
            }],
            created: 0,
            model: "test-model".to_owned(),
            system_fingerprint: "local".to_owned(),
            object: "chat.completion".to_owned(),
            usage: usage(10, 5),
        };
        let chunks = map(&mistralrs::Response::Done(done));
        assert_eq!(
            chunks,
            vec![
                Chunk::Usage(Usage {
                    prompt_tokens: 10,
                    completion_tokens: 5,
                    reasoning_tokens: 0,
                    cached_tokens: 0,
                }),
                Chunk::Done(FinishReason::Stop),
            ]
        );
    }

    #[test]
    fn an_internal_error_maps_to_a_single_error_done_chunk() {
        let response = mistralrs::Response::InternalError("boom".into());
        assert_eq!(map(&response), vec![Chunk::Done(FinishReason::Error)]);
    }

    #[test]
    fn a_validation_error_maps_to_a_single_error_done_chunk() {
        let response = mistralrs::Response::ValidationError("bad request".into());
        assert_eq!(map(&response), vec![Chunk::Done(FinishReason::Error)]);
    }
}
