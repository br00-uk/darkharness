//! The engine interface.
//!
//! `dark-engine` implements [`Engine`] over mistral.rs. `dark-engine-fake`
//! implements it with scripted responses. Every other crate depends on the
//! trait, never on an implementation.

use async_trait::async_trait;
use futures_core::stream::BoxStream;
use serde::{Deserialize, Serialize};

use crate::{Message, Result, ToolSchema};

/// The purpose that a model serves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RoleClass {
    /// Charts maps and makes design decisions.
    Architect,
    /// Does the coding work.
    Worker,
    /// Runs cheap bounded jobs, such as compaction and classification.
    Scout,
    /// Produces embedding vectors. The resident set manager pins this class.
    Embed,
    /// Scores documents against a query.
    Rerank,
}

impl RoleClass {
    /// Returns the lowercase name, for example `architect`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Architect => "architect",
            Self::Worker => "worker",
            Self::Scout => "scout",
            Self::Embed => "embed",
            Self::Rerank => "rerank",
        }
    }
}

impl std::fmt::Display for RoleClass {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Whether the model thinks before it answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThinkMode {
    /// The harness decides for each turn. See task unit `I2`.
    #[default]
    Auto,
    /// Always think.
    On,
    /// Never think.
    Off,
}

/// Which side of an asymmetric embedding model to use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EmbedPurpose {
    /// The text is a search query.
    Query,
    /// The text is a document to store.
    Document,
}

/// The device that runs a model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Device {
    /// The central processor.
    Cpu,
    /// An NVIDIA graphics processor.
    Cuda {
        /// The device index.
        index: usize,
    },
    /// Apple Silicon.
    Metal,
}

/// How the harness chooses tool calls for one request.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ToolChoice {
    /// The model decides.
    #[default]
    Auto,
    /// The model must not call a tool.
    None,
    /// The model must call some tool.
    Required,
    /// The model must call this tool.
    Named(String),
}

/// A constraint on the shape of the output.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Grammar {
    /// The output must match this JSON schema.
    JsonSchema(serde_json::Value),
    /// The output must match this regular expression.
    Regex(String),
    /// The output must match this Lark grammar.
    Lark(String),
}

/// The sampling settings for one request.
///
/// A `None` field means the engine uses the model default.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct Sampling {
    /// Higher values make the output more varied.
    pub temperature: Option<f32>,
    /// Nucleus sampling threshold.
    pub top_p: Option<f32>,
    /// Keep only this many candidate tokens.
    pub top_k: Option<usize>,
    /// Drop tokens below this fraction of the most likely token.
    pub min_p: Option<f32>,
    /// Penalise tokens that already appeared.
    pub presence_penalty: Option<f32>,
    /// Penalise repetition.
    pub repetition_penalty: Option<f32>,
    /// Fix the sampling seed.
    pub seed: Option<u64>,
}

/// One generation request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Request {
    /// Which model to use.
    pub class: RoleClass,
    /// The conversation.
    pub messages: Vec<Message>,
    /// The tools that the model may call.
    pub tools: Vec<ToolSchema>,
    /// How the model chooses a tool.
    pub tool_choice: ToolChoice,
    /// The sampling settings.
    pub sampling: Sampling,
    /// Whether the model thinks.
    pub think: ThinkMode,
    /// The generation limit.
    pub max_tokens: usize,
    /// Stop the generation at any of these strings.
    pub stop: Vec<String>,
    /// Constrain the output shape.
    pub grammar: Option<Grammar>,
    /// Ask for reproducible output. See task unit `B7`.
    pub deterministic: bool,
}

impl Request {
    /// Creates a request with the defaults that most callers want.
    pub fn new(class: RoleClass, messages: Vec<Message>) -> Self {
        Self {
            class,
            messages,
            tools: Vec::new(),
            tool_choice: ToolChoice::Auto,
            sampling: Sampling::default(),
            think: ThinkMode::Auto,
            max_tokens: 2048,
            stop: Vec::new(),
            grammar: None,
            deterministic: false,
        }
    }
}

/// Why a generation stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FinishReason {
    /// The model stopped on its own or hit a stop string.
    Stop,
    /// The generation reached `max_tokens`.
    Length,
    /// The model asked for one or more tool calls.
    ToolCalls,
    /// The caller cancelled the request.
    Cancelled,
    /// The generation failed.
    Error,
}

/// Token counts for one request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Usage {
    /// Tokens in the request.
    pub prompt_tokens: usize,
    /// Tokens that the model generated, thinking included.
    pub completion_tokens: usize,
    /// The part of `completion_tokens` that was thinking.
    pub reasoning_tokens: usize,
    /// Prompt tokens that the engine served from its cache.
    pub cached_tokens: usize,
}

impl Usage {
    /// Returns the total token count.
    pub fn total(&self) -> usize {
        self.prompt_tokens + self.completion_tokens
    }
}

/// One piece of a generation stream.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Chunk {
    /// Visible output.
    Text(String),
    /// Thinking output.
    Reasoning(String),
    /// Part of a tool call. The engine may split one call across many chunks.
    ToolCallDelta {
        /// Which call this fragment belongs to.
        index: usize,
        /// The call identifier, when the engine has produced it.
        id: Option<String>,
        /// The tool name, when the engine has produced it.
        name: Option<String>,
        /// The next part of the JSON argument text.
        args_fragment: String,
    },
    /// The token counts for the request.
    Usage(Usage),
    /// A model load is in progress.
    ModelLoading {
        /// The model that is loading.
        model: String,
        /// Progress between 0.0 and 1.0.
        progress: f32,
    },
    /// The stream ended.
    Done(FinishReason),
}

/// One scored document from [`Engine::rerank`].
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Scored {
    /// The position of the document in the input list.
    pub index: usize,
    /// The score. Higher is more relevant.
    pub score: f32,
}

/// What a loaded model can do.
///
/// The boolean fields are a capability flag set, not hidden state: a caller
/// reads one flag to decide whether a feature is available. Replacing them
/// with enums would make every call site longer and clearer about nothing.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Caps {
    /// The model identifier, for example `Qwen/Qwen3-14B`.
    pub model_id: String,
    /// The context length that the model supports.
    pub max_context: usize,
    /// The context that the resident set manager grants now.
    ///
    /// A caller budgets against this field, never against `max_context`.
    /// See Rule 4.
    pub granted_context: usize,
    /// The engine parses tool calls itself.
    pub native_tools: bool,
    /// The model supports a thinking mode.
    pub thinking: bool,
    /// The engine supports grammar-constrained decoding.
    pub grammar: bool,
    /// The model accepts images.
    pub vision: bool,
    /// The engine returns log probabilities. [`Engine::rerank`] needs this.
    pub logprobs: bool,
    /// The parameter count in billions.
    pub params_b: f32,
    /// The quantisation name, for example `q4k`.
    pub quant: String,
    /// The device that runs this model.
    pub device: Device,
    /// The measured generation rate, when `dark tune` has run.
    pub measured_tok_s: Option<f32>,
}

/// The state of one slot in the resident set.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum SlotState {
    /// The model is in memory and ready.
    Loaded,
    /// The model is loading now.
    Loading {
        /// Progress between 0.0 and 1.0.
        progress: f32,
    },
    /// The manager removed the model from memory.
    Evicted,
}

/// One model in the resident set.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResidentModel {
    /// The model identifier.
    pub model_id: String,
    /// The role classes that this model serves.
    pub classes: Vec<RoleClass>,
    /// Whether the model is loaded, loading, or evicted.
    pub state: SlotState,
    /// The memory that this model uses.
    pub bytes: u64,
    /// A pinned model is never evicted. See Rule 2.
    pub pinned: bool,
    /// A leased model is running a turn and is never evicted. See Rule 3.
    pub leased: bool,
}

/// What is in memory now.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct ResidencySnapshot {
    /// The memory that the harness may use.
    pub budget_bytes: u64,
    /// The memory that the resident set uses now.
    pub used_bytes: u64,
    /// The models.
    pub models: Vec<ResidentModel>,
}

/// A stream of generation chunks.
pub type ChunkStream = BoxStream<'static, Result<Chunk>>;

/// Runs models.
///
/// Only `dark-engine` and `dark-engine-fake` implement this trait. See Rule 12.
#[async_trait]
pub trait Engine: Send + Sync + 'static {
    /// Returns what the model for `class` can do.
    ///
    /// # Errors
    ///
    /// Returns an error when no model serves `class`, or when a load fails.
    async fn caps(&self, class: RoleClass) -> Result<Caps>;

    /// Starts a generation.
    ///
    /// The token cancels the request. A dropped stream also cancels it. The
    /// engine releases the key-value cache block on cancellation.
    ///
    /// # Errors
    ///
    /// Returns an error when the model does not fit, when a load fails, or
    /// when the request is not valid for the model.
    async fn stream(
        &self,
        req: Request,
        cancel: tokio_util::sync::CancellationToken,
    ) -> Result<ChunkStream>;

    /// Produces one vector for each input text.
    ///
    /// # Errors
    ///
    /// Returns an error when the embedding model is absent or fails.
    async fn embed(&self, texts: Vec<String>, purpose: EmbedPurpose) -> Result<Vec<Vec<f32>>>;

    /// Scores each document against the query.
    ///
    /// # Errors
    ///
    /// Returns [`crate::ErrCode::EngineUnsupported`] when [`Caps::logprobs`]
    /// is false. See task unit `B5`.
    async fn rerank(&self, query: &str, docs: Vec<String>) -> Result<Vec<Scored>>;

    /// Counts the tokens in `text` for the model that serves `class`.
    ///
    /// # Errors
    ///
    /// Returns an error when no tokenizer is loaded for `class`.
    fn tokenize(&self, class: RoleClass, text: &str) -> Result<usize>;

    /// Returns what is in memory now.
    fn residency(&self) -> ResidencySnapshot;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_new_uses_safe_defaults() {
        let req = Request::new(RoleClass::Worker, vec![]);
        assert_eq!(req.tool_choice, ToolChoice::Auto);
        assert_eq!(req.think, ThinkMode::Auto);
        assert!(!req.deterministic);
        assert!(req.grammar.is_none());
    }

    #[test]
    fn usage_total_excludes_cached_tokens() {
        // cached_tokens is a subset of prompt_tokens, so adding it would
        // double count.
        let usage = Usage {
            prompt_tokens: 100,
            completion_tokens: 20,
            reasoning_tokens: 5,
            cached_tokens: 80,
        };
        assert_eq!(usage.total(), 120);
    }

    #[test]
    fn role_class_names_are_stable() {
        assert_eq!(RoleClass::Architect.to_string(), "architect");
        assert_eq!(RoleClass::Embed.as_str(), "embed");
    }

    #[test]
    fn think_mode_defaults_to_auto() {
        assert_eq!(ThinkMode::default(), ThinkMode::Auto);
    }

    #[test]
    fn the_trait_is_object_safe() {
        // Every crate holds the engine as `dyn Engine`, so this must compile.
        fn assert_object_safe(_: Option<&dyn Engine>) {}
        assert_object_safe(None);
    }
}
