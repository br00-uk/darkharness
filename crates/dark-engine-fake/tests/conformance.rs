//! The engine conformance suite.
//!
//! These tests describe what any [`Engine`] implementation must do, not what
//! the fake one happens to do. Task unit `B2` and later should run the same
//! expectations against the real engine.

use std::time::Duration;

use dark_contract::{
    Caps, Chunk, EmbedPurpose, Engine, ErrCode, FinishReason, Message, Request, Role, RoleClass,
    SlotState,
};
use dark_engine_fake::{FakeEngine, Script, collect_tool_calls};
use futures_util::StreamExt;
use tokio_util::sync::CancellationToken;

/// Drains a stream into the chunks it produced and the first error it hit.
async fn drain(
    engine: &FakeEngine,
    request: Request,
    cancel: CancellationToken,
) -> (Vec<Chunk>, Option<dark_contract::Error>) {
    let mut stream = engine.stream(request, cancel).await.expect("stream starts");
    let mut chunks = Vec::new();
    let mut failure = None;
    while let Some(item) = stream.next().await {
        match item {
            Ok(chunk) => chunks.push(chunk),
            Err(err) => {
                failure = Some(err);
                break;
            }
        }
    }
    (chunks, failure)
}

fn ask(text: &str) -> Request {
    Request::new(RoleClass::Worker, vec![Message::text(Role::User, text)])
}

fn text_of(chunks: &[Chunk]) -> String {
    chunks
        .iter()
        .filter_map(|chunk| match chunk {
            Chunk::Text(part) => Some(part.as_str()),
            _ => None,
        })
        .collect()
}

fn reasoning_of(chunks: &[Chunk]) -> String {
    chunks
        .iter()
        .filter_map(|chunk| match chunk {
            Chunk::Reasoning(part) => Some(part.as_str()),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn a_stream_ends_with_exactly_one_done_chunk() {
    let engine = FakeEngine::with_replies(["all good"]);
    let (chunks, failure) = drain(&engine, ask("hi"), CancellationToken::new()).await;

    assert!(failure.is_none());
    let done: Vec<_> = chunks
        .iter()
        .filter(|c| matches!(c, Chunk::Done(_)))
        .collect();
    assert_eq!(done.len(), 1, "expected one Done chunk, got {done:?}");
    assert!(matches!(
        chunks.last(),
        Some(Chunk::Done(FinishReason::Stop))
    ));
}

#[tokio::test]
async fn streamed_text_rejoins_into_the_scripted_reply() {
    let engine = FakeEngine::with_replies(["The quick brown fox jumps."]);
    let (chunks, _) = drain(&engine, ask("hi"), CancellationToken::new()).await;
    assert_eq!(text_of(&chunks), "The quick brown fox jumps.");
}

#[tokio::test]
async fn text_arrives_one_token_at_a_time() {
    let engine = FakeEngine::with_replies(["one two three four"]);
    let (chunks, _) = drain(&engine, ask("hi"), CancellationToken::new()).await;
    let text_chunks = chunks
        .iter()
        .filter(|c| matches!(c, Chunk::Text(_)))
        .count();
    assert_eq!(text_chunks, 4, "expected one chunk for each word");
}

#[tokio::test]
async fn each_call_plays_the_next_turn() {
    let engine = FakeEngine::with_replies(["first", "second"]);

    let (first, _) = drain(&engine, ask("a"), CancellationToken::new()).await;
    let (second, _) = drain(&engine, ask("b"), CancellationToken::new()).await;

    assert_eq!(text_of(&first), "first");
    assert_eq!(text_of(&second), "second");
    assert_eq!(engine.turns_played(), 2);
}

#[tokio::test]
async fn running_past_the_script_reports_a_clear_error() {
    // A silent empty reply would look like a model failure. Say what is wrong.
    let engine = FakeEngine::with_replies(["only one"]);
    drain(&engine, ask("a"), CancellationToken::new()).await;

    // ChunkStream is not Debug, so match rather than using expect_err.
    let Err(err) = engine.stream(ask("b"), CancellationToken::new()).await else {
        panic!("the script is exhausted, so the call must fail");
    };
    assert_eq!(err.code, ErrCode::EngineGenerate);
    assert!(
        err.message.contains("turn 2"),
        "unhelpful message: {}",
        err.message
    );
}

#[tokio::test]
async fn reasoning_arrives_before_the_text() {
    let script = Script::from_toml(
        r#"
        [[turns]]
        reasoning = "let me think"
        text = "the answer"
        "#,
    )
    .expect("valid script");
    let engine = FakeEngine::new(script);

    let (chunks, _) = drain(&engine, ask("hi"), CancellationToken::new()).await;

    assert_eq!(reasoning_of(&chunks), "let me think");
    assert_eq!(text_of(&chunks), "the answer");

    let first_text = chunks
        .iter()
        .position(|c| matches!(c, Chunk::Text(_)))
        .unwrap();
    let last_reason = chunks
        .iter()
        .rposition(|c| matches!(c, Chunk::Reasoning(_)))
        .unwrap();
    assert!(last_reason < first_text, "reasoning must precede text");
}

#[tokio::test]
async fn an_injected_tool_call_arrives_and_rebuilds() {
    let script = Script::from_toml(
        r#"
        [[turns]]
        text = "reading the file"
        finish = "tool_calls"

        [[turns.tool_calls]]
        id = "call-1"
        name = "read_file"
        args = { path = "src/lib.rs", limit = 20 }
        "#,
    )
    .expect("valid script");
    let engine = FakeEngine::new(script);

    let (chunks, _) = drain(&engine, ask("read it"), CancellationToken::new()).await;

    assert!(matches!(
        chunks.last(),
        Some(Chunk::Done(FinishReason::ToolCalls))
    ));

    let calls = collect_tool_calls(&chunks).expect("the fragments rebuild");
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].id, "call-1");
    assert_eq!(calls[0].name, "read_file");
    assert_eq!(calls[0].args["path"], "src/lib.rs");
    assert_eq!(calls[0].args["limit"], 20);
}

#[tokio::test]
async fn an_injected_error_stops_the_stream() {
    let script = Script::from_toml(
        r#"
        [[turns]]
        text = "this never finishes"
        error = { code = "E_ENGINE_WONT_FIT", message = "needs 4 GB more", after_chunks = 1 }
        "#,
    )
    .expect("valid script");
    let engine = FakeEngine::new(script);

    let (chunks, failure) = drain(&engine, ask("hi"), CancellationToken::new()).await;

    let err = failure.expect("the stream must fail");
    assert_eq!(err.code, ErrCode::EngineWontFit);
    assert_eq!(err.message, "needs 4 GB more");
    assert_eq!(chunks.len(), 1, "the error arrives after one chunk");
    // The remedy from Appendix A is attached without the script naming it.
    assert!(err.remedy.is_some());
}

#[tokio::test]
async fn a_cancelled_stream_still_terminates_with_done() {
    // The turn loop writes a tool reply for every issued call, so it must
    // always see a terminator. See task unit A2.
    let script = Script::from_toml(
        r#"
        token_delay_ms = 50

        [[turns]]
        text = "one two three four five six seven eight"
        "#,
    )
    .expect("valid script");
    let engine = FakeEngine::new(script);

    let cancel = CancellationToken::new();
    let mut stream = engine
        .stream(ask("hi"), cancel.clone())
        .await
        .expect("stream starts");

    let cancel_after = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(60)).await;
        cancel_after.cancel();
    });

    let mut chunks = Vec::new();
    while let Some(Ok(chunk)) = stream.next().await {
        chunks.push(chunk);
    }

    assert!(
        matches!(chunks.last(), Some(Chunk::Done(FinishReason::Cancelled))),
        "expected a cancelled terminator, got {:?}",
        chunks.last()
    );
    assert!(
        text_of(&chunks).split_whitespace().count() < 8,
        "cancellation must cut the text short"
    );
}

#[tokio::test]
async fn cancelling_before_the_first_poll_yields_only_done() {
    let engine = FakeEngine::with_replies(["never seen"]);
    let cancel = CancellationToken::new();
    cancel.cancel();

    let (chunks, _) = drain(&engine, ask("hi"), cancel).await;
    assert_eq!(chunks.len(), 1);
    assert!(matches!(chunks[0], Chunk::Done(FinishReason::Cancelled)));
}

#[tokio::test]
async fn a_model_load_reports_progress_that_ends_at_one() {
    let script = Script::from_toml(
        r#"
        [[turns]]
        text = "ready"
        model_loading = { model = "fake/qwen3-4b", steps = 4 }
        "#,
    )
    .expect("valid script");
    let engine = FakeEngine::new(script);

    let (chunks, _) = drain(&engine, ask("hi"), CancellationToken::new()).await;

    let progress: Vec<f32> = chunks
        .iter()
        .filter_map(|c| match c {
            Chunk::ModelLoading { progress, .. } => Some(*progress),
            _ => None,
        })
        .collect();

    assert_eq!(progress.len(), 4);
    assert!(
        (progress[3] - 1.0).abs() < f32::EPSILON,
        "load must end at 1.0"
    );
    assert!(
        progress.windows(2).all(|w| w[0] < w[1]),
        "progress must rise"
    );
}

#[tokio::test]
async fn usage_counts_reasoning_inside_the_completion() {
    let script = Script::from_toml(
        r#"
        [[turns]]
        reasoning = "a b"
        text = "c d e"
        "#,
    )
    .expect("valid script");
    let engine = FakeEngine::new(script);

    let (chunks, _) = drain(&engine, ask("one two"), CancellationToken::new()).await;

    let usage = chunks
        .iter()
        .find_map(|c| match c {
            Chunk::Usage(usage) => Some(*usage),
            _ => None,
        })
        .expect("a stream reports usage");

    assert_eq!(usage.prompt_tokens, 2);
    assert_eq!(usage.reasoning_tokens, 2);
    assert_eq!(
        usage.completion_tokens, 5,
        "reasoning is part of the completion"
    );
}

#[tokio::test]
async fn caps_default_to_a_small_model_and_the_script_overrides_them() {
    let engine = FakeEngine::with_replies(["hi"]);
    let caps = engine.caps(RoleClass::Worker).await.expect("caps");
    assert!((caps.params_b - 4.0).abs() < f32::EPSILON);
    assert!(!caps.logprobs);

    let script = Script::from_toml(
        r#"
        [[caps]]
        class = "worker"
        model_id = "fake/qwen3-32b"
        max_context = 131072
        granted_context = 32768
        params_b = 32.0
        native_tools = true
        logprobs = true
        device = "cuda:0"
        "#,
    )
    .expect("valid script");
    let engine = FakeEngine::new(script);
    let caps = engine.caps(RoleClass::Worker).await.expect("caps");

    assert_eq!(caps.model_id, "fake/qwen3-32b");
    assert!((caps.params_b - 32.0).abs() < f32::EPSILON);
    assert!(caps.native_tools);
    assert_eq!(caps.device, dark_contract::Device::Cuda { index: 0 });
    // Rule 4: a caller budgets against the grant, not the maximum.
    assert!(caps.granted_context < caps.max_context);
}

#[tokio::test]
async fn the_large_preset_describes_a_32b_model() {
    let caps: Caps = FakeEngine::large_caps();
    assert!((caps.params_b - 32.0).abs() < f32::EPSILON);
    assert!(caps.logprobs, "a large model supports reranking");
    assert!(caps.native_tools);
}

#[tokio::test]
async fn embeddings_are_stable_and_reflect_shared_words() {
    let engine = FakeEngine::with_replies(Vec::<String>::new());

    let vectors = engine
        .embed(
            vec![
                "tokio runtime worker threads".to_owned(),
                "tokio runtime worker threads".to_owned(),
                "postgres replication lag".to_owned(),
            ],
            EmbedPurpose::Document,
        )
        .await
        .expect("embeddings");

    assert_eq!(vectors.len(), 3);
    assert_eq!(
        vectors[0], vectors[1],
        "the same text must embed identically"
    );
    assert_ne!(vectors[0], vectors[2]);
}

#[tokio::test]
async fn rerank_needs_log_probabilities() {
    // Reranking is single-token scoring, so a model without log probabilities
    // cannot do it. Fail rather than return a meaningless order.
    let engine = FakeEngine::with_replies(Vec::<String>::new());
    let err = engine
        .rerank("query", vec!["a".to_owned()])
        .await
        .expect_err("the default model has no log probabilities");
    assert_eq!(err.code, ErrCode::EngineUnsupported);
}

#[tokio::test]
async fn rerank_orders_by_relevance() {
    let script = Script::from_toml(
        r#"
        [[caps]]
        class = "rerank"
        model_id = "fake/reranker"
        max_context = 4096
        granted_context = 4096
        params_b = 0.6
        logprobs = true
        "#,
    )
    .expect("valid script");
    let engine = FakeEngine::new(script);

    let scored = engine
        .rerank(
            "tokio runtime worker threads",
            vec![
                "postgres replication and failover".to_owned(),
                "tokio runtime worker threads configuration".to_owned(),
                "unrelated notes about yaml".to_owned(),
            ],
        )
        .await
        .expect("rerank");

    assert_eq!(scored.len(), 3);
    assert_eq!(scored[0].index, 1, "the matching document must rank first");
    assert!(scored[0].score > scored[1].score);
}

#[tokio::test]
async fn tokenize_is_deterministic() {
    let engine = FakeEngine::with_replies(Vec::<String>::new());
    let count = engine
        .tokenize(RoleClass::Worker, "one two three")
        .expect("tokenize");
    assert_eq!(count, 3);
    assert_eq!(
        engine.tokenize(RoleClass::Worker, "one two three").unwrap(),
        count
    );
}

#[tokio::test]
async fn residency_reports_the_scripted_set() {
    let script = Script::from_toml(
        r#"
        [residency]
        budget_bytes = 25000000000
        used_bytes = 17000000000

        [[residency.models]]
        model_id = "fake/qwen3-embed"
        classes = ["embed"]
        state = "loaded"
        bytes = 1200000000
        pinned = true

        [[residency.models]]
        model_id = "fake/qwen3-14b"
        classes = ["architect", "worker", "scout"]
        state = "loading"
        progress = 0.5
        bytes = 15800000000
        leased = true
        "#,
    )
    .expect("valid script");
    let engine = FakeEngine::new(script);

    let snapshot = engine.residency();
    assert_eq!(snapshot.budget_bytes, 25_000_000_000);
    assert_eq!(snapshot.models.len(), 2);

    // Rule 2: the embedding model is pinned.
    let embed_model = &snapshot.models[0];
    assert!(embed_model.pinned);
    assert_eq!(embed_model.state, SlotState::Loaded);

    // Rule 3: a model that holds a turn lease is never evicted.
    let worker = &snapshot.models[1];
    assert!(worker.leased);
    assert_eq!(worker.state, SlotState::Loading { progress: 0.5 });
    // Rule 1: below 24 GB the three roles share one model.
    assert_eq!(worker.classes.len(), 3);
}

#[tokio::test]
async fn the_engine_records_every_request_it_received() {
    // Task unit I2 asserts that no outbound request carries reasoning.
    let engine = FakeEngine::with_replies(["a", "b"]);
    drain(&engine, ask("first"), CancellationToken::new()).await;
    drain(&engine, ask("second"), CancellationToken::new()).await;

    let seen = engine.seen_requests();
    assert_eq!(seen.len(), 2);
    assert_eq!(seen[0].messages[0].text_content(), "first");
    assert!(
        seen.iter()
            .all(|req| req.messages.iter().all(|m| m.reasoning.is_none()))
    );
}

#[tokio::test]
async fn rewind_replays_the_script() {
    let engine = FakeEngine::with_replies(["only"]);
    let (first, _) = drain(&engine, ask("a"), CancellationToken::new()).await;
    engine.rewind();
    let (again, _) = drain(&engine, ask("a"), CancellationToken::new()).await;
    assert_eq!(text_of(&first), text_of(&again));
}

#[tokio::test]
async fn the_engine_is_usable_behind_a_trait_object() {
    // Every crate except dark-cli holds the engine as `dyn Engine`. See Rule 17.
    let engine: Box<dyn Engine> = Box::new(FakeEngine::with_replies(["via dyn"]));
    let mut stream = engine
        .stream(ask("hi"), CancellationToken::new())
        .await
        .expect("stream starts");

    let mut chunks = Vec::new();
    while let Some(Ok(chunk)) = stream.next().await {
        chunks.push(chunk);
    }
    assert_eq!(text_of(&chunks), "via dyn");
}

#[tokio::test]
async fn a_script_loads_from_a_file_and_plays_through() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/session.toml");
    let engine = FakeEngine::from_path(&path).expect("the example script parses");

    let caps = engine.caps(RoleClass::Worker).await.expect("caps");
    assert_eq!(caps.model_id, "fake/qwen3-14b");
    assert_eq!(caps.device, dark_contract::Device::Metal);

    // Rule 2: the embedding model is pinned.
    let snapshot = engine.residency();
    assert!(
        snapshot
            .models
            .iter()
            .any(|m| m.pinned && m.classes == vec![RoleClass::Embed])
    );

    // Turn 1 thinks, then calls a tool.
    let (first, _) = drain(&engine, ask("read the manifest"), CancellationToken::new()).await;
    assert!(!reasoning_of(&first).is_empty());
    let calls = collect_tool_calls(&first).expect("the call rebuilds");
    assert_eq!(calls[0].name, "read_file");
    assert_eq!(calls[0].args["limit"], 200);

    // Turn 2 answers.
    let (second, _) = drain(&engine, ask("and?"), CancellationToken::new()).await;
    assert!(text_of(&second).contains("licence"));

    // Turn 3 fails before any chunk arrives.
    let (third, failure) = drain(&engine, ask("use the big model"), CancellationToken::new()).await;
    assert!(third.is_empty());
    assert_eq!(failure.expect("turn 3 fails").code, ErrCode::EngineWontFit);
}
