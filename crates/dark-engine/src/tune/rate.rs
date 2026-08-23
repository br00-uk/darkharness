//! Measures the generation rate (task unit `B6`, step 3).
//!
//! [`measure`] takes `&dyn Engine`, not a concrete engine, so a test drives
//! it with `dark-engine-fake` (a dev-dependency, which Rule 17 allows) and
//! asserts the arithmetic with no model and no accelerator.

use futures_util::StreamExt;
use tokio_util::sync::CancellationToken;

use dark_contract::{Chunk, Engine, Message, Request, Result, Role, RoleClass};

/// Runs one short generation against `engine` and returns the measured
/// tokens-per-second rate.
///
/// Reads the completion token count from [`Chunk::Usage`], and times from
/// just before the request starts to the moment the stream ends
/// ([`Chunk::Done`] or the stream closing). Returns `0.0` when the engine
/// reports zero completion tokens or the measured time is not positive —
/// Rule 9 asks `dark tune` to report a rate, and a caller should be able to
/// tell "measured zero" from "did not measure" without matching on an
/// error.
///
/// # Errors
///
/// Returns an error when starting the stream fails, or when a chunk in it
/// does.
pub async fn measure(engine: &dyn Engine, class: RoleClass, prompt: &str) -> Result<f32> {
    let request = Request::new(class, vec![Message::text(Role::User, prompt)]);
    let start = tokio::time::Instant::now();
    let mut stream = engine.stream(request, CancellationToken::new()).await?;

    let mut completion_tokens = 0usize;
    while let Some(chunk) = stream.next().await {
        match chunk? {
            Chunk::Usage(usage) => completion_tokens = usage.completion_tokens,
            Chunk::Done(_) => break,
            _ => {}
        }
    }
    let elapsed = tokio::time::Instant::now()
        .duration_since(start)
        .as_secs_f32();

    if completion_tokens == 0 || elapsed <= 0.0 {
        return Ok(0.0);
    }
    #[allow(clippy::cast_precision_loss)]
    Ok(completion_tokens as f32 / elapsed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use dark_engine_fake::{FakeEngine, script};

    #[tokio::test(start_paused = true)]
    async fn measures_tokens_per_second_from_a_scripted_reply() {
        // dark-engine-fake's stream delays every item it yields by
        // `token_delay_ms`, text tokens included, and it always appends
        // one Usage chunk and one Done chunk after the text. Ten words is
        // ten text-token items, so this reply yields 12 items total; at
        // 100 ms per item that is 1200 ms of virtual time by the time
        // Done arrives, for 10 completion tokens: 10 / 1.2 ≈ 8.33 tok/s.
        let script = script::Script {
            turns: vec![script::Turn {
                text: "one two three four five six seven eight nine ten".to_owned(),
                ..script::Turn::default()
            }],
            token_delay_ms: 100,
            ..script::Script::default()
        };
        let engine = FakeEngine::new(script);
        let rate = measure(&engine, RoleClass::Worker, "count to ten")
            .await
            .unwrap();
        assert!(
            (rate - 8.333_333).abs() < 0.01,
            "expected about 8.33 tok/s, got {rate}"
        );
    }

    #[tokio::test]
    async fn a_reply_with_no_tokens_measures_zero() {
        let engine = FakeEngine::with_replies([""]);
        let rate = measure(&engine, RoleClass::Worker, "say nothing")
            .await
            .unwrap();
        assert!(rate.abs() < f32::EPSILON);
    }
}
