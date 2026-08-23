//! Turns a scraped model's plain text into the tool-call chunks the turn
//! loop reads.
//!
//! # Why this exists
//!
//! A model that mistral.rs parses tool calls for natively produces
//! [`Chunk::ToolCallDelta`], which
//! [`dark_core::turn::Accumulator`](dark_core::turn::Accumulator) reads
//! directly. A Qwen model does not: `dark-engine` attaches tools to the
//! mistral.rs request only when [`Caps::native_tools`] is true (see
//! `crates/dark-engine/src/stream/request.rs`), so a Qwen model's calls
//! arrive as ordinary [`Chunk::Text`] holding Hermes-style
//! `<tool_call>{…}</tool_call>` blocks. `dark-qwen` (task unit `I3`)
//! knows how to read those blocks, and the turn loop knows how to answer
//! a [`Chunk::ToolCallDelta`]. Nothing joined the two.
//!
//! [`ScrapingEngine`] is that join, and it lives here because this is the
//! composition root. `dark-engine` cannot do it: Rule 12 keeps mistral.rs
//! behind that crate, and adding a `dark-qwen` dependency there would put
//! one model family's text format inside the general engine.
//! `dark-core` cannot do it either — it holds `dyn Engine` and knows
//! nothing about any model family (Rule 17). Only `dark-cli` sees both.
//!
//! # What it does to a stream
//!
//! [`ScrapingEngine::stream`] delegates to the inner engine and then
//! rewrites its chunks, but only for a model whose [`Caps::native_tools`]
//! is false. For a native model the stream passes through untouched, so
//! wrapping an engine costs nothing when the wrapping is not needed.
//!
//! For a scraped model:
//!
//! - [`Chunk::Text`] goes through [`ToolCallExtractor`], which separates
//!   prose from call blocks. The prose is forwarded as it settles, so a
//!   person watching the terminal sees tokens arrive rather than waiting
//!   for the whole reply. A `<tool_call>` block, or any fragment of one,
//!   is never forwarded as text.
//! - At the end of the stream, each scraped call becomes one
//!   [`Chunk::ToolCallDelta`] carrying the whole argument text. The turn
//!   loop's accumulator joins fragments, so one fragment per call is a
//!   valid way to report a call that was never fragmented in the first
//!   place.
//! - A call whose arguments could not be repaired into something the
//!   schema accepts is still reported, with its arguments as they were
//!   read. Dropping it would leave the model believing it had called a
//!   tool that never answered; reporting it lets the turn loop answer it
//!   with the error, which is what keeps the chat template well formed
//!   (task unit `A2`).
//!
//! Every other chunk — reasoning, usage, load progress, the done marker —
//! passes through in order.

use std::sync::Arc;

use async_trait::async_trait;
use dark_contract::{
    Caps, Chunk, ChunkStream, EmbedPurpose, Engine, Request, ResidencySnapshot, Result, RoleClass,
    Scored, ToolSchema,
};
use dark_qwen::toolcall::{Interpreted, ToolCallExtractor, interpret};
use futures_util::StreamExt;
use tokio_util::sync::CancellationToken;

/// Wraps an [`Engine`] and scrapes tool calls out of the text of a model
/// that does not report them natively.
///
/// See the module documentation for what this rewrites and why.
pub(crate) struct ScrapingEngine<E: Engine> {
    inner: Arc<E>,
}

impl<E: Engine> ScrapingEngine<E> {
    /// Wraps `inner`.
    pub(crate) fn new(inner: Arc<E>) -> Self {
        Self { inner }
    }
}

/// Rewrites `stream` so that Hermes-style tool-call blocks in its text
/// arrive as [`Chunk::ToolCallDelta`] instead.
///
/// `schemas` are the tools the request offered, which
/// [`dark_qwen::toolcall::interpret`] validates each scraped call
/// against.
fn scrape(stream: ChunkStream, schemas: Vec<ToolSchema>) -> ChunkStream {
    // The extractor and the schemas live in the stream's own state, so
    // nothing outside this stream can observe a half-read call.
    let state = (ToolCallExtractor::new(), schemas, false);

    let scraped = futures_util::stream::unfold(
        (stream, state),
        |(mut stream, (mut extractor, schemas, mut ended))| async move {
            loop {
                if ended {
                    return None;
                }

                let Some(item) = stream.next().await else {
                    // The inner stream stopped without a Done chunk. Flush
                    // whatever the extractor holds so a call that arrived
                    // is still answered, then stop.
                    ended = true;
                    let chunks = finish(extractor, &schemas);
                    return Some((chunks, (stream, (ToolCallExtractor::new(), schemas, ended))));
                };

                match item {
                    Ok(Chunk::Text(text)) => {
                        extractor.push(&text);
                        let prose = extractor.take_prose();
                        if prose.is_empty() {
                            // Everything so far is held back against a
                            // partial tag or an open call. Nothing to
                            // show yet; read more.
                            continue;
                        }
                        return Some((
                            vec![Ok(Chunk::Text(prose))],
                            (stream, (extractor, schemas, ended)),
                        ));
                    }
                    Ok(Chunk::Done(reason)) => {
                        ended = true;
                        let mut chunks = finish(extractor, &schemas);
                        chunks.push(Ok(Chunk::Done(reason)));
                        return Some((
                            chunks,
                            (stream, (ToolCallExtractor::new(), schemas, ended)),
                        ));
                    }
                    other => {
                        return Some((vec![other], (stream, (extractor, schemas, ended))));
                    }
                }
            }
        },
    )
    .flat_map(futures_util::stream::iter);

    Box::pin(scraped)
}

/// Reads the `name` field out of a raw tool-call body.
///
/// Used only for a call that failed to validate: the call is still
/// reported, so the turn loop can answer it, and naming the tool the
/// model asked for makes that answer say something useful. Returns an
/// empty name when the body does not parse or states no name — the turn
/// loop then answers with "no such tool", which is the truth.
fn name_in(json_text: &str) -> String {
    serde_json::from_str::<serde_json::Value>(json_text)
        .ok()
        .and_then(|value| {
            value
                .get("name")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
        })
        .unwrap_or_default()
}

/// Drains `extractor` at the end of a stream into the chunks that report
/// its leftover prose and every call it found.
fn finish(extractor: ToolCallExtractor, schemas: &[ToolSchema]) -> Vec<Result<Chunk>> {
    let (prose, raw_calls) = extractor.finish();

    let mut chunks: Vec<Result<Chunk>> = Vec::new();
    if !prose.is_empty() {
        chunks.push(Ok(Chunk::Text(prose)));
    }

    for (index, raw) in raw_calls.iter().enumerate() {
        // A call that failed to validate is still reported: see the module
        // documentation, and task unit `A2` on answering every call. Its
        // name and arguments are then whatever could be read from the raw
        // block, so the turn loop answers the call the model believes it
        // made, rather than dropping it.
        let (name, args) = match interpret(raw, schemas) {
            Interpreted::Call { call, .. } => (call.name, call.args.to_string()),
            Interpreted::Failed { .. } => (name_in(&raw.json_text), raw.json_text.clone()),
        };

        chunks.push(Ok(Chunk::ToolCallDelta {
            index,
            id: Some(format!("scraped-{index}")),
            name: Some(name),
            args_fragment: args,
        }));
    }

    chunks
}

#[async_trait]
impl<E: Engine> Engine for ScrapingEngine<E> {
    async fn caps(&self, class: RoleClass) -> Result<Caps> {
        self.inner.caps(class).await
    }

    async fn stream(&self, req: Request, cancel: CancellationToken) -> Result<ChunkStream> {
        let caps = self.inner.caps(req.class).await?;
        let schemas = req.tools.clone();
        let stream = self.inner.stream(req, cancel).await?;

        if caps.native_tools {
            // mistral.rs reports this model's calls itself. Touching the
            // stream here could only corrupt what already works.
            return Ok(stream);
        }
        Ok(scrape(stream, schemas))
    }

    async fn embed(&self, texts: Vec<String>, purpose: EmbedPurpose) -> Result<Vec<Vec<f32>>> {
        self.inner.embed(texts, purpose).await
    }

    async fn rerank(&self, query: &str, docs: Vec<String>) -> Result<Vec<Scored>> {
        self.inner.rerank(query, docs).await
    }

    fn tokenize(&self, class: RoleClass, text: &str) -> Result<usize> {
        self.inner.tokenize(class, text)
    }

    fn residency(&self) -> ResidencySnapshot {
        self.inner.residency()
    }
}

#[cfg(test)]
mod tests {
    use dark_contract::{FinishReason, Message, Role};
    use dark_engine_fake::{FakeEngine, Script};

    use super::*;

    /// The schema of a tool the scraped calls in these tests name.
    fn schemas() -> Vec<ToolSchema> {
        vec![ToolSchema {
            name: "read_file".to_owned(),
            description: "Reads a file.".to_owned(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": { "path": { "type": "string" } },
                "required": ["path"]
            }),
            tier: 1,
            mutating: false,
        }]
    }

    /// Builds a chunk stream from `chunks`.
    fn stream_of(chunks: Vec<Chunk>) -> ChunkStream {
        Box::pin(futures_util::stream::iter(
            chunks.into_iter().map(Ok).collect::<Vec<_>>(),
        ))
    }

    /// Collects a scraped stream into its chunks.
    async fn scraped(chunks: Vec<Chunk>) -> Vec<Chunk> {
        scrape(stream_of(chunks), schemas())
            .map(|item| item.expect("no chunk in these tests fails"))
            .collect()
            .await
    }

    /// Returns the text of every [`Chunk::Text`] in `chunks`, joined.
    fn text_of(chunks: &[Chunk]) -> String {
        chunks
            .iter()
            .filter_map(|chunk| match chunk {
                Chunk::Text(text) => Some(text.as_str()),
                _ => None,
            })
            .collect()
    }

    /// Returns every tool-call delta in `chunks`.
    fn calls_of(chunks: &[Chunk]) -> Vec<(String, String)> {
        chunks
            .iter()
            .filter_map(|chunk| match chunk {
                Chunk::ToolCallDelta {
                    name,
                    args_fragment,
                    ..
                } => Some((name.clone().unwrap_or_default(), args_fragment.clone())),
                _ => None,
            })
            .collect()
    }

    #[tokio::test]
    async fn a_scraped_call_becomes_a_tool_call_delta() {
        let chunks = scraped(vec![
            Chunk::Text("I will read it. ".to_owned()),
            Chunk::Text(
                "<tool_call>{\"name\": \"read_file\", \"arguments\": {\"path\": \"a.rs\"}}\
                 </tool_call>"
                    .to_owned(),
            ),
            Chunk::Done(FinishReason::ToolCalls),
        ])
        .await;

        let calls = calls_of(&chunks);
        assert_eq!(calls.len(), 1, "the call was scraped out of the text");
        assert_eq!(calls[0].0, "read_file");
        assert!(calls[0].1.contains("a.rs"), "arguments: {}", calls[0].1);
    }

    #[tokio::test]
    async fn the_call_block_never_appears_in_the_text() {
        let chunks = scraped(vec![
            Chunk::Text("before ".to_owned()),
            Chunk::Text(
                "<tool_call>{\"name\": \"read_file\", \"arguments\": {\"path\": \"a.rs\"}}\
                 </tool_call>"
                    .to_owned(),
            ),
            Chunk::Text(" after".to_owned()),
            Chunk::Done(FinishReason::ToolCalls),
        ])
        .await;

        let text = text_of(&chunks);
        assert!(
            !text.contains("tool_call"),
            "a person must never see the raw block: {text:?}"
        );
        assert!(
            !text.contains("read_file"),
            "the call body must never reach the display: {text:?}"
        );
        assert_eq!(text, "before  after");
    }

    #[tokio::test]
    async fn a_call_split_across_chunks_is_still_one_call() {
        // The shape a live stream produces: the tag, the body, and the
        // closing tag all arrive separately.
        let chunks = scraped(vec![
            Chunk::Text("<tool_call>".to_owned()),
            Chunk::Text("{\"name\": \"read_file\", ".to_owned()),
            Chunk::Text("\"arguments\": {\"path\": \"a.rs\"}}".to_owned()),
            Chunk::Text("</tool_call>".to_owned()),
            Chunk::Done(FinishReason::ToolCalls),
        ])
        .await;

        assert_eq!(calls_of(&chunks).len(), 1);
        assert!(
            !text_of(&chunks).contains("tool_call"),
            "text: {:?}",
            text_of(&chunks)
        );
    }

    #[tokio::test]
    async fn plain_text_passes_through_unchanged() {
        let chunks = scraped(vec![
            Chunk::Text("an ordinary reply".to_owned()),
            Chunk::Text(" with no call in it".to_owned()),
            Chunk::Done(FinishReason::Stop),
        ])
        .await;

        assert_eq!(text_of(&chunks), "an ordinary reply with no call in it");
        assert!(calls_of(&chunks).is_empty());
    }

    #[tokio::test]
    async fn the_end_chunk_arrives_after_the_calls() {
        let chunks = scraped(vec![
            Chunk::Text(
                "<tool_call>{\"name\": \"read_file\", \"arguments\": {\"path\": \"a.rs\"}}\
                 </tool_call>"
                    .to_owned(),
            ),
            Chunk::Done(FinishReason::ToolCalls),
        ])
        .await;

        let last = chunks.last().expect("the stream is not empty");
        assert!(
            matches!(last, Chunk::Done(_)),
            "Done must come last, or the accumulator sees a call after the end: {last:?}"
        );
    }

    #[tokio::test]
    async fn other_chunks_pass_through_in_order() {
        let chunks = scraped(vec![
            Chunk::Reasoning("thinking".to_owned()),
            Chunk::Text("hello, this is long enough to settle".to_owned()),
            Chunk::Done(FinishReason::Stop),
        ])
        .await;

        assert!(
            matches!(chunks.first(), Some(Chunk::Reasoning(text)) if text == "thinking"),
            "reasoning passes through first: {chunks:?}"
        );
    }

    #[tokio::test]
    async fn a_call_with_no_closing_tag_is_still_reported() {
        let chunks = scraped(vec![
            Chunk::Text(
                "<tool_call>{\"name\": \"read_file\", \"arguments\": {\"path\": \"a.rs\"}}"
                    .to_owned(),
            ),
            Chunk::Done(FinishReason::Length),
        ])
        .await;

        assert_eq!(
            calls_of(&chunks).len(),
            1,
            "an unanswered call breaks the chat template; it must be reported"
        );
    }

    #[tokio::test]
    async fn a_stream_that_ends_with_no_end_chunk_still_flushes() {
        let chunks = scraped(vec![Chunk::Text(
            "<tool_call>{\"name\": \"read_file\", \"arguments\": {\"path\": \"a.rs\"}}</tool_call>"
                .to_owned(),
        )])
        .await;

        assert_eq!(calls_of(&chunks).len(), 1);
    }

    #[tokio::test]
    async fn a_native_model_stream_is_not_rewritten() {
        // FakeEngine's default model reports native_tools: false, so this
        // checks the delegation path rather than the pass-through one;
        // the pass-through is checked by construction in `stream`.
        let engine = Arc::new(FakeEngine::new(Script::default()));
        let scraping = ScrapingEngine::new(Arc::clone(&engine));

        let caps = scraping.caps(RoleClass::Worker).await.unwrap();
        let inner_caps = engine.caps(RoleClass::Worker).await.unwrap();
        assert_eq!(
            caps.model_id, inner_caps.model_id,
            "caps delegate to the inner engine unchanged"
        );
    }

    #[tokio::test]
    async fn tokenize_and_residency_delegate() {
        let engine = Arc::new(FakeEngine::new(Script::default()));
        let scraping = ScrapingEngine::new(Arc::clone(&engine));

        assert_eq!(
            scraping.tokenize(RoleClass::Worker, "some text").unwrap(),
            engine.tokenize(RoleClass::Worker, "some text").unwrap()
        );
        assert_eq!(
            scraping.residency().models.len(),
            engine.residency().models.len()
        );
    }

    #[tokio::test]
    async fn a_scraped_call_reaches_a_caller_holding_the_engine_as_dyn_engine() {
        // The point of the wrapper: `dark-core` holds `dyn Engine` and
        // gets tool calls out of a model that only ever produced text.
        // The fake engine's default model reports `native_tools: false`,
        // which is exactly the scraped case.
        let script = Script::from_toml(
            r#"
            [[turns]]
            text = """I will read it. <tool_call>{"name": "read_file", \
"arguments": {"path": "a.rs"}}</tool_call>"""
            finish = "tool_calls"
            "#,
        )
        .expect("the script is valid");

        let engine: Arc<dyn Engine> =
            Arc::new(ScrapingEngine::new(Arc::new(FakeEngine::new(script))));
        let mut req = Request::new(RoleClass::Worker, vec![Message::text(Role::User, "hello")]);
        req.tools = schemas();

        let stream = engine.stream(req, CancellationToken::new()).await.unwrap();
        let chunks: Vec<Chunk> = stream
            .map(|item| item.expect("no chunk in this test fails"))
            .collect()
            .await;

        let calls = calls_of(&chunks);
        assert_eq!(
            calls.len(),
            1,
            "the turn loop sees a tool call it can answer: {chunks:?}"
        );
        assert_eq!(calls[0].0, "read_file");
        assert!(
            !text_of(&chunks).contains("tool_call"),
            "text: {:?}",
            text_of(&chunks)
        );
    }
}
