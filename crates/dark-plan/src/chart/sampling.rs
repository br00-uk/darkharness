//! Sampling settings for one micro-role, and the helper that runs one
//! generation against `&dyn Engine`.
//!
//! `dark-qwen` owns the real micro-role table (`deliberate`, `extract`,
//! `classify`, `narrate`; see `dark_qwen::profile::MicroRoleConfig`), but
//! `dark-plan` must not depend on `dark-qwen` (Rule 17 keeps the model stack
//! reachable through `dyn Engine` only). [`MicroSampling`] mirrors
//! `MicroRoleConfig` field for field, so a caller that holds a
//! `dark_qwen::profile::MicroRoleConfig` builds one of these with a plain
//! struct literal instead of an `into()` conversion this crate cannot write.

use std::future::Future;
use std::pin::Pin;

use dark_contract::{
    Chunk, ChunkStream, Engine, ErrCode, Error, FinishReason, Message, Request, Result, RoleClass,
    Sampling, ThinkMode, Usage,
};
use tokio_util::sync::CancellationToken;

/// The sampling and thinking settings for one micro-role.
///
/// Mirrors `dark_qwen::profile::MicroRoleConfig`. See the module
/// documentation for why this crate defines its own copy rather than
/// importing that one.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MicroSampling {
    /// Whether this micro-role thinks before it answers.
    pub think: ThinkMode,
    /// The sampling temperature.
    pub temperature: f32,
    /// The nucleus sampling threshold.
    pub top_p: f32,
    /// Whether this micro-role constrains its output with a grammar.
    pub grammar: bool,
    /// The generation limit, when this micro-role sets one.
    pub max_tokens: Option<usize>,
}

impl MicroSampling {
    /// Returns the settings the build specification gives for `deliberate`:
    /// thinking on, temperature 0.6, no grammar. Stages 1 and 3 use this
    /// micro-role.
    #[must_use]
    pub fn deliberate() -> Self {
        Self {
            think: ThinkMode::On,
            temperature: 0.6,
            top_p: 0.95,
            grammar: false,
            max_tokens: None,
        }
    }

    /// Returns the settings the build specification gives for `extract`:
    /// thinking off, grammar on. Stage 4 uses this micro-role.
    #[must_use]
    pub fn extract() -> Self {
        Self {
            think: ThinkMode::Off,
            temperature: 0.2,
            top_p: 0.8,
            grammar: true,
            max_tokens: Some(1200),
        }
    }

    /// Returns the settings the build specification gives for `classify`:
    /// thinking off, temperature 0, grammar on, one token. Stages 5 to 7 use
    /// this micro-role.
    #[must_use]
    pub fn classify() -> Self {
        Self {
            think: ThinkMode::Off,
            temperature: 0.0,
            top_p: 0.8,
            grammar: true,
            max_tokens: Some(64),
        }
    }
}

/// The `deliberate`, `extract`, and `classify` settings that the charting
/// pipeline needs. Stage 1 and stage 3 use `deliberate`. Stage 4 uses
/// `extract`. Stages 5 to 7 use `classify`.
///
/// The caller builds this from the resolved `dark-qwen` profile and passes
/// it in; see the module documentation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StageSampling {
    /// The `deliberate` micro-role: reasons about a plan.
    pub deliberate: MicroSampling,
    /// The `extract` micro-role: pulls structure out of a conversation.
    pub extract: MicroSampling,
    /// The `classify` micro-role: chooses one label from a fixed set.
    pub classify: MicroSampling,
}

impl Default for StageSampling {
    fn default() -> Self {
        Self {
            deliberate: MicroSampling::deliberate(),
            extract: MicroSampling::extract(),
            classify: MicroSampling::classify(),
        }
    }
}

/// What one finished generation produced.
#[derive(Debug, Clone, PartialEq)]
pub struct Generation {
    /// The visible text.
    pub text: String,
    /// The thinking text, when the micro-role thinks and the model emitted
    /// any.
    pub reasoning: Option<String>,
    /// The token counts the engine reported.
    pub usage: Usage,
    /// Why the generation stopped.
    pub finish: FinishReason,
}

/// Builds the [`Request`] for one micro-role call.
///
/// No tools and no conversation history beyond `messages`: every charting
/// stage starts a fresh sub-session (Do step 2 of task unit `E1`), so the
/// caller passes exactly the messages this one call should see, nothing
/// carried over from an earlier stage.
///
/// `sampling.grammar` says whether this micro-role expects
/// grammar-constrained decoding; it does not by itself pick a grammar. A
/// stage that needs one sets [`Request::grammar`] on the value this
/// function returns.
#[must_use]
pub fn build_request(class: RoleClass, messages: Vec<Message>, sampling: MicroSampling) -> Request {
    Request {
        think: sampling.think,
        sampling: Sampling {
            temperature: Some(sampling.temperature),
            top_p: Some(sampling.top_p),
            ..Sampling::default()
        },
        max_tokens: sampling.max_tokens.unwrap_or(2048),
        ..Request::new(class, messages)
    }
}

/// Runs one generation and folds its chunks into a [`Generation`].
///
/// This is the whole conversation driver a charting stage needs: one
/// request, one accumulated reply, no tool calls. `dark-plan` does not
/// depend on `dark-core`'s turn loop (`dark-core` depends downwards on
/// `dark-plan`, not the reverse — see the architecture diagram in
/// `CLAUDE.md`), so this function is the charting pipeline's own, narrower
/// substitute: it never issues a tool call and never loops.
///
/// # Errors
///
/// Returns an error when the engine fails to start the stream, or fails
/// partway through it.
pub async fn run_generation(engine: &dyn Engine, request: Request) -> Result<Generation> {
    let mut stream = engine.stream(request, CancellationToken::new()).await?;

    let mut text = String::new();
    let mut reasoning = String::new();
    let mut has_reasoning = false;
    let mut usage = Usage::default();
    let mut finish = FinishReason::Stop;

    while let Some(chunk) = next_chunk(&mut stream).await {
        match chunk? {
            Chunk::Text(part) => text.push_str(&part),
            Chunk::Reasoning(part) => {
                has_reasoning = true;
                reasoning.push_str(&part);
            }
            Chunk::Usage(reported) => usage = reported,
            Chunk::Done(reason) => {
                finish = reason;
                break;
            }
            Chunk::ToolCallDelta { .. } | Chunk::ModelLoading { .. } => {}
        }
    }

    if finish == FinishReason::Error {
        return Err(Error::new(
            ErrCode::EngineGenerate,
            "the engine ended the stream with an error and no Err chunk",
        ));
    }

    Ok(Generation {
        text,
        reasoning: has_reasoning.then_some(reasoning),
        usage,
        finish,
    })
}

/// Pulls one item out of a boxed stream.
///
/// [`ChunkStream`] is a `Pin<Box<dyn Stream<...> + Send>>`
/// (`futures_core::stream::BoxStream`). `dark-plan` has no direct
/// dependency on `futures-core` or `futures-util` — `Engine::stream` is the
/// only way to run a generation (the engine is held as `&dyn Engine`, Rule
/// 17), and draining its stream still needs no such dependency: calling a
/// trait method on a value whose static type already spells out `dyn
/// Trait`, as `ChunkStream` does, resolves without importing that trait.
/// `std::future::poll_fn` (stable, `std`-only) turns the raw `poll_next`
/// call into an `.await`-able future.
async fn next_chunk(stream: &mut ChunkStream) -> Option<Result<Chunk>> {
    std::future::poll_fn(|cx| stream.as_mut().poll_next(cx)).await
}

/// A type alias for the boxed future a hand-rolled async trait method
/// returns.
///
/// The charting pipeline holds stages 4 to 7 as trait objects (see
/// `crate::chart::stages`), because those stages belong to task units `E3`
/// to `E6` and are not yet implemented. A trait object cannot host a native
/// `async fn` (it is not object-safe), so those traits return this boxed
/// future instead — the standard pattern from before `async fn` in traits
/// existed, and it needs nothing beyond `std`.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

#[cfg(test)]
mod tests {
    #![allow(clippy::float_cmp)]
    use super::*;
    use dark_contract::Message;
    use dark_engine_fake::FakeEngine;

    #[tokio::test]
    async fn run_generation_collects_text_and_reasoning() {
        let engine = FakeEngine::new(dark_engine_fake::Script {
            turns: vec![dark_engine_fake::script::Turn {
                text: "the destination is clear".to_owned(),
                reasoning: Some("thinking about scope".to_owned()),
                ..Default::default()
            }],
            ..Default::default()
        });

        let request = build_request(
            RoleClass::Architect,
            vec![Message::text(dark_contract::Role::User, "chart this")],
            MicroSampling::deliberate(),
        );
        let generation = run_generation(&engine, request).await.expect("generates");

        assert_eq!(generation.text, "the destination is clear");
        assert_eq!(
            generation.reasoning.as_deref(),
            Some("thinking about scope")
        );
        assert_eq!(generation.finish, FinishReason::Stop);
    }

    #[tokio::test]
    async fn run_generation_propagates_an_engine_error() {
        let engine = FakeEngine::new(dark_engine_fake::Script {
            turns: vec![dark_engine_fake::script::Turn {
                text: "partial".to_owned(),
                error: Some(dark_engine_fake::script::ScriptedError {
                    code: "E_ENGINE_GENERATE".to_owned(),
                    message: "the model crashed".to_owned(),
                    after_chunks: 0,
                }),
                ..Default::default()
            }],
            ..Default::default()
        });

        let request = build_request(
            RoleClass::Architect,
            vec![Message::text(dark_contract::Role::User, "chart this")],
            MicroSampling::deliberate(),
        );
        let err = run_generation(&engine, request)
            .await
            .expect_err("the injected error must surface");
        assert_eq!(err.code, ErrCode::EngineGenerate);
    }

    #[test]
    fn built_in_micro_role_settings_match_the_build_specification() {
        let deliberate = MicroSampling::deliberate();
        assert_eq!(deliberate.think, ThinkMode::On);
        assert_eq!(deliberate.temperature, 0.6);
        assert!(!deliberate.grammar);

        let extract = MicroSampling::extract();
        assert_eq!(extract.think, ThinkMode::Off);
        assert!(extract.grammar);
        assert_eq!(extract.max_tokens, Some(1200));

        let classify = MicroSampling::classify();
        assert_eq!(classify.temperature, 0.0);
        assert_eq!(classify.max_tokens, Some(64));
    }
}
