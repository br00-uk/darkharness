//! The context tail: tool schemas, Lexicon chunks, and the turn's messages.
//!
//! The tail changes as a turn runs (Rule 8): a tool result appends to it
//! while the prefix stays fixed. This module assembles the tail
//! ([`assemble_tail`]) and evicts Lexicon chunks when the tail runs over
//! budget ([`evict_lexicon_chunks`]), which the build specification requires
//! to happen before any history compaction (Do step 7).

use dark_contract::{Engine, Message, Result, Role, RoleClass, ToolSchema};

use super::tokens::count_tokens;

/// One chunk of documentation that the Lexicon retrieved for this turn.
///
/// A caller ranks chunks by relevance and passes them in ranked order, most
/// relevant first. [`evict_lexicon_chunks`] drops from the back of that
/// order, so the least relevant chunk goes first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LexiconChunk {
    /// Where the chunk came from, for example a documentation file path.
    pub source: String,
    /// The chunk text.
    pub text: String,
}

impl LexiconChunk {
    /// Creates a chunk from a source name and its text.
    pub fn new(source: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            source: source.into(),
            text: text.into(),
        }
    }

    /// Renders this chunk to the [`Message`] the tail carries.
    ///
    /// The chunk keeps its source name in the message text, so a person
    /// reading the transcript can tell where retrieved text came from.
    fn to_message(&self) -> Message {
        Message::text(
            Role::System,
            format!("Lexicon chunk from {}:\n\n{}", self.source, self.text),
        )
    }
}

/// The text a caller supplies to build the tail.
///
/// `history` holds every earlier message, oldest first, and never includes
/// `input`. `tool_results` holds the results that have arrived so far in
/// this turn, in arrival order; a turn loop calls [`assemble_tail`] again
/// each time one more result arrives, and the prefix stays untouched because
/// this function never touches it.
#[derive(Debug, Clone, Copy)]
pub struct TailInputs<'a> {
    /// The tools the model may call this turn.
    pub tool_schemas: &'a [ToolSchema],
    /// The Lexicon chunks retrieved for this turn, most relevant first.
    pub lexicon_chunks: &'a [LexiconChunk],
    /// The message history, oldest first.
    pub history: &'a [Message],
    /// The input message for this turn.
    pub input: &'a Message,
    /// The tool results that have arrived so far, in arrival order.
    pub tool_results: &'a [Message],
}

/// The assembled tail, grouped the way Appendix B accounts for it.
///
/// `tools` is not a [`Message`]: the [`dark_contract::Request::tools`] field
/// carries it separately from the conversation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssembledTail {
    /// Tail section 6: the tool schemas, unchanged from the input.
    pub tools: Vec<ToolSchema>,
    /// Tail section 7: the Lexicon chunks, rendered to messages.
    pub lexicon: Vec<Message>,
    /// Tail section 8: the message history, oldest first.
    pub history: Vec<Message>,
    /// Tail section 9: the input message.
    pub input: Message,
    /// Tail section 10: the tool results that have arrived so far.
    pub tool_results: Vec<Message>,
}

impl AssembledTail {
    /// Returns the tail as one ordered list of messages.
    ///
    /// A turn loop appends this after [`super::prefix::AssembledPrefix::messages`]
    /// to build the full [`dark_contract::Request::messages`] list.
    pub fn messages(&self) -> Vec<Message> {
        let mut out = Vec::with_capacity(
            self.lexicon.len() + self.history.len() + 1 + self.tool_results.len(),
        );
        out.extend(self.lexicon.iter().cloned());
        out.extend(self.history.iter().cloned());
        out.push(self.input.clone());
        out.extend(self.tool_results.iter().cloned());
        out
    }
}

/// Builds the tail in the fixed order the build specification names.
///
/// This function is pure: the same `inputs` always produce byte-identical
/// output. A turn loop calls it again every time a tool result arrives; only
/// `tool_results` grows between calls, so the prefix stays untouched and the
/// earlier tail sections stay byte-identical.
pub fn assemble_tail(inputs: &TailInputs<'_>) -> AssembledTail {
    AssembledTail {
        tools: inputs.tool_schemas.to_vec(),
        lexicon: inputs
            .lexicon_chunks
            .iter()
            .map(LexiconChunk::to_message)
            .collect(),
        history: inputs.history.to_vec(),
        input: inputs.input.clone(),
        tool_results: inputs.tool_results.to_vec(),
    }
}

/// What [`evict_lexicon_chunks`] decided.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LexiconEviction {
    /// The chunks that fit inside `budget_tokens`, in their original order.
    pub kept: Vec<LexiconChunk>,
    /// The chunks that did not fit. The harness retrieves these again next
    /// turn if it still needs them (Do step 7).
    pub evicted: Vec<LexiconChunk>,
    /// The token count of every kept chunk, combined.
    pub kept_tokens: usize,
}

/// Drops whole Lexicon chunks, least relevant first, until the rest fit
/// inside `budget_tokens`.
///
/// Do step 7 says to evict Lexicon chunks before history, and to drop them
/// whole rather than compact them. `chunks` must already be ranked most
/// relevant first; this function only ever drops from the back of that
/// order, and it never truncates a chunk's text.
///
/// # Errors
///
/// Returns an error when [`dark_contract::Engine::tokenize`] fails on any
/// chunk's rendered text.
pub fn evict_lexicon_chunks(
    engine: &dyn Engine,
    class: RoleClass,
    chunks: &[LexiconChunk],
    budget_tokens: usize,
) -> Result<LexiconEviction> {
    let mut kept = Vec::new();
    let mut evicted = Vec::new();
    let mut kept_tokens = 0_usize;

    for chunk in chunks {
        let chunk_tokens = count_tokens(engine, class, &chunk.to_message().text_content())?;
        if kept_tokens + chunk_tokens <= budget_tokens {
            kept_tokens += chunk_tokens;
            kept.push(chunk.clone());
        } else {
            evicted.push(chunk.clone());
        }
    }

    Ok(LexiconEviction {
        kept,
        evicted,
        kept_tokens,
    })
}

#[cfg(test)]
mod tests {
    use dark_contract::Role;
    use dark_engine_fake::FakeEngine;

    use super::*;

    fn schema(name: &str) -> ToolSchema {
        ToolSchema {
            name: name.to_owned(),
            description: "test tool".to_owned(),
            parameters: serde_json::json!({}),
            tier: 1,
            mutating: false,
        }
    }

    #[test]
    fn assemble_tail_orders_sections_lexicon_then_history_then_input_then_results() {
        let tools = vec![schema("read")];
        let lexicon = vec![LexiconChunk::new("docs/a.md", "chunk a")];
        let history = vec![Message::text(Role::User, "earlier turn")];
        let input = Message::text(Role::User, "current input");
        let tool_results = vec![Message::tool_reply("call-1", "result")];

        let inputs = TailInputs {
            tool_schemas: &tools,
            lexicon_chunks: &lexicon,
            history: &history,
            input: &input,
            tool_results: &tool_results,
        };
        let assembled = assemble_tail(&inputs);
        let messages = assembled.messages();

        assert_eq!(messages.len(), 4);
        assert!(messages[0].text_content().contains("chunk a"));
        assert_eq!(messages[1].text_content(), "earlier turn");
        assert_eq!(messages[2].text_content(), "current input");
        assert_eq!(messages[3].text_content(), "result");
        assert_eq!(assembled.tools, tools);
    }

    #[test]
    fn assemble_tail_is_pure() {
        let tools = vec![schema("read")];
        let lexicon = vec![LexiconChunk::new("docs/a.md", "chunk a")];
        let history = vec![Message::text(Role::User, "earlier turn")];
        let input = Message::text(Role::User, "current input");
        let tool_results = vec![];

        let inputs = TailInputs {
            tool_schemas: &tools,
            lexicon_chunks: &lexicon,
            history: &history,
            input: &input,
            tool_results: &tool_results,
        };
        assert_eq!(assemble_tail(&inputs), assemble_tail(&inputs));
    }

    #[test]
    fn evict_lexicon_chunks_keeps_the_most_relevant_chunks_first() {
        let engine = FakeEngine::with_replies(Vec::<String>::new());
        let chunks = vec![
            LexiconChunk::new("most-relevant.md", "alpha beta gamma"),
            LexiconChunk::new("least-relevant.md", "delta epsilon zeta eta theta"),
        ];

        // Budget only room for the first, ranked chunk's rendered tokens.
        let first_tokens = count_tokens(
            &engine,
            RoleClass::Scout,
            &chunks[0].to_message().text_content(),
        )
        .unwrap();

        let eviction =
            evict_lexicon_chunks(&engine, RoleClass::Scout, &chunks, first_tokens).unwrap();

        assert_eq!(eviction.kept, vec![chunks[0].clone()]);
        assert_eq!(eviction.evicted, vec![chunks[1].clone()]);
        assert_eq!(eviction.kept_tokens, first_tokens);
    }

    #[test]
    fn evict_lexicon_chunks_keeps_every_chunk_when_the_budget_is_generous() {
        let engine = FakeEngine::with_replies(Vec::<String>::new());
        let chunks = vec![
            LexiconChunk::new("a.md", "one two"),
            LexiconChunk::new("b.md", "three four"),
        ];

        let eviction = evict_lexicon_chunks(&engine, RoleClass::Scout, &chunks, 10_000).unwrap();

        assert_eq!(eviction.kept, chunks);
        assert!(eviction.evicted.is_empty());
    }

    #[test]
    fn evict_lexicon_chunks_never_truncates_a_chunk_it_evicts() {
        // A chunk that alone busts the budget is dropped whole, not shortened.
        let engine = FakeEngine::with_replies(Vec::<String>::new());
        let chunks = vec![LexiconChunk::new("huge.md", "one two three four five")];

        let eviction = evict_lexicon_chunks(&engine, RoleClass::Scout, &chunks, 1).unwrap();

        assert!(eviction.kept.is_empty());
        assert_eq!(eviction.evicted, chunks);
    }
}
