//! Produces embedding vectors and reranks documents (task unit `B5`).
//!
//! [`prefix`] and [`batch`] hold the pure logic: which prefix a text gets,
//! and how input splits into batches. [`rerank`] holds the pure half of
//! reranking: building the fixed yes/no prompt and reading the affirmative
//! token's probability back out of a real [`mistralrs::Logprobs`] value.
//! Every one of those is tested directly, with no loaded model.
//!
//! [`embed_via_model`] and [`rerank_via_model`] are the live orchestration:
//! they call [`mistralrs::Model::generate_embeddings`] and
//! [`mistralrs::Model::send_chat_request`]. Like [`super::load::materialize`],
//! these two functions cannot run in this sandbox — no model is loaded here
//! — so they are compile-true against the real API but not exercised by a
//! test in this crate.

pub mod batch;
pub mod prefix;
pub mod rerank;

use dark_contract::{Caps, EmbedPurpose, ErrCode, Error, Result, Scored};

pub use batch::DEFAULT_BATCH_SIZE;
pub use prefix::PrefixConfig;

/// Produces one embedding vector for each of `texts`, using the pinned
/// embedding model (Rule 2), with the purpose-appropriate prefix applied
/// and the input batched (task unit `B5`, steps 1 to 3).
///
/// # Errors
///
/// Returns [`ErrCode::EngineGenerate`] when a batch fails to embed.
pub async fn embed_via_model(
    model: &mistralrs::Model,
    texts: &[String],
    purpose: EmbedPurpose,
    prefix_config: &PrefixConfig,
    batch_size: usize,
) -> Result<Vec<Vec<f32>>> {
    let mut vectors = Vec::with_capacity(texts.len());
    for group in batch::chunks(texts, batch_size) {
        let mut builder = mistralrs::EmbeddingRequestBuilder::new();
        for text in group {
            builder = builder.add_prompt(prefix::apply(text, purpose, prefix_config));
        }
        let batch_vectors = model
            .generate_embeddings(builder)
            .await
            .map_err(|source| engine_error(&format!("embedding batch failed: {source}")))?;
        vectors.extend(batch_vectors);
    }
    Ok(vectors)
}

/// Scores each of `docs` against `query` as single-token generation (task
/// unit `B5`, step 4): one chat request per document, `max_tokens = 1`,
/// reading the affirmative token's log probability out of the response.
/// This never runs a second embedding pass.
///
/// Returns scores sorted highest-first, ties broken by the original input
/// order — the same order [`dark_engine_fake`](https://docs.rs/dark-engine-fake)'s
/// scripted engine uses, so a caller does not see the ordering change
/// depending on which engine is behind it.
///
/// # Errors
///
/// Returns [`ErrCode::EngineUnsupported`] when `caps.logprobs` is `false`
/// (task unit `B5`, step 5). Returns [`ErrCode::EngineGenerate`] when a
/// scoring request fails.
pub async fn rerank_via_model(
    model: &mistralrs::Model,
    caps: &Caps,
    query: &str,
    docs: &[String],
) -> Result<Vec<Scored>> {
    /// How many alternative tokens to ask mistral.rs for alongside the
    /// generated one, so the affirmative token is very likely to appear
    /// even when it was not the model's top choice.
    const TOP_LOGPROBS: usize = 20;

    rerank::require_logprobs(caps)?;

    let mut scored = Vec::with_capacity(docs.len());
    for (index, doc) in docs.iter().enumerate() {
        let request = mistralrs::RequestBuilder::new()
            .add_message(mistralrs::TextMessageRole::User, rerank::prompt(query, doc))
            .set_sampler_max_len(1)
            .return_logprobs(true)
            .set_sampler_topn_logprobs(TOP_LOGPROBS);
        let response = model
            .send_chat_request(request)
            .await
            .map_err(|source| engine_error(&format!("rerank request failed: {source}")))?;
        let score = response
            .choices
            .first()
            .and_then(|choice| choice.logprobs.as_ref())
            .and_then(rerank::affirmative_probability)
            .unwrap_or(0.0);
        scored.push(Scored { index, score });
    }

    scored.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.index.cmp(&b.index))
    });
    Ok(scored)
}

/// Builds an `E_ENGINE_GENERATE` error with `message`.
fn engine_error(message: &str) -> Error {
    Error::new(ErrCode::EngineGenerate, message.to_owned())
}
