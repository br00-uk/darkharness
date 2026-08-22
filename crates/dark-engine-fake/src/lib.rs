//! A scripted engine for tests and interface development.
//!
//! [`FakeEngine`] implements [`Engine`] without loading a model. Seven task
//! units test against it, so it stays cheap to build and free of native
//! dependencies.
//!
//! ```
//! use dark_contract::{Engine, Message, Request, Role, RoleClass};
//! use dark_engine_fake::FakeEngine;
//! use futures_util::StreamExt;
//!
//! # #[tokio::main(flavor = "current_thread")]
//! # async fn main() {
//! let engine = FakeEngine::with_replies(["Hello from the fake engine."]);
//! let request = Request::new(RoleClass::Worker, vec![Message::text(Role::User, "hi")]);
//!
//! let mut stream = engine
//!     .stream(request, tokio_util::sync::CancellationToken::new())
//!     .await
//!     .unwrap();
//!
//! let mut text = String::new();
//! while let Some(Ok(chunk)) = stream.next().await {
//!     if let dark_contract::Chunk::Text(part) = chunk {
//!         text.push_str(&part);
//!     }
//! }
//! assert_eq!(text, "Hello from the fake engine.");
//! # }
//! ```

mod embed;
pub mod script;

use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use dark_contract::{
    Caps, Chunk, ChunkStream, Device, EmbedPurpose, Engine, ErrCode, Error, FinishReason, Request,
    ResidencySnapshot, Result, RoleClass, Scored, ToolCall,
};
use futures_util::StreamExt;
use tokio_util::sync::CancellationToken;

pub use script::Script;

/// An engine that plays scripted responses.
#[derive(Debug)]
pub struct FakeEngine {
    script: Script,
    /// Which turn plays next.
    cursor: AtomicUsize,
    /// Every request the engine received, in order.
    ///
    /// Tests assert on this. Task unit `I2` uses it to prove that no outbound
    /// request carries a reasoning field.
    seen: Mutex<Vec<Request>>,
}

impl FakeEngine {
    /// Creates an engine from a script.
    pub fn new(script: Script) -> Self {
        Self {
            script,
            cursor: AtomicUsize::new(0),
            seen: Mutex::new(Vec::new()),
        }
    }

    /// Creates an engine from TOML text.
    ///
    /// # Errors
    ///
    /// Returns [`ErrCode::EngineLoad`] when the text does not parse.
    pub fn from_toml(text: &str) -> Result<Self> {
        Ok(Self::new(Script::from_toml(text)?))
    }

    /// Creates an engine from a TOML file.
    ///
    /// # Errors
    ///
    /// Returns [`ErrCode::EngineLoad`] when the file cannot be read or parsed.
    pub fn from_path(path: impl AsRef<std::path::Path>) -> Result<Self> {
        Ok(Self::new(Script::from_path(path)?))
    }

    /// Creates an engine that replies with each text in turn.
    ///
    /// This is the shortest way to write a test that needs no tool calls.
    pub fn with_replies<I, S>(replies: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let turns = replies
            .into_iter()
            .map(|text| script::Turn {
                text: text.into(),
                ..script::Turn::default()
            })
            .collect();
        Self::new(Script {
            turns,
            ..Script::default()
        })
    }

    /// Returns the script this engine plays.
    pub fn script(&self) -> &Script {
        &self.script
    }

    /// Returns every request the engine received, in order.
    ///
    /// # Panics
    ///
    /// Panics when another thread panicked while holding the lock.
    pub fn seen_requests(&self) -> Vec<Request> {
        self.seen
            .lock()
            .expect("the request log is poisoned")
            .clone()
    }

    /// Returns how many turns the engine has played.
    pub fn turns_played(&self) -> usize {
        self.cursor.load(Ordering::SeqCst)
    }

    /// Plays the script from the start again.
    pub fn rewind(&self) {
        self.cursor.store(0, Ordering::SeqCst);
    }

    /// Returns the capability defaults for a role class.
    ///
    /// A script overrides these. The worker default describes a 4B model with
    /// no log probabilities, so [`Engine::rerank`] fails unless a test asks
    /// for a larger model with [`FakeEngine::large_caps`].
    fn default_caps(class: RoleClass) -> Caps {
        Caps {
            model_id: format!("fake/qwen3-4b-{class}"),
            max_context: 32_768,
            granted_context: 32_768,
            native_tools: false,
            thinking: true,
            grammar: true,
            vision: false,
            logprobs: false,
            params_b: 4.0,
            quant: "q4k".to_owned(),
            device: Device::Cpu,
            measured_tok_s: Some(41.2),
        }
    }

    /// Returns capabilities that describe a 32B model.
    ///
    /// Use this to test the paths that need a large model: native tool
    /// parsing, log probabilities, and tier 3 tools.
    pub fn large_caps() -> Caps {
        Caps {
            model_id: "fake/qwen3-32b".to_owned(),
            params_b: 32.0,
            native_tools: true,
            logprobs: true,
            ..Self::default_caps(RoleClass::Worker)
        }
    }

    /// Builds the chunks for one turn, including any injected failure.
    fn plan(turn: &script::Turn, prompt_tokens: usize) -> Result<Vec<Result<Chunk>>> {
        let finish = turn.finish_reason()?;
        let mut items: Vec<Result<Chunk>> = Vec::new();

        if let Some(loading) = &turn.model_loading {
            let steps = loading.steps.max(1);
            for step in 1..=steps {
                #[allow(clippy::cast_precision_loss)]
                let progress = step as f32 / steps as f32;
                items.push(Ok(Chunk::ModelLoading {
                    model: loading.model.clone(),
                    progress,
                }));
            }
        }

        if let Some(reasoning) = &turn.reasoning {
            for token in split_tokens(reasoning) {
                items.push(Ok(Chunk::Reasoning(token)));
            }
        }

        for token in split_tokens(&turn.text) {
            items.push(Ok(Chunk::Text(token)));
        }

        for (index, call) in turn.tool_calls.iter().enumerate() {
            let args = toml_to_json(&call.args);
            items.push(Ok(Chunk::ToolCallDelta {
                index,
                id: Some(call.id.clone()),
                name: Some(call.name.clone()),
                args_fragment: args.to_string(),
            }));
        }

        items.push(Ok(Chunk::Usage(script::usage_for(
            prompt_tokens,
            &turn.text,
            turn.reasoning.as_deref(),
        ))));
        items.push(Ok(Chunk::Done(finish)));

        if let Some(error) = &turn.error {
            let at = error.after_chunks.min(items.len());
            items.truncate(at);
            items.push(Err(error.to_error()));
        }

        Ok(items)
    }
}

/// Splits text into tokens that concatenate back to the original.
///
/// Each token is one word and the whitespace that follows it, so a caller can
/// join the stream and compare it against the scripted text.
fn split_tokens(text: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut seen_word = false;

    for ch in text.chars() {
        if ch.is_whitespace() {
            current.push(ch);
        } else {
            if seen_word && current.ends_with(char::is_whitespace) {
                tokens.push(std::mem::take(&mut current));
            }
            current.push(ch);
            seen_word = true;
        }
    }

    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

/// Counts the tokens in a text.
pub(crate) fn token_count(text: &str) -> usize {
    split_tokens(text).len()
}

/// Converts a TOML value to the JSON value that a tool call carries.
fn toml_to_json(value: &toml::Value) -> serde_json::Value {
    match value {
        toml::Value::String(text) => serde_json::Value::String(text.clone()),
        toml::Value::Integer(number) => serde_json::Value::from(*number),
        toml::Value::Float(number) => serde_json::Value::from(*number),
        toml::Value::Boolean(flag) => serde_json::Value::Bool(*flag),
        toml::Value::Datetime(stamp) => serde_json::Value::String(stamp.to_string()),
        toml::Value::Array(items) => {
            serde_json::Value::Array(items.iter().map(toml_to_json).collect())
        }
        toml::Value::Table(table) => serde_json::Value::Object(
            table
                .iter()
                .map(|(key, item)| (key.clone(), toml_to_json(item)))
                .collect(),
        ),
    }
}

/// The state that the response stream carries.
struct StreamState {
    items: Vec<Result<Chunk>>,
    index: usize,
    cancel: CancellationToken,
    delay: Duration,
    finished: bool,
}

#[async_trait]
impl Engine for FakeEngine {
    async fn caps(&self, class: RoleClass) -> Result<Caps> {
        match self.script.caps_for(class) {
            Some(spec) => spec.to_caps(),
            None => Ok(Self::default_caps(class)),
        }
    }

    async fn stream(&self, req: Request, cancel: CancellationToken) -> Result<ChunkStream> {
        let prompt_tokens = req
            .messages
            .iter()
            .map(|message| token_count(&message.text_content()))
            .sum();

        self.seen
            .lock()
            .expect("the request log is poisoned")
            .push(req);

        let index = self.cursor.fetch_add(1, Ordering::SeqCst);
        let turn = self.script.turns.get(index).ok_or_else(|| {
            Error::new(
                ErrCode::EngineGenerate,
                format!(
                    "the script has {} turn(s) and the caller asked for turn {}",
                    self.script.turns.len(),
                    index + 1
                ),
            )
            .with_remedy("Add another [[turns]] entry to the script.")
        })?;

        let state = StreamState {
            items: Self::plan(turn, prompt_tokens)?,
            index: 0,
            cancel,
            delay: Duration::from_millis(self.script.token_delay_ms),
            finished: false,
        };

        let stream = futures_util::stream::unfold(state, |mut state| async move {
            if state.finished {
                return None;
            }

            // A cancelled turn still ends with a Done chunk, so a caller
            // always sees a terminator and can write its tool replies.
            if state.cancel.is_cancelled() {
                state.finished = true;
                return Some((Ok(Chunk::Done(FinishReason::Cancelled)), state));
            }

            if state.index >= state.items.len() {
                return None;
            }

            if !state.delay.is_zero() {
                tokio::select! {
                    () = tokio::time::sleep(state.delay) => {}
                    () = state.cancel.cancelled() => {
                        state.finished = true;
                        return Some((Ok(Chunk::Done(FinishReason::Cancelled)), state));
                    }
                }
            }

            let item = state.items[state.index].clone();
            state.index += 1;
            if item.is_err() {
                state.finished = true;
            }
            Some((item, state))
        });

        Ok(stream.boxed())
    }

    async fn embed(&self, texts: Vec<String>, purpose: EmbedPurpose) -> Result<Vec<Vec<f32>>> {
        Ok(texts
            .iter()
            .map(|text| embed::embed_text(&self.script.embed, text, purpose))
            .collect())
    }

    async fn rerank(&self, query: &str, docs: Vec<String>) -> Result<Vec<Scored>> {
        let caps = self.caps(RoleClass::Rerank).await?;
        if !caps.logprobs {
            return Err(Error::new(
                ErrCode::EngineUnsupported,
                "the rerank model returns no log probabilities",
            ));
        }

        let query_vector = embed::embed_text(&self.script.embed, query, EmbedPurpose::Query);
        let mut scored: Vec<Scored> = docs
            .iter()
            .enumerate()
            .map(|(index, doc)| {
                let vector = embed::embed_text(&self.script.embed, doc, EmbedPurpose::Document);
                Scored {
                    index,
                    score: embed::cosine(&query_vector, &vector),
                }
            })
            .collect();

        // Sort by score, then by index, so equal scores keep a stable order.
        scored.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(a.index.cmp(&b.index))
        });
        Ok(scored)
    }

    fn tokenize(&self, _class: RoleClass, text: &str) -> Result<usize> {
        Ok(token_count(text))
    }

    fn residency(&self) -> ResidencySnapshot {
        self.script.residency.as_ref().map_or_else(
            ResidencySnapshot::default,
            script::ResidencySpec::to_snapshot,
        )
    }
}

/// A tool call rebuilt from the chunks of a stream.
///
/// Callers accumulate [`Chunk::ToolCallDelta`] by index. This helper does the
/// same thing so a test does not have to.
///
/// # Errors
///
/// Returns [`ErrCode::ToolInvalidArgs`] when the accumulated argument text is
/// not valid JSON.
pub fn collect_tool_calls(chunks: &[Chunk]) -> Result<Vec<ToolCall>> {
    let mut parts: Vec<(Option<String>, Option<String>, String)> = Vec::new();

    for chunk in chunks {
        if let Chunk::ToolCallDelta {
            index,
            id,
            name,
            args_fragment,
        } = chunk
        {
            if parts.len() <= *index {
                parts.resize(*index + 1, (None, None, String::new()));
            }
            let entry = &mut parts[*index];
            if id.is_some() {
                entry.0.clone_from(id);
            }
            if name.is_some() {
                entry.1.clone_from(name);
            }
            entry.2.push_str(args_fragment);
        }
    }

    parts
        .into_iter()
        .map(|(id, name, args)| {
            let args = serde_json::from_str(&args).map_err(|err| {
                Error::new(
                    ErrCode::ToolInvalidArgs,
                    format!("bad tool arguments: {err}"),
                )
            })?;
            Ok(ToolCall {
                id: id.unwrap_or_default(),
                name: name.unwrap_or_default(),
                args,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_rejoin_into_the_original_text() {
        for text in [
            "hello world",
            "  leading and  double  spaces ",
            "one",
            "",
            "a\nb",
        ] {
            assert_eq!(split_tokens(text).concat(), text, "failed for {text:?}");
        }
    }

    #[test]
    fn token_count_counts_words() {
        assert_eq!(token_count("one two three"), 3);
        assert_eq!(token_count(""), 0);
        assert_eq!(token_count("   "), 1);
    }

    #[test]
    fn toml_arguments_become_json() {
        let value: toml::Value = toml::from_str("path = 'src/lib.rs'\nlimit = 20").unwrap();
        let json = toml_to_json(&value);
        assert_eq!(json["path"], "src/lib.rs");
        assert_eq!(json["limit"], 20);
    }
}
