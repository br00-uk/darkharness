//! The scripted response format.
//!
//! One TOML file holds many turns. The engine plays them in order. A test
//! that needs no file builds a [`Script`] directly.

use dark_contract::{
    Caps, Device, ErrCode, Error, FinishReason, ResidencySnapshot, ResidentModel, Result,
    RoleClass, SlotState, Usage,
};
use serde::Deserialize;

/// A whole scripted session.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Script {
    /// What [`crate::FakeEngine::caps`] returns for each role class.
    #[serde(default)]
    pub caps: Vec<CapsSpec>,
    /// What [`crate::FakeEngine::residency`] returns.
    #[serde(default)]
    pub residency: Option<ResidencySpec>,
    /// The turns, in the order the engine plays them.
    #[serde(default)]
    pub turns: Vec<Turn>,
    /// How the engine builds fake vectors.
    #[serde(default)]
    pub embed: EmbedSpec,
    /// How long the engine waits between tokens, in milliseconds.
    ///
    /// The default is zero so a test suite runs fast. Set it to see the
    /// terminal application stream at a human pace.
    #[serde(default)]
    pub token_delay_ms: u64,
}

impl Script {
    /// Reads a script from TOML text.
    ///
    /// # Errors
    ///
    /// Returns [`ErrCode::EngineLoad`] when the text is not valid TOML or
    /// does not match the schema.
    pub fn from_toml(text: &str) -> Result<Self> {
        toml::from_str(text).map_err(|err| {
            Error::new(ErrCode::EngineLoad, format!("invalid script: {err}"))
                .with_remedy("Check the script against the Script type in dark-engine-fake.")
        })
    }

    /// Reads a script from a file.
    ///
    /// # Errors
    ///
    /// Returns [`ErrCode::EngineLoad`] when the file cannot be read or does
    /// not parse.
    pub fn from_path(path: impl AsRef<std::path::Path>) -> Result<Self> {
        let path = path.as_ref();
        let text = std::fs::read_to_string(path).map_err(|err| {
            Error::new(
                ErrCode::EngineLoad,
                format!("cannot read {}: {err}", path.display()),
            )
        })?;
        Self::from_toml(&text)
    }

    /// Returns the capability specification for `class`.
    pub fn caps_for(&self, class: RoleClass) -> Option<&CapsSpec> {
        self.caps.iter().find(|spec| spec.class == class)
    }
}

/// One scripted turn.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Turn {
    /// The visible output.
    #[serde(default)]
    pub text: String,
    /// The thinking output. The engine emits this before the text.
    #[serde(default)]
    pub reasoning: Option<String>,
    /// Tool calls to inject.
    #[serde(default)]
    pub tool_calls: Vec<ScriptedToolCall>,
    /// An error to inject instead of output.
    #[serde(default)]
    pub error: Option<ScriptedError>,
    /// A model load to report before the output.
    #[serde(default)]
    pub model_loading: Option<ModelLoadingSpec>,
    /// Why the turn stops. Defaults to `tool_calls` when this turn injects a
    /// call, and to `stop` otherwise.
    #[serde(default)]
    pub finish: Option<String>,
}

impl Turn {
    /// Returns the finish reason for this turn.
    ///
    /// # Errors
    ///
    /// Returns [`ErrCode::EngineLoad`] when `finish` names no known reason.
    pub fn finish_reason(&self) -> Result<FinishReason> {
        let Some(name) = self.finish.as_deref() else {
            return Ok(if self.tool_calls.is_empty() {
                FinishReason::Stop
            } else {
                FinishReason::ToolCalls
            });
        };

        match name {
            "stop" => Ok(FinishReason::Stop),
            "length" => Ok(FinishReason::Length),
            "tool_calls" => Ok(FinishReason::ToolCalls),
            "cancelled" => Ok(FinishReason::Cancelled),
            "error" => Ok(FinishReason::Error),
            other => Err(Error::new(
                ErrCode::EngineLoad,
                format!("unknown finish reason {other:?}"),
            )
            .with_remedy("Use stop, length, tool_calls, cancelled, or error.")),
        }
    }
}

/// A tool call to inject into a turn.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScriptedToolCall {
    /// The call identifier.
    pub id: String,
    /// The tool name.
    pub name: String,
    /// The arguments, as TOML. The engine converts them to JSON.
    #[serde(default = "empty_table")]
    pub args: toml::Value,
}

/// Returns an empty TOML table. `toml::Value` has no `Default`.
fn empty_table() -> toml::Value {
    toml::Value::Table(toml::map::Map::new())
}

/// An error to inject into a turn.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScriptedError {
    /// The code string, for example `E_ENGINE_WONT_FIT`.
    pub code: String,
    /// The message.
    pub message: String,
    /// How many chunks the engine emits before it fails.
    ///
    /// This lets a test fail a stream part way through rather than at once.
    #[serde(default)]
    pub after_chunks: usize,
}

impl ScriptedError {
    /// Builds the error that this specification describes.
    ///
    /// An unknown code becomes [`ErrCode::EngineGenerate`], so a script can
    /// still inject a failure without naming an exact code.
    pub fn to_error(&self) -> Error {
        Error::new(parse_code(&self.code), self.message.clone())
    }
}

/// Maps a code string back to an [`ErrCode`].
fn parse_code(text: &str) -> ErrCode {
    match text {
        "E_ENGINE_WONT_FIT" => ErrCode::EngineWontFit,
        "E_ENGINE_UNSUPPORTED" => ErrCode::EngineUnsupported,
        "E_ENGINE_LOAD" => ErrCode::EngineLoad,
        "E_ENGINE_CANCELLED" => ErrCode::EngineCancelled,
        _ => ErrCode::EngineGenerate,
    }
}

/// A model load to report.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelLoadingSpec {
    /// The model that loads.
    pub model: String,
    /// How many progress events the engine emits.
    #[serde(default = "default_load_steps")]
    pub steps: usize,
}

fn default_load_steps() -> usize {
    4
}

/// What [`crate::FakeEngine::caps`] returns for one role class.
///
/// The boolean fields mirror [`Caps`], which is a capability flag set rather
/// than hidden state. See the note on `Caps` in `dark-contract`.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CapsSpec {
    /// The role class this specification covers.
    pub class: RoleClass,
    /// The model identifier.
    pub model_id: String,
    /// The context length the model supports.
    pub max_context: usize,
    /// The context the resident set manager grants.
    pub granted_context: usize,
    /// The parameter count in billions.
    pub params_b: f32,
    /// The quantisation name.
    #[serde(default = "default_quant")]
    pub quant: String,
    /// `cpu`, `metal`, or `cuda:N`.
    #[serde(default = "default_device")]
    pub device: String,
    /// The engine parses tool calls itself.
    #[serde(default)]
    pub native_tools: bool,
    /// The model supports a thinking mode.
    #[serde(default)]
    pub thinking: bool,
    /// The engine supports grammar-constrained decoding.
    #[serde(default)]
    pub grammar: bool,
    /// The model accepts images.
    #[serde(default)]
    pub vision: bool,
    /// The engine returns log probabilities. Reranking needs this.
    #[serde(default)]
    pub logprobs: bool,
    /// The measured generation rate.
    #[serde(default)]
    pub measured_tok_s: Option<f32>,
}

fn default_quant() -> String {
    "q4k".to_owned()
}

fn default_device() -> String {
    "cpu".to_owned()
}

impl CapsSpec {
    /// Converts this specification into a [`Caps`].
    ///
    /// # Errors
    ///
    /// Returns [`ErrCode::EngineLoad`] when the device string is not valid.
    pub fn to_caps(&self) -> Result<Caps> {
        Ok(Caps {
            model_id: self.model_id.clone(),
            max_context: self.max_context,
            granted_context: self.granted_context,
            native_tools: self.native_tools,
            thinking: self.thinking,
            grammar: self.grammar,
            vision: self.vision,
            logprobs: self.logprobs,
            params_b: self.params_b,
            quant: self.quant.clone(),
            device: parse_device(&self.device)?,
            measured_tok_s: self.measured_tok_s,
        })
    }
}

/// Parses `cpu`, `metal`, or `cuda:N`.
fn parse_device(text: &str) -> Result<Device> {
    match text {
        "cpu" => Ok(Device::Cpu),
        "metal" => Ok(Device::Metal),
        other => {
            if let Some(index) = other.strip_prefix("cuda:") {
                let index = index.parse::<usize>().map_err(|_| {
                    Error::new(ErrCode::EngineLoad, format!("bad cuda index in {other:?}"))
                })?;
                return Ok(Device::Cuda { index });
            }
            Err(
                Error::new(ErrCode::EngineLoad, format!("unknown device {other:?}"))
                    .with_remedy("Use cpu, metal, or cuda:N."),
            )
        }
    }
}

/// What [`crate::FakeEngine::residency`] returns.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResidencySpec {
    /// The memory the harness may use.
    pub budget_bytes: u64,
    /// The memory the resident set uses.
    pub used_bytes: u64,
    /// The models.
    #[serde(default)]
    pub models: Vec<ResidentModelSpec>,
}

impl ResidencySpec {
    /// Converts this specification into a [`ResidencySnapshot`].
    pub fn to_snapshot(&self) -> ResidencySnapshot {
        ResidencySnapshot {
            budget_bytes: self.budget_bytes,
            used_bytes: self.used_bytes,
            models: self
                .models
                .iter()
                .map(ResidentModelSpec::to_model)
                .collect(),
        }
    }
}

/// One model in a scripted resident set.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResidentModelSpec {
    /// The model identifier.
    pub model_id: String,
    /// The role classes this model serves.
    #[serde(default)]
    pub classes: Vec<RoleClass>,
    /// `loaded`, `loading`, or `evicted`.
    #[serde(default = "default_slot_state")]
    pub state: String,
    /// Progress, used when the state is `loading`.
    #[serde(default)]
    pub progress: f32,
    /// The memory this model uses.
    #[serde(default)]
    pub bytes: u64,
    /// Whether the manager may never evict this model.
    #[serde(default)]
    pub pinned: bool,
    /// Whether a turn holds a lease on this model.
    #[serde(default)]
    pub leased: bool,
}

fn default_slot_state() -> String {
    "loaded".to_owned()
}

impl ResidentModelSpec {
    /// Converts this specification into a [`ResidentModel`].
    ///
    /// An unknown state string becomes [`SlotState::Loaded`].
    pub fn to_model(&self) -> ResidentModel {
        let state = match self.state.as_str() {
            "loading" => SlotState::Loading {
                progress: self.progress,
            },
            "evicted" => SlotState::Evicted,
            _ => SlotState::Loaded,
        };
        ResidentModel {
            model_id: self.model_id.clone(),
            classes: self.classes.clone(),
            state,
            bytes: self.bytes,
            pinned: self.pinned,
            leased: self.leased,
        }
    }
}

/// How the engine builds fake vectors.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EmbedSpec {
    /// The vector length.
    pub dim: usize,
    /// Vectors that override the hashed value, keyed by exact input text.
    ///
    /// Use this when a test needs an exact similarity rather than the
    /// similarity that shared words produce.
    #[serde(default)]
    pub fixed: Vec<FixedVector>,
}

impl Default for EmbedSpec {
    fn default() -> Self {
        Self {
            dim: 64,
            fixed: Vec::new(),
        }
    }
}

/// One exact vector for one exact input.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FixedVector {
    /// The input text to match.
    pub text: String,
    /// The vector to return. The engine normalises it.
    pub vector: Vec<f32>,
}

/// Builds a usage record for a played turn.
pub(crate) fn usage_for(prompt_tokens: usize, text: &str, reasoning: Option<&str>) -> Usage {
    let reasoning_tokens = reasoning.map_or(0, crate::token_count);
    Usage {
        prompt_tokens,
        completion_tokens: crate::token_count(text) + reasoning_tokens,
        reasoning_tokens,
        cached_tokens: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_script_parses() {
        let script = Script::from_toml("").expect("empty script is valid");
        assert!(script.turns.is_empty());
        assert_eq!(script.embed.dim, 64);
    }

    #[test]
    fn a_finish_reason_defaults_from_the_tool_calls() {
        let plain = Turn::default();
        assert_eq!(plain.finish_reason().unwrap(), FinishReason::Stop);

        let calling = Turn {
            tool_calls: vec![ScriptedToolCall {
                id: "c1".into(),
                name: "read_file".into(),
                args: toml::Value::Table(toml::map::Map::new()),
            }],
            ..Turn::default()
        };
        assert_eq!(calling.finish_reason().unwrap(), FinishReason::ToolCalls);
    }

    #[test]
    fn an_unknown_finish_reason_is_rejected() {
        let turn = Turn {
            finish: Some("wat".into()),
            ..Turn::default()
        };
        let err = turn.finish_reason().expect_err("unknown reason must fail");
        assert_eq!(err.code, ErrCode::EngineLoad);
    }

    #[test]
    fn devices_parse() {
        assert_eq!(parse_device("cpu").unwrap(), Device::Cpu);
        assert_eq!(parse_device("metal").unwrap(), Device::Metal);
        assert_eq!(parse_device("cuda:1").unwrap(), Device::Cuda { index: 1 });
        assert!(parse_device("tpu").is_err());
        assert!(parse_device("cuda:x").is_err());
    }

    #[test]
    fn an_unknown_field_is_rejected() {
        // A typo in a script should fail loudly, not be ignored.
        let err = Script::from_toml("token_delay_millis = 5").expect_err("typo must fail");
        assert_eq!(err.code, ErrCode::EngineLoad);
    }
}
