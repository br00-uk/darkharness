//! The real inference engine, over mistral.rs.
//!
//! This is the only crate that may depend on `mistralrs` (Rule 12), and
//! `cargo xtask check-deps` fails the build when another one does. Every
//! other crate holds the engine as `dyn Engine` and tests against
//! `dark-engine-fake` (Rule 17).
//!
//! Memory is the limit that shapes this crate. Estimate before loading and
//! never discover a limit by allocation failure. Never evict a pinned model
//! or one that holds a turn lease. Budget against `Caps::granted_context`,
//! never `Caps::max_context`. See Rules 1 to 4 and task units `B2` to `B7`.

pub mod determinism;
pub mod embed;
pub mod load;
pub mod resident;
pub mod stream;
pub mod tune;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use dark_contract::{
    Caps, ChunkStream, Device, EmbedPurpose, Engine, ErrCode, Error, EventTx, ResidencySnapshot,
    Result, RoleClass, Scored,
};
use tokio_util::sync::CancellationToken;

use resident::{ModelKey, ResidentSet};

/// The capability flags a caller cannot read off a loaded `mistralrs::Model`
/// directly, supplied at [`RealEngine::register_model`] time.
///
/// mistral.rs exposes `MistralRsConfig` (device, category, `max_seq_len`),
/// but nothing that says whether *this harness's* tool-call scraper should
/// run instead of mistral.rs's own parser, or whether the loaded weights
/// were built with logprobs in mind. Those are decisions the loader
/// already made when it picked a model and a chat template, so the loader
/// is what supplies them here, rather than this module trying to
/// rediscover them from the running model.
#[derive(Debug, Clone, Copy, PartialEq)]
// Like Caps in dark-contract: these are independent yes-or-no facts about
// one model, not a state machine hiding in four flags.
#[allow(clippy::struct_excessive_bools)]
pub struct ModelCapabilities {
    /// The engine parses tool calls itself for this model. See
    /// `crates/dark-engine/src/stream/request.rs`'s module documentation:
    /// tools are attached to the mistral.rs request only when this is
    /// `true`, which is what keeps a scraper-based model's output as plain
    /// text.
    pub native_tools: bool,
    /// The model supports a thinking mode.
    pub thinking: bool,
    /// The engine supports grammar-constrained decoding for this model.
    pub grammar: bool,
    /// The model accepts images.
    pub vision: bool,
    /// The engine returns log probabilities for this model.
    /// [`Engine::rerank`] needs this (Rule from task unit `B5`, step 5).
    pub logprobs: bool,
    /// The parameter count in billions, for [`Caps::params_b`].
    pub params_b: f32,
}

/// What [`RealEngine::install`] needs to load one model from a directory
/// and make it answerable.
///
/// This names no mistral.rs type on purpose: it is the composition root's
/// side of the seam Rule 12 draws, so `dark-cli` can describe a load
/// without depending on `mistralrs`.
#[derive(Debug, Clone)]
pub struct InstallSpec {
    /// Identifies the model in the resident set.
    pub key: ModelKey,
    /// The directory holding the weights, `config.json`, and
    /// `tokenizer.json`.
    pub dir: std::path::PathBuf,
    /// The format the weights are stored in.
    pub format: load::ModelFormat,
    /// The quantisation the weights are stored at, from the model's
    /// manifest.
    pub quant: String,
    /// The role classes this model serves.
    pub classes: Vec<RoleClass>,
    /// What the loaded model can do. See [`ModelCapabilities`].
    ///
    /// [`ModelCapabilities::params_b`] is **ignored** here:
    /// [`RealEngine::install`] measures the parameter count from the
    /// weights on disk (see [`load::shape`]) and fills the field in from
    /// that. A caller cannot know the count before the load, and a wrong
    /// one would gate the wrong tool set (`dark-tools` picks its gating
    /// row from `params_b`), so the measured figure wins.
    pub capabilities: ModelCapabilities,
    /// The context length to ask for. The resident set may grant less.
    pub requested_context: u64,
    /// The model's own maximum context length.
    pub max_context: u64,
}

/// A model this engine can route a request to.
struct RegisteredModel {
    model: Arc<mistralrs::Model>,
    tokenizer: Arc<tokenizers::Tokenizer>,
    capabilities: ModelCapabilities,
}

/// Ties the resident set, loading, streaming, embedding, and tuning
/// together behind the [`Engine`] trait.
///
/// Every method's error path works with no model loaded: this struct
/// starts with an empty [`ResidentSet`] and an empty model map, and every
/// method that needs a model reports a clean [`dark_contract::ErrCode`]
/// with a remedy when one is not there, rather than panicking or
/// unwrapping absent state. [`RealEngine::register_model`] is how a model
/// actually becomes available — the composition root (`dark-cli`) calls it
/// once [`load::materialize`] and [`resident::ResidentSet::finish_load`]
/// have both succeeded for a load.
pub struct RealEngine {
    resident: Arc<Mutex<ResidentSet>>,
    models: Mutex<HashMap<ModelKey, RegisteredModel>>,
    limiter: Mutex<stream::Limiter>,
    device: Device,
}

impl RealEngine {
    /// Creates an engine with an empty resident set budgeted at
    /// `budget_bytes`, and no models loaded.
    #[must_use]
    pub fn new(budget_bytes: u64, events: EventTx) -> Self {
        let device = tune::device::detect();
        Self {
            resident: Arc::new(Mutex::new(ResidentSet::new(budget_bytes, events))),
            models: Mutex::new(HashMap::new()),
            // A conservative default until a model is registered: one
            // sequence at a time. `register_model` resizes this from real
            // headroom once there is a resident model's footprint to
            // measure it against.
            limiter: Mutex::new(stream::Limiter::new(1)),
            device,
        }
    }

    /// Returns the resident set, for a caller (the composition root) that
    /// drives loads directly through [`load::pull`] and
    /// [`load::materialize`].
    #[must_use]
    pub fn resident(&self) -> Arc<Mutex<ResidentSet>> {
        Arc::clone(&self.resident)
    }

    /// Registers `model` as serving `key`, and resizes the concurrency
    /// limiter from the resident set's current headroom.
    ///
    /// The caller must have already reserved `key`'s memory in the
    /// resident set (`begin_load`/`finish_load`) before calling this: this
    /// method makes the model *answerable*, not resident — those are
    /// deliberately separate steps, so a load's memory accounting and a
    /// model's availability to serve a request can never drift apart from
    /// forgetting one of the two calls in either order.
    ///
    /// # Errors
    ///
    /// Returns [`ErrCode::EngineLoad`] when the resident set lock is
    /// poisoned.
    pub fn register_model(
        &self,
        key: ModelKey,
        model: Arc<mistralrs::Model>,
        tokenizer: Arc<tokenizers::Tokenizer>,
        capabilities: ModelCapabilities,
    ) -> Result<()> {
        self.models.lock().map_or_else(
            |_| Err(lock_poisoned_error()),
            |mut models| {
                models.insert(
                    key,
                    RegisteredModel {
                        model,
                        tokenizer,
                        capabilities,
                    },
                );
                Ok(())
            },
        )?;
        self.resize_limiter()
    }

    /// Loads the model in `spec`'s directory and makes it answerable, in
    /// the order Rule 1 requires.
    ///
    /// This is what the composition root calls. It exists because every
    /// step below either names a mistral.rs type or reads a mistral.rs
    /// format, and Rule 12 keeps both behind this crate: `dark-cli`
    /// depends on `mistralrs` through no path of its own, so it cannot
    /// call [`load::materialize`] or [`RealEngine::register_model`]
    /// directly. It hands over a directory and a [`ModelKey`] instead.
    ///
    /// The steps, in order:
    ///
    /// 1. Read the model's shape from `config.json` and the weight files
    ///    ([`load::shape`]).
    /// 2. Ask the resident set whether it fits, at what quantisation and
    ///    context ([`ResidentSet::begin_load`]). This happens **before**
    ///    any weight is read, which is what Rule 1 asks for.
    /// 3. Load the weights through mistral.rs ([`load::materialize`]).
    /// 4. Load the tokenizer handle this crate keeps for
    ///    [`Engine::tokenize`], which must answer synchronously.
    /// 5. Record the load ([`ResidentSet::finish_load`]) and register the
    ///    model ([`RealEngine::register_model`]).
    ///
    /// A failure at step 3 or 4 releases the reservation step 2 made, so
    /// a failed load never leaves the resident set holding memory that
    /// nothing uses.
    ///
    /// Returns the context length the resident set granted, which can be
    /// smaller than `spec.requested_context` when the degradation ladder
    /// had to cut it. Budget against the returned value, never the
    /// requested one (Rule 4).
    ///
    /// # Errors
    ///
    /// Returns [`ErrCode::EngineLoad`] when the model's shape cannot be
    /// read, when it does not fit at any size the ladder offers, when
    /// mistral.rs cannot load the weights, or when `tokenizer.json` is
    /// absent or unreadable. Every error carries a remedy.
    pub async fn install(&self, spec: InstallSpec) -> Result<u64> {
        let cfg = load::shape::read(&spec.dir, &spec.quant)?;
        let bits = resident::estimate::bits_per_weight(&spec.quant)?;

        let plan = {
            let mut set = self.resident.lock().map_err(|_| lock_poisoned_error())?;
            set.begin_load(resident::BeginLoadRequest {
                key: spec.key.clone(),
                cfg,
                classes: spec.classes.clone(),
                requested_quant: resident::QuantOption {
                    name: &spec.quant,
                    bits,
                },
                smaller_quants_on_disk: &[],
                requested_context: spec.requested_context,
                max_context: spec.max_context,
                alias_to_class: None,
            })?
        };

        let granted = match &plan {
            resident::LoadPlan::Fits { context, .. } => *context,
            // No fresh load happens for this key: it is already resident,
            // or the request aliased to a model that is. Report whatever
            // that slot already holds.
            resident::LoadPlan::AlreadyPresent | resident::LoadPlan::Alias { .. } => self
                .resident
                .lock()
                .ok()
                .and_then(|set| set.granted_context(&spec.key))
                .unwrap_or(spec.requested_context),
        };

        let load_spec = load::spec_for(&spec.dir, spec.format, &spec.quant)?;
        let outcome = async {
            let model = load::materialize::materialize(&load_spec).await?;
            let tokenizer = load::tokenizer_in(&spec.dir)?;
            Ok::<_, Error>((model, tokenizer))
        }
        .await;

        let (model, tokenizer) = match outcome {
            Ok(loaded) => loaded,
            Err(err) => {
                // The reservation must not outlive a load that failed.
                if let Ok(mut set) = self.resident.lock() {
                    let _ = set.fail_load(&spec.key);
                }
                return Err(err);
            }
        };

        {
            let mut set = self.resident.lock().map_err(|_| lock_poisoned_error())?;
            set.finish_load(&spec.key, None)?;
        }

        // The parameter count comes from the shape read off disk, never
        // from the caller: see `InstallSpec::capabilities`.
        #[allow(
            clippy::cast_precision_loss,
            reason = "a parameter count in billions is a gating and display figure; the \
                      precision lost at this magnitude is far below dark-tools' gating \
                      thresholds of 8B and 32B"
        )]
        let params_b = cfg.params as f32 / 1e9;
        let capabilities = ModelCapabilities {
            params_b,
            ..spec.capabilities
        };

        self.register_model(spec.key, Arc::new(model), Arc::new(tokenizer), capabilities)?;
        Ok(granted)
    }

    /// Resizes the concurrency limiter from current resident-set headroom,
    /// budgeting one sequence's key-value cache at a nominal 512 MiB —
    /// generous enough for a several-thousand-token context on a small
    /// model, conservative enough not to promise more concurrency than a
    /// large one can back. A precise per-model figure would need the
    /// loaded model's own shape, which [`RegisteredModel`] does not carry
    /// today; this is a deliberately simple starting point over an exact
    /// one, named here rather than left implicit.
    ///
    /// Replaces the limiter outright rather than resizing the existing
    /// one in place: [`stream::Limiter`] has no in-place resize, and does
    /// not need one — a [`stream::Permit`] already acquired from the old
    /// limiter owns its own handle to it and keeps working across this
    /// swap, so no in-flight turn is disturbed.
    fn resize_limiter(&self) -> Result<()> {
        const NOMINAL_SEQUENCE_BYTES: u64 = 512 * 1024 * 1024;
        let free = self
            .resident
            .lock()
            .map_err(|_| lock_poisoned_error())?
            .free_bytes();
        let target = stream::max_concurrent_sequences(free, NOMINAL_SEQUENCE_BYTES);
        *self.limiter.lock().map_err(|_| lock_poisoned_error())? = stream::Limiter::new(target);
        Ok(())
    }

    /// Looks up the model serving `class`, or a clean, remedied error.
    ///
    /// # Errors
    ///
    /// Returns [`ErrCode::EngineLoad`] when no model serves `class` yet, or
    /// when a lock is poisoned.
    fn resolve(&self, class: RoleClass) -> Result<(ModelKey, Arc<mistralrs::Model>, Caps)> {
        let key = {
            let resident = self.resident.lock().map_err(|_| lock_poisoned_error())?;
            resident
                .key_for_class(class)
                .cloned()
                .ok_or_else(|| no_model_error(class))?
        };

        let models = self.models.lock().map_err(|_| lock_poisoned_error())?;
        let registered = models.get(&key).ok_or_else(|| no_model_error(class))?;

        let resident = self.resident.lock().map_err(|_| lock_poisoned_error())?;
        let caps = Caps {
            model_id: key.to_string(),
            max_context: to_usize(resident.max_context_of(&key).unwrap_or(0)),
            granted_context: to_usize(resident.granted_context(&key).unwrap_or(0)),
            native_tools: registered.capabilities.native_tools,
            thinking: registered.capabilities.thinking,
            grammar: registered.capabilities.grammar,
            vision: registered.capabilities.vision,
            logprobs: registered.capabilities.logprobs,
            params_b: registered.capabilities.params_b,
            quant: resident.quant_of(&key).unwrap_or_default().to_owned(),
            device: self.device.clone(),
            measured_tok_s: None,
        };
        Ok((key, Arc::clone(&registered.model), caps))
    }
}

/// Builds the error every method returns when no model serves a role
/// class.
fn no_model_error(class: RoleClass) -> Error {
    Error::new(
        ErrCode::EngineLoad,
        format!("no model serves the {class} role class yet"),
    )
    .with_remedy("Run dark models pull, then try again.")
}

/// Builds the error a poisoned lock returns. This crate never panics while
/// holding one of its own locks, so this should be unreachable in
/// practice; it exists so a caller still gets a clean error rather than a
/// propagated panic if it ever is.
fn lock_poisoned_error() -> Error {
    Error::new(ErrCode::EngineGenerate, "an internal lock was poisoned")
}

/// Converts a `u64` byte or token count to `usize`, saturating rather than
/// panicking on a platform where `usize` is narrower.
fn to_usize(value: u64) -> usize {
    usize::try_from(value).unwrap_or(usize::MAX)
}

#[async_trait]
impl Engine for RealEngine {
    async fn caps(&self, class: RoleClass) -> Result<Caps> {
        self.resolve(class).map(|(_, _, caps)| caps)
    }

    async fn stream(
        &self,
        req: dark_contract::Request,
        cancel: CancellationToken,
    ) -> Result<ChunkStream> {
        let (key, model, caps) = self.resolve(req.class)?;
        let turn = resident::TurnId::new(ulid::Ulid::new().to_string());
        let limiter = self
            .limiter
            .lock()
            .map_err(|_| lock_poisoned_error())?
            .clone();
        stream::live::run(
            model,
            &key,
            Arc::clone(&self.resident),
            &limiter,
            &req,
            &caps,
            turn,
            cancel,
        )
        .await
    }

    async fn embed(&self, texts: Vec<String>, purpose: EmbedPurpose) -> Result<Vec<Vec<f32>>> {
        let (_, model, _caps) = self.resolve(RoleClass::Embed)?;
        embed::embed_via_model(
            &model,
            &texts,
            purpose,
            &embed::PrefixConfig::default(),
            embed::DEFAULT_BATCH_SIZE,
        )
        .await
    }

    async fn rerank(&self, query: &str, docs: Vec<String>) -> Result<Vec<Scored>> {
        let (_, model, caps) = self.resolve(RoleClass::Rerank)?;
        embed::rerank_via_model(&model, &caps, query, &docs).await
    }

    fn tokenize(&self, class: RoleClass, text: &str) -> Result<usize> {
        // `mistralrs::Model::tokenize` is async — it round-trips through
        // the engine's request channel, the same as a generation would.
        // The contract asks for a synchronous count (dark-core's context
        // budgeting runs it inline while assembling a prefix, never
        // wanting to await a whole channel round-trip just to size a
        // string), so this crate keeps its own `tokenizers::Tokenizer`
        // handle per registered model instead — loaded once, synchronously,
        // from the same `tokenizer.json` mistral.rs itself reads. See
        // docs/adr/0006.
        let key = {
            let resident = self.resident.lock().map_err(|_| lock_poisoned_error())?;
            resident
                .key_for_class(class)
                .cloned()
                .ok_or_else(|| no_model_error(class))?
        };
        let models = self.models.lock().map_err(|_| lock_poisoned_error())?;
        let registered = models.get(&key).ok_or_else(|| no_model_error(class))?;
        let encoding = registered.tokenizer.encode(text, false).map_err(|source| {
            Error::new(
                ErrCode::EngineGenerate,
                format!("tokenize failed: {source}"),
            )
        })?;
        Ok(encoding.len())
    }

    fn residency(&self) -> ResidencySnapshot {
        self.resident.lock().map_or_else(
            |poisoned| poisoned.into_inner().snapshot(),
            |resident| resident.snapshot(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dark_contract::{EventBus, Message, Request, Role};

    fn engine() -> RealEngine {
        RealEngine::new(8 * 1024 * 1024 * 1024, EventBus::new().tx())
    }

    #[tokio::test]
    async fn caps_fails_cleanly_with_no_model_loaded() {
        let err = engine().caps(RoleClass::Worker).await.unwrap_err();
        assert_eq!(err.code, ErrCode::EngineLoad);
        assert!(err.remedy.is_some());
    }

    #[tokio::test]
    async fn stream_fails_cleanly_with_no_model_loaded() {
        let req = Request::new(RoleClass::Worker, vec![Message::text(Role::User, "hi")]);
        let result = engine().stream(req, CancellationToken::new()).await;
        let Err(err) = result else {
            panic!("expected an error");
        };
        assert_eq!(err.code, ErrCode::EngineLoad);
    }

    #[tokio::test]
    async fn embed_fails_cleanly_with_no_model_loaded() {
        let err = engine()
            .embed(vec!["hello".to_owned()], EmbedPurpose::Query)
            .await
            .unwrap_err();
        assert_eq!(err.code, ErrCode::EngineLoad);
    }

    #[tokio::test]
    async fn rerank_fails_cleanly_with_no_model_loaded() {
        let err = engine()
            .rerank("query", vec!["doc".to_owned()])
            .await
            .unwrap_err();
        assert_eq!(err.code, ErrCode::EngineLoad);
    }

    #[test]
    fn tokenize_fails_cleanly_with_no_model_loaded() {
        let err = engine().tokenize(RoleClass::Worker, "hello").unwrap_err();
        assert_eq!(err.code, ErrCode::EngineLoad);
    }

    #[test]
    fn residency_is_empty_with_no_model_loaded() {
        let snapshot = engine().residency();
        assert!(snapshot.models.is_empty());
        assert_eq!(snapshot.used_bytes, 0);
    }

    #[test]
    fn residency_never_panics_even_after_many_reads() {
        let engine = engine();
        for _ in 0..100 {
            let _ = engine.residency();
        }
    }
}
