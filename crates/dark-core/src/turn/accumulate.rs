//! Accumulates a chunk stream into one assistant message.
//!
//! [`Engine::stream`](dark_contract::Engine::stream) produces text,
//! reasoning, and tool-call fragments in whatever order the model emits
//! them, and it may split one tool call across many chunks. [`Accumulator`]
//! collects those fragments and produces the single [`Message`] that goes
//! into the history, plus the [`FinishReason`] that decides what the turn
//! loop does next. See Do steps 3 and 4 of task unit `A2`.
//!
//! # A partial tool call is still a tool call
//!
//! A stream can end in the middle of a tool call: the engine names the call
//! but never finishes its argument text, or the person cancels. The
//! accumulator still reports that call. The turn loop must answer every
//! call it was told about, because an unanswered call breaks the chat
//! template, and a call it never heard about is a call it cannot answer.
//! [`PendingCall::into_call`] therefore reports whether the arguments
//! parsed, rather than dropping a call whose arguments did not.

use dark_contract::{Chunk, FinishReason, Message, Part, Role, ToolCall};

/// One tool call as it arrives, before its fragments are complete.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct PendingCall {
    /// The call identifier, when the engine has produced it.
    id: Option<String>,
    /// The tool name, when the engine has produced it.
    name: Option<String>,
    /// The argument text so far, concatenated in arrival order.
    args_text: String,
}

impl PendingCall {
    /// Turns this pending call into a [`ToolCall`], and reports whether its
    /// arguments parsed as JSON.
    ///
    /// `index` names the position in the stream, and stands in for an
    /// identifier the engine never produced: a call with no identifier still
    /// needs one, or its reply cannot be tied back to it.
    ///
    /// Arguments that do not parse become [`serde_json::Value::Null`] and
    /// the second element of the pair is `false`. The caller answers such a
    /// call with an error rather than dropping it.
    fn into_call(self, index: usize) -> (ToolCall, bool) {
        let id = self.id.unwrap_or_else(|| format!("call-{index}"));
        let name = self.name.unwrap_or_default();

        let trimmed = self.args_text.trim();
        let (args, parsed) = if trimmed.is_empty() {
            // No argument text at all is a valid call to a tool that takes
            // no arguments.
            (serde_json::Value::Object(serde_json::Map::new()), true)
        } else {
            match serde_json::from_str(trimmed) {
                Ok(value) => (value, true),
                Err(_) => (serde_json::Value::Null, false),
            }
        };

        (ToolCall { id, name, args }, parsed)
    }
}

/// One tool call that the model asked for, and whether the harness could
/// read its arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssuedCall {
    /// The call itself.
    pub call: ToolCall,
    /// Whether the argument text parsed as JSON.
    ///
    /// When this is `false`, [`IssuedCall::call`] carries
    /// [`serde_json::Value::Null`] arguments. The turn loop answers the call
    /// with an error instead of invoking the tool. It never drops the call.
    pub args_parsed: bool,
}

/// What one round trip produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Accumulated {
    /// The assistant message, ready for the history.
    pub message: Message,
    /// The calls the model asked for, in the order the engine indexed them.
    pub calls: Vec<IssuedCall>,
    /// Why the stream ended.
    ///
    /// A stream that ends without a [`Chunk::Done`] reports
    /// [`FinishReason::Error`]: the engine broke its contract, and the turn
    /// loop must not read that as a clean stop.
    pub finish: FinishReason,
}

/// Collects the chunks of one round trip.
#[derive(Debug, Default)]
pub struct Accumulator {
    text: String,
    reasoning: String,
    /// Pending calls by engine index. An engine may index calls sparsely,
    /// so this grows to fit rather than assuming call `n` arrives after
    /// `n-1`. An index the engine never touched stays `None` and is not a
    /// call: see [`Accumulator::finish`].
    calls: Vec<Option<PendingCall>>,
    finish: Option<FinishReason>,
}

impl Accumulator {
    /// Creates an empty accumulator.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether the stream has reported a finish reason.
    #[must_use]
    pub fn is_done(&self) -> bool {
        self.finish.is_some()
    }

    /// Folds one chunk in.
    ///
    /// [`Chunk::Usage`] and [`Chunk::ModelLoading`] carry no message content,
    /// so this ignores them. The turn loop forwards them to the event bus
    /// itself.
    pub fn push(&mut self, chunk: &Chunk) {
        match chunk {
            Chunk::Text(text) => self.text.push_str(text),
            Chunk::Reasoning(text) => self.reasoning.push_str(text),
            Chunk::ToolCallDelta {
                index,
                id,
                name,
                args_fragment,
            } => {
                if self.calls.len() <= *index {
                    self.calls.resize(index + 1, None);
                }
                let pending = self.calls[*index].get_or_insert_with(PendingCall::default);
                if let Some(id) = id {
                    pending.id = Some(id.clone());
                }
                if let Some(name) = name {
                    pending.name = Some(name.clone());
                }
                pending.args_text.push_str(args_fragment);
            }
            Chunk::Done(reason) => self.finish = Some(*reason),
            Chunk::Usage(_) | Chunk::ModelLoading { .. } => {}
        }
    }

    /// Produces the assistant message and the calls it requests.
    ///
    /// `cancelled` forces [`FinishReason::Cancelled`], for the case where the
    /// turn loop cancelled the request and stopped reading before the engine
    /// reported a reason of its own. The accumulated calls still come back,
    /// because a cancelled call still needs its reply.
    ///
    /// An index that the engine skipped produces no call. Only an index it
    /// named, or sent argument text for, becomes one.
    #[must_use]
    pub fn finish(self, cancelled: bool) -> Accumulated {
        // An index the engine skipped is a gap, not a call. Answering a
        // call the model never made would put a reply in the history with
        // nothing to tie it to.
        let calls: Vec<IssuedCall> = self
            .calls
            .into_iter()
            .enumerate()
            .filter_map(|(index, pending)| {
                let (call, args_parsed) = pending?.into_call(index);
                Some(IssuedCall { call, args_parsed })
            })
            .collect();

        let mut parts = Vec::new();
        if !self.text.is_empty() {
            parts.push(Part::Text(self.text));
        }

        let message = Message {
            role: Role::Assistant,
            parts,
            tool_calls: calls.iter().map(|issued| issued.call.clone()).collect(),
            tool_call_id: None,
            reasoning: (!self.reasoning.is_empty()).then_some(self.reasoning),
            pinned: false,
        };

        let finish = if cancelled {
            FinishReason::Cancelled
        } else {
            // A stream that stopped without saying why did not stop cleanly.
            self.finish.unwrap_or(FinishReason::Error)
        };

        Accumulated {
            message,
            calls,
            finish,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn delta(index: usize, name: Option<&str>, fragment: &str) -> Chunk {
        Chunk::ToolCallDelta {
            index,
            id: name.map(|n| format!("id-{n}")),
            name: name.map(ToOwned::to_owned),
            args_fragment: fragment.to_owned(),
        }
    }

    #[test]
    fn text_chunks_concatenate_in_order() {
        let mut acc = Accumulator::new();
        acc.push(&Chunk::Text("hello ".to_owned()));
        acc.push(&Chunk::Text("world".to_owned()));
        acc.push(&Chunk::Done(FinishReason::Stop));

        let out = acc.finish(false);
        assert_eq!(out.message.text_content(), "hello world");
        assert_eq!(out.finish, FinishReason::Stop);
    }

    #[test]
    fn reasoning_stays_out_of_the_parts_and_lands_in_the_field() {
        let mut acc = Accumulator::new();
        acc.push(&Chunk::Reasoning("let me think".to_owned()));
        acc.push(&Chunk::Text("the answer".to_owned()));
        acc.push(&Chunk::Done(FinishReason::Stop));

        let out = acc.finish(false);
        assert_eq!(out.message.text_content(), "the answer");
        assert_eq!(out.message.reasoning.as_deref(), Some("let me think"));
    }

    #[test]
    fn one_call_split_across_many_chunks_reassembles() {
        let mut acc = Accumulator::new();
        acc.push(&delta(0, Some("read_file"), "{\"path\":"));
        acc.push(&delta(0, None, "\"src/lib.rs\"}"));
        acc.push(&Chunk::Done(FinishReason::ToolCalls));

        let out = acc.finish(false);
        assert_eq!(out.calls.len(), 1);
        assert!(out.calls[0].args_parsed);
        assert_eq!(out.calls[0].call.name, "read_file");
        assert_eq!(out.calls[0].call.args["path"], "src/lib.rs");
    }

    #[test]
    fn many_calls_in_one_message_keep_their_indexes() {
        let mut acc = Accumulator::new();
        acc.push(&delta(0, Some("read_file"), "{\"path\":\"a\"}"));
        acc.push(&delta(1, Some("list_dir"), "{\"path\":\"b\"}"));
        acc.push(&Chunk::Done(FinishReason::ToolCalls));

        let out = acc.finish(false);
        assert_eq!(out.calls.len(), 2);
        assert_eq!(out.calls[0].call.name, "read_file");
        assert_eq!(out.calls[1].call.name, "list_dir");
    }

    #[test]
    fn a_call_whose_arguments_never_finished_is_still_reported() {
        let mut acc = Accumulator::new();
        acc.push(&delta(0, Some("read_file"), "{\"path\": \"src/li"));
        acc.push(&Chunk::Done(FinishReason::ToolCalls));

        let out = acc.finish(false);
        assert_eq!(out.calls.len(), 1, "a truncated call must not vanish");
        assert!(
            !out.calls[0].args_parsed,
            "the caller must be told the arguments did not parse"
        );
        assert_eq!(out.calls[0].call.args, serde_json::Value::Null);
    }

    #[test]
    fn a_call_with_no_arguments_parses_as_an_empty_object() {
        let mut acc = Accumulator::new();
        acc.push(&delta(0, Some("status"), ""));
        acc.push(&Chunk::Done(FinishReason::ToolCalls));

        let out = acc.finish(false);
        assert!(out.calls[0].args_parsed);
        assert!(out.calls[0].call.args.is_object());
    }

    #[test]
    fn a_call_with_no_identifier_gets_one_from_its_index() {
        let mut acc = Accumulator::new();
        acc.push(&Chunk::ToolCallDelta {
            index: 0,
            id: None,
            name: Some("read_file".to_owned()),
            args_fragment: "{}".to_owned(),
        });
        acc.push(&Chunk::Done(FinishReason::ToolCalls));

        let out = acc.finish(false);
        assert_eq!(
            out.calls[0].call.id, "call-0",
            "a reply needs an identifier to tie itself to"
        );
    }

    #[test]
    fn a_stream_that_ends_without_a_reason_is_an_error_not_a_clean_stop() {
        let mut acc = Accumulator::new();
        acc.push(&Chunk::Text("half an ans".to_owned()));

        assert!(!acc.is_done());
        assert_eq!(acc.finish(false).finish, FinishReason::Error);
    }

    #[test]
    fn cancellation_overrides_the_reported_reason_and_keeps_the_calls() {
        let mut acc = Accumulator::new();
        acc.push(&delta(0, Some("run_command"), "{\"command\":\"sleep 60\"}"));
        acc.push(&Chunk::Done(FinishReason::ToolCalls));

        let out = acc.finish(true);
        assert_eq!(out.finish, FinishReason::Cancelled);
        assert_eq!(
            out.calls.len(),
            1,
            "a cancelled call still needs its reply, so it must survive"
        );
    }

    #[test]
    fn usage_and_loading_chunks_do_not_reach_the_message() {
        let mut acc = Accumulator::new();
        acc.push(&Chunk::ModelLoading {
            model: "qwen3-14b".to_owned(),
            progress: 0.5,
        });
        acc.push(&Chunk::Usage(dark_contract::Usage::default()));
        acc.push(&Chunk::Text("hi".to_owned()));
        acc.push(&Chunk::Done(FinishReason::Stop));

        let out = acc.finish(false);
        assert_eq!(out.message.text_content(), "hi");
        assert!(out.message.tool_calls.is_empty());
    }

    #[test]
    fn a_sparse_index_yields_one_call_not_a_run_of_empty_ones() {
        let mut acc = Accumulator::new();
        acc.push(&delta(3, Some("read_file"), "{}"));
        acc.push(&Chunk::Done(FinishReason::ToolCalls));

        let out = acc.finish(false);
        assert_eq!(
            out.calls.len(),
            1,
            "an index the engine skipped is a gap, not a call to answer"
        );
        assert_eq!(out.calls[0].call.name, "read_file");
        assert_eq!(
            out.calls[0].call.id, "id-read_file",
            "the call keeps the identifier the engine gave it"
        );
    }
}
