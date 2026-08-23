//! Accumulates [`dark_contract::Chunk::ToolCallDelta`] fragments by index
//! (task unit `B4`, step 2).
//!
//! mistral.rs delivers a whole tool call in one [`super::response::map`]
//! call (see that module's documentation), but a caller of this crate must
//! not assume that: the contract's [`Chunk::ToolCallDelta`] shape allows a
//! true multi-fragment stream, and a future engine (or a different
//! mistral.rs version) may use it. [`Accumulator`] reassembles by index
//! regardless of how many fragments one call arrives in, using the same
//! algorithm `dark-engine-fake`'s `collect_tool_calls` already uses to
//! build its own scripted deltas back into calls, so a caller sees the
//! same accumulation behaviour from either engine.

use dark_contract::{Chunk, ErrCode, Error, Result, ToolCall};

/// Reassembles [`Chunk::ToolCallDelta`] fragments into complete
/// [`ToolCall`]s, keyed by the index each fragment names.
#[derive(Debug, Default)]
pub struct Accumulator {
    parts: Vec<Option<Part>>,
}

/// One tool call's accumulated state.
#[derive(Debug, Default, Clone)]
struct Part {
    id: Option<String>,
    name: Option<String>,
    args: String,
}

impl Accumulator {
    /// Creates an empty accumulator.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Feeds one chunk. Chunks other than [`Chunk::ToolCallDelta`] are
    /// ignored: a caller can feed a whole stream through this method
    /// without filtering first.
    pub fn feed(&mut self, chunk: &Chunk) {
        let Chunk::ToolCallDelta {
            index,
            id,
            name,
            args_fragment,
        } = chunk
        else {
            return;
        };
        if self.parts.len() <= *index {
            self.parts.resize(*index + 1, None);
        }
        let part = self.parts[*index].get_or_insert_with(Part::default);
        if id.is_some() {
            part.id.clone_from(id);
        }
        if name.is_some() {
            part.name.clone_from(name);
        }
        part.args.push_str(args_fragment);
    }

    /// Finishes accumulation and returns the complete [`ToolCall`]s, in
    /// index order.
    ///
    /// # Errors
    ///
    /// Returns [`ErrCode::ToolInvalidArgs`] when some call's accumulated
    /// argument text is not valid JSON.
    pub fn finish(self) -> Result<Vec<ToolCall>> {
        self.parts
            .into_iter()
            .enumerate()
            .filter_map(|(index, part)| part.map(|part| (index, part)))
            .map(|(index, part)| {
                let args = serde_json::from_str(&part.args).map_err(|err| {
                    Error::new(
                        ErrCode::ToolInvalidArgs,
                        format!("tool call {index}: bad arguments: {err}"),
                    )
                })?;
                Ok(ToolCall {
                    id: part.id.unwrap_or_default(),
                    name: part.name.unwrap_or_default(),
                    args,
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_single_fragment_call_reassembles_whole() {
        let mut acc = Accumulator::new();
        acc.feed(&Chunk::ToolCallDelta {
            index: 0,
            id: Some("call-1".to_owned()),
            name: Some("read_file".to_owned()),
            args_fragment: r#"{"path":"src/lib.rs"}"#.to_owned(),
        });
        let calls = acc.finish().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].id, "call-1");
        assert_eq!(calls[0].name, "read_file");
        assert_eq!(calls[0].args, serde_json::json!({"path": "src/lib.rs"}));
    }

    #[test]
    fn a_multi_fragment_call_concatenates_arguments_in_order() {
        let mut acc = Accumulator::new();
        acc.feed(&Chunk::ToolCallDelta {
            index: 0,
            id: Some("call-1".to_owned()),
            name: Some("read_file".to_owned()),
            args_fragment: r#"{"path":"#.to_owned(),
        });
        acc.feed(&Chunk::ToolCallDelta {
            index: 0,
            id: None,
            name: None,
            args_fragment: r#""src/lib.rs"}"#.to_owned(),
        });
        let calls = acc.finish().unwrap();
        assert_eq!(calls[0].args, serde_json::json!({"path": "src/lib.rs"}));
    }

    #[test]
    fn two_calls_at_different_indices_stay_separate() {
        let mut acc = Accumulator::new();
        acc.feed(&Chunk::ToolCallDelta {
            index: 1,
            id: Some("call-b".to_owned()),
            name: Some("b".to_owned()),
            args_fragment: "{}".to_owned(),
        });
        acc.feed(&Chunk::ToolCallDelta {
            index: 0,
            id: Some("call-a".to_owned()),
            name: Some("a".to_owned()),
            args_fragment: "{}".to_owned(),
        });
        let calls = acc.finish().unwrap();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].id, "call-a", "index order, not arrival order");
        assert_eq!(calls[1].id, "call-b");
    }

    #[test]
    fn non_tool_call_chunks_are_ignored() {
        let mut acc = Accumulator::new();
        acc.feed(&Chunk::Text("hello".to_owned()));
        acc.feed(&Chunk::Done(dark_contract::FinishReason::Stop));
        assert!(acc.finish().unwrap().is_empty());
    }

    #[test]
    fn invalid_accumulated_json_fails_with_tool_invalid_args() {
        let mut acc = Accumulator::new();
        acc.feed(&Chunk::ToolCallDelta {
            index: 0,
            id: Some("call-1".to_owned()),
            name: Some("x".to_owned()),
            args_fragment: "{not json".to_owned(),
        });
        let err = acc.finish().unwrap_err();
        assert_eq!(err.code, ErrCode::ToolInvalidArgs);
    }

    #[test]
    fn an_empty_accumulator_finishes_to_an_empty_list() {
        assert!(Accumulator::new().finish().unwrap().is_empty());
    }
}
