//! [`ResidentSet`]: the state machine that owns which models are in memory.
//!
//! This is the intellectual core of task unit `B3`. It never touches
//! mistral.rs, a file, or a device: every method is a transition over the
//! struct's own fields, so a test drives the whole lifecycle — load, evict,
//! lease, degrade, refuse — with no model file and no accelerator.

use std::collections::{HashMap, HashSet, VecDeque};

use dark_contract::{
    Error, Event, EventTx, ResidencySnapshot, ResidentModel, Result, RoleClass, SlotState,
};

use super::degrade::{self, DegradeRequest, Outcome as DegradeOutcome, QuantOption, Step};
use super::estimate::{self, ModelConfig};
use super::model_key::{ModelKey, TurnId};

/// The narrowest context this harness offers before it moves past step 1 of
/// the degradation ladder. Chosen well under any profile's default: a
/// context this small is a visible degradation, not a silent one.
pub const DEFAULT_MIN_CONTEXT: u64 = 2048;

/// One model's state inside the resident set.
#[derive(Debug, Clone)]
struct Slot {
    state: SlotState,
    classes: Vec<RoleClass>,
    cfg: ModelConfig,
    quant: String,
    granted_context: u64,
    max_context: u64,
    /// The total footprint (weights + key-value cache + 10% headroom) at
    /// `granted_context`. `0` once the slot is [`SlotState::Evicted`].
    bytes: u64,
}

/// What to load, handed to [`ResidentSet::begin_load`].
#[derive(Debug, Clone)]
pub struct BeginLoadRequest<'a> {
    /// The model to load.
    pub key: ModelKey,
    /// The model's shape, from its configuration file.
    pub cfg: ModelConfig,
    /// The role classes this load will serve.
    pub classes: Vec<RoleClass>,
    /// The quantisation the caller asked for.
    pub requested_quant: QuantOption<'a>,
    /// Smaller quantisations on disk, for the degradation ladder's step 2.
    pub smaller_quants_on_disk: &'a [QuantOption<'a>],
    /// The context length the caller asked for.
    pub requested_context: u64,
    /// The model's own maximum context length.
    pub max_context: u64,
    /// A smaller, already-resident class this request may alias to
    /// (the degradation ladder's step 3), when one exists.
    pub alias_to_class: Option<RoleClass>,
}

/// What [`ResidentSet::begin_load`] decided.
#[derive(Debug, Clone, PartialEq)]
pub enum LoadPlan {
    /// The requested model already has a loaded or loading slot; its class
    /// list absorbed the new classes and no eviction or degradation ran.
    AlreadyPresent,
    /// The model fits, at `quant`/`context`, which may be smaller than
    /// requested if a rung of the degradation ladder had to reduce them.
    /// `evicted` lists the models the resident set evicted to make room,
    /// oldest-evicted first. `degrade_steps` is empty when the request fit
    /// with no need to climb the ladder.
    Fits {
        /// The quantisation to load.
        quant: String,
        /// The context length granted.
        context: u64,
        /// Models evicted to make room, in eviction order.
        evicted: Vec<ModelKey>,
        /// The ladder steps visited, oldest first.
        degrade_steps: Vec<Step>,
    },
    /// The model does not fit at any size, but the caller's role class may
    /// alias to `class`, which is already resident.
    Alias {
        /// The class to use instead.
        class: RoleClass,
        /// Models evicted while searching for room, in eviction order.
        evicted: Vec<ModelKey>,
        /// The ladder steps visited, oldest first.
        degrade_steps: Vec<Step>,
    },
}

/// Controls which models are in memory and prevents memory exhaustion
/// (Rules 1 to 4).
#[derive(Debug)]
pub struct ResidentSet {
    budget_bytes: u64,
    slots: HashMap<ModelKey, Slot>,
    pinned: HashSet<ModelKey>,
    lru: VecDeque<ModelKey>,
    turn_leases: HashMap<TurnId, ModelKey>,
    events: EventTx,
    min_context: u64,
}

impl ResidentSet {
    /// Creates an empty resident set with `budget_bytes` of memory to grant
    /// and `events` as the channel that carries [`Event::Residency`].
    #[must_use]
    pub fn new(budget_bytes: u64, events: EventTx) -> Self {
        Self {
            budget_bytes,
            slots: HashMap::new(),
            pinned: HashSet::new(),
            lru: VecDeque::new(),
            turn_leases: HashMap::new(),
            events,
            min_context: DEFAULT_MIN_CONTEXT,
        }
    }

    /// Sets the narrowest context the degradation ladder offers before it
    /// moves past step 1. Defaults to [`DEFAULT_MIN_CONTEXT`].
    #[must_use]
    pub fn with_min_context(mut self, min_context: u64) -> Self {
        self.min_context = min_context;
        self
    }

    /// Returns the total memory budget.
    #[must_use]
    pub fn budget_bytes(&self) -> u64 {
        self.budget_bytes
    }

    /// Returns the memory every non-evicted slot uses.
    #[must_use]
    pub fn used_bytes(&self) -> u64 {
        self.slots
            .values()
            .filter(|slot| !matches!(slot.state, SlotState::Evicted))
            .map(|slot| slot.bytes)
            .sum()
    }

    /// Returns the memory that is not yet committed to a slot.
    #[must_use]
    pub fn free_bytes(&self) -> u64 {
        self.budget_bytes.saturating_sub(self.used_bytes())
    }

    /// Pins `key`. Rule 2: the resident set manager never evicts a pinned
    /// model. Pin the embedding model, by convention, right after its first
    /// [`Self::begin_load`].
    pub fn pin(&mut self, key: ModelKey) {
        self.pinned.insert(key);
    }

    /// Reports whether `key` is pinned.
    #[must_use]
    pub fn is_pinned(&self, key: &ModelKey) -> bool {
        self.pinned.contains(key)
    }

    /// Reports whether some turn holds a lease on `key`. Rule 3: the
    /// resident set manager never evicts a leased model.
    #[must_use]
    pub fn is_leased(&self, key: &ModelKey) -> bool {
        self.turn_leases.values().any(|leased| leased == key)
    }

    /// Returns the context length [`Self::begin_load`] granted to `key`,
    /// when it has a slot.
    #[must_use]
    pub fn granted_context(&self, key: &ModelKey) -> Option<u64> {
        self.slots.get(key).map(|slot| slot.granted_context)
    }

    /// Returns the quantisation `key` loaded at, when it has a slot.
    #[must_use]
    pub fn quant_of(&self, key: &ModelKey) -> Option<&str> {
        self.slots.get(key).map(|slot| slot.quant.as_str())
    }

    /// Returns the model's own maximum context length, when `key` has a
    /// slot. This is distinct from [`Self::granted_context`], which is
    /// bounded by the memory budget, not by the model itself (Rule 4).
    #[must_use]
    pub fn max_context_of(&self, key: &ModelKey) -> Option<u64> {
        self.slots.get(key).map(|slot| slot.max_context)
    }

    /// Returns the shape [`estimate`] used for `key`, when it has a slot.
    #[must_use]
    pub fn cfg_of(&self, key: &ModelKey) -> Option<ModelConfig> {
        self.slots.get(key).map(|slot| slot.cfg)
    }

    /// Returns the role classes `key` serves, when it has a slot.
    #[must_use]
    pub fn classes_of(&self, key: &ModelKey) -> Option<&[RoleClass]> {
        self.slots.get(key).map(|slot| slot.classes.as_slice())
    }

    /// Returns the memory `key` uses, when it has a slot. `0` once evicted.
    #[must_use]
    pub fn bytes_of(&self, key: &ModelKey) -> Option<u64> {
        self.slots.get(key).map(|slot| slot.bytes)
    }

    /// Returns whether `key` is loaded, loading, or evicted.
    #[must_use]
    pub fn state_of(&self, key: &ModelKey) -> Option<SlotState> {
        self.slots.get(key).map(|slot| slot.state)
    }

    /// Returns the key of the [`SlotState::Loaded`] model that serves
    /// `class`, when one is resident. A caller builds
    /// [`dark_contract::Caps`] for a role class from this key's other
    /// accessors.
    #[must_use]
    pub fn key_for_class(&self, class: RoleClass) -> Option<&ModelKey> {
        self.slots
            .iter()
            .find(|(_, slot)| {
                matches!(slot.state, SlotState::Loaded) && slot.classes.contains(&class)
            })
            .map(|(key, _)| key)
    }

    /// Returns a snapshot of what is in memory now, for
    /// [`dark_contract::Engine::residency`] and [`Event::Residency`].
    #[must_use]
    pub fn snapshot(&self) -> ResidencySnapshot {
        let mut models: Vec<ResidentModel> = self
            .slots
            .iter()
            .map(|(key, slot)| ResidentModel {
                model_id: key.to_string(),
                classes: slot.classes.clone(),
                state: slot.state,
                bytes: slot.bytes,
                pinned: self.pinned.contains(key),
                leased: self.is_leased(key),
            })
            .collect();
        // A HashMap iterates in an arbitrary order; a snapshot a test
        // compares must not depend on it.
        models.sort_by(|a, b| a.model_id.cmp(&b.model_id));
        ResidencySnapshot {
            budget_bytes: self.budget_bytes,
            used_bytes: self.used_bytes(),
            models,
        }
    }

    /// Emits the current snapshot as [`Event::Residency`].
    fn emit_residency(&self) {
        self.events.send(Event::Residency(self.snapshot()));
    }

    /// Removes the least recently used slot that is unpinned, unleased, and
    /// [`SlotState::Loaded`]. `Loading` slots are never eviction candidates:
    /// a load in progress has not yet produced anything a caller can use
    /// again, so evicting it would only waste the work already spent on it.
    fn pop_lru_evictable(&mut self) -> Option<ModelKey> {
        let position = self.lru.iter().position(|key| {
            !self.pinned.contains(key)
                && !self.is_leased(key)
                && matches!(
                    self.slots.get(key).map(|slot| &slot.state),
                    Some(SlotState::Loaded)
                )
        })?;
        self.lru.remove(position)
    }

    /// Marks `key` evicted and frees its memory. `key` must already be out
    /// of the LRU queue (see [`Self::pop_lru_evictable`]).
    fn evict(&mut self, key: &ModelKey) {
        if let Some(slot) = self.slots.get_mut(key) {
            slot.state = SlotState::Evicted;
            slot.bytes = 0;
        }
    }

    /// Moves `key` to the most-recently-used end of the LRU queue.
    fn touch_lru(&mut self, key: &ModelKey) {
        self.lru.retain(|existing| existing != key);
        self.lru.push_back(key.clone());
    }

    /// Decides how to load the model `req` describes: reusing an existing
    /// slot, evicting least-recently-used models to make room, or climbing
    /// the degradation ladder (task unit `B3`, step 8) when eviction alone
    /// is not enough. Estimates memory before committing to anything (Rule
    /// 4): this never discovers a limit by allocation failure, because it
    /// allocates nothing itself.
    ///
    /// On [`LoadPlan::Fits`] or [`LoadPlan::Alias`], the caller still has to
    /// call [`Self::finish_load`] once the weights actually materialise, or
    /// [`Self::fail_load`] if that fails. This method reserves the memory
    /// immediately, before that happens, so a second concurrent load cannot
    /// also claim it.
    ///
    /// # Errors
    ///
    /// Returns [`dark_contract::ErrCode::EngineWontFit`] with the shortfall
    /// in bytes when the model does not fit at any context, quantisation, or
    /// role-class alias.
    pub fn begin_load(&mut self, req: BeginLoadRequest<'_>) -> Result<LoadPlan> {
        if let Some(slot) = self.slots.get_mut(&req.key)
            && !matches!(slot.state, SlotState::Evicted)
        {
            for class in req.classes {
                if !slot.classes.contains(&class) {
                    slot.classes.push(class);
                }
            }
            self.touch_lru(&req.key);
            self.emit_residency();
            return Ok(LoadPlan::AlreadyPresent);
        }

        let full_needed =
            estimate::total_bytes(req.cfg, req.requested_context, req.requested_quant.bits);

        // The most this key could ever free up by evicting: the budget
        // minus every slot that is not itself evictable (pinned, leased,
        // or already this key).
        let reserved_elsewhere: u64 = self
            .slots
            .iter()
            .filter(|(key, slot)| {
                *key != &req.key
                    && !matches!(slot.state, SlotState::Evicted)
                    && (self.pinned.contains(*key) || self.is_leased(key))
            })
            .map(|(_, slot)| slot.bytes)
            .sum();
        let max_possible_free = self.budget_bytes.saturating_sub(reserved_elsewhere);

        let mut evicted = Vec::new();

        if full_needed <= max_possible_free {
            while self.free_bytes() < full_needed {
                let Some(victim) = self.pop_lru_evictable() else {
                    break;
                };
                self.evict(&victim);
                evicted.push(victim);
            }
            self.insert_loading_slot(&req, req.requested_quant.name, req.requested_context);
            self.emit_residency();
            return Ok(LoadPlan::Fits {
                quant: req.requested_quant.name.to_owned(),
                context: req.requested_context,
                evicted,
                degrade_steps: Vec::new(),
            });
        }

        // Even evicting everything evictable will not make room: evict it
        // all anyway (the caller needs every byte it can get), then climb
        // the ladder against what remains.
        while let Some(victim) = self.pop_lru_evictable() {
            self.evict(&victim);
            evicted.push(victim);
        }

        let degrade_req = DegradeRequest {
            cfg: req.cfg,
            requested_context: req.requested_context,
            min_context: self.min_context,
            max_context: req.max_context,
            requested_quant: req.requested_quant,
            smaller_quants_on_disk: req.smaller_quants_on_disk,
            alias_to_class: req.alias_to_class,
            budget_bytes: self.free_bytes(),
        };
        let (outcome, degrade_steps) = degrade::climb(&degrade_req);

        let result = match outcome {
            DegradeOutcome::Fits { quant, context, .. } => {
                self.insert_loading_slot(&req, &quant, context);
                Ok(LoadPlan::Fits {
                    quant,
                    context,
                    evicted,
                    degrade_steps,
                })
            }
            DegradeOutcome::Alias { class } => Ok(LoadPlan::Alias {
                class,
                evicted,
                degrade_steps,
            }),
            DegradeOutcome::Refuse(err) => Err(err),
        };
        self.emit_residency();
        result
    }

    /// Inserts a `Loading` slot for `req.key` at `quant`/`context`.
    fn insert_loading_slot(&mut self, req: &BeginLoadRequest<'_>, quant: &str, context: u64) {
        let bits = if quant == req.requested_quant.name {
            req.requested_quant.bits
        } else {
            req.smaller_quants_on_disk
                .iter()
                .find(|candidate| candidate.name == quant)
                .map_or(req.requested_quant.bits, |candidate| candidate.bits)
        };
        let bytes = estimate::total_bytes(req.cfg, context, bits);
        self.slots.insert(
            req.key.clone(),
            Slot {
                state: SlotState::Loading { progress: 0.0 },
                classes: req.classes.clone(),
                cfg: req.cfg,
                quant: quant.to_owned(),
                granted_context: context,
                max_context: req.max_context,
                bytes,
            },
        );
        self.touch_lru(&req.key);
    }

    /// Updates the load progress for `key`. Emits [`Event::Residency`].
    ///
    /// # Errors
    ///
    /// Returns [`dark_contract::ErrCode::EngineLoad`] when `key` has no
    /// `Loading` slot.
    pub fn set_progress(&mut self, key: &ModelKey, progress: f32) -> Result<()> {
        let slot = self.loading_slot_mut(key)?;
        slot.state = SlotState::Loading { progress };
        self.emit_residency();
        Ok(())
    }

    /// Marks `key` loaded. `measured_bytes`, when given, replaces the
    /// estimate with what the load actually used; `None` keeps the
    /// estimate. Emits [`Event::Residency`].
    ///
    /// # Errors
    ///
    /// Returns [`dark_contract::ErrCode::EngineLoad`] when `key` has no
    /// `Loading` slot.
    pub fn finish_load(&mut self, key: &ModelKey, measured_bytes: Option<u64>) -> Result<()> {
        let slot = self.loading_slot_mut(key)?;
        slot.state = SlotState::Loaded;
        if let Some(bytes) = measured_bytes {
            slot.bytes = bytes;
        }
        self.emit_residency();
        Ok(())
    }

    /// Abandons a load that failed: frees the memory it had reserved and
    /// removes the slot. Emits [`Event::Residency`].
    ///
    /// # Errors
    ///
    /// Returns [`dark_contract::ErrCode::EngineLoad`] when `key` has no
    /// `Loading` slot.
    pub fn fail_load(&mut self, key: &ModelKey) -> Result<()> {
        self.loading_slot_mut(key)?;
        self.slots.remove(key);
        self.lru.retain(|existing| existing != key);
        self.emit_residency();
        Ok(())
    }

    /// Borrows the `Loading` slot for `key`, or an `E_ENGINE_LOAD` error
    /// naming it.
    fn loading_slot_mut(&mut self, key: &ModelKey) -> Result<&mut Slot> {
        match self.slots.get_mut(key) {
            Some(slot) if matches!(slot.state, SlotState::Loading { .. }) => Ok(slot),
            Some(_) => Err(engine_load_error(&format!("{key} is not loading"))),
            None => Err(engine_load_error(&format!("{key} has no slot"))),
        }
    }

    /// Grants `turn` a lease on `key`, so [`Self::begin_load`] never evicts
    /// it (Rule 3) until [`Self::release_turn`]. Emits [`Event::Residency`].
    ///
    /// # Errors
    ///
    /// Returns [`dark_contract::ErrCode::EngineLoad`] when `key` is not
    /// [`SlotState::Loaded`].
    pub fn acquire_turn_lease(&mut self, turn: TurnId, key: ModelKey) -> Result<()> {
        match self.slots.get(&key) {
            Some(slot) if matches!(slot.state, SlotState::Loaded) => {}
            Some(_) => return Err(engine_load_error(&format!("{key} is not loaded"))),
            None => return Err(engine_load_error(&format!("{key} has no slot"))),
        }
        self.touch_lru(&key);
        self.turn_leases.insert(turn, key);
        self.emit_residency();
        Ok(())
    }

    /// Releases the lease `turn` holds, when it holds one. Emits
    /// [`Event::Residency`] only when a lease was actually released.
    pub fn release_turn(&mut self, turn: &TurnId) {
        if self.turn_leases.remove(turn).is_some() {
            self.emit_residency();
        }
    }

    /// Returns the key `turn` holds a lease on, when it holds one.
    #[must_use]
    pub fn lease_of(&self, turn: &TurnId) -> Option<&ModelKey> {
        self.turn_leases.get(turn)
    }

    /// Returns how many turn leases are outstanding.
    ///
    /// The `cancel_leak` acceptance test (task unit `B4`) asserts this
    /// returns to `0` after 1000 cancelled turns each acquire and release a
    /// lease.
    #[must_use]
    pub fn outstanding_leases(&self) -> usize {
        self.turn_leases.len()
    }
}

/// Builds an `E_ENGINE_LOAD` error with `message`.
fn engine_load_error(message: &str) -> Error {
    Error::new(dark_contract::ErrCode::EngineLoad, message.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use dark_contract::{EventBus, Received};

    const GIB: u64 = 1024 * 1024 * 1024;

    fn small_cfg() -> ModelConfig {
        ModelConfig {
            params: 4_000_000_000,
            layers: 36,
            kv_heads: 8,
            head_dim: 128,
        }
    }

    fn q4k() -> QuantOption<'static> {
        QuantOption {
            name: "q4k",
            bits: 4.0,
        }
    }

    fn set(budget: u64) -> (ResidentSet, EventBus) {
        let bus = EventBus::new();
        (ResidentSet::new(budget, bus.tx()), bus)
    }

    fn load_req(key: ModelKey, class: RoleClass) -> BeginLoadRequest<'static> {
        BeginLoadRequest {
            key,
            cfg: small_cfg(),
            classes: vec![class],
            requested_quant: q4k(),
            smaller_quants_on_disk: &[],
            requested_context: 8192,
            max_context: 131_072,
            alias_to_class: None,
        }
    }

    #[test]
    fn a_model_that_fits_loads_with_no_eviction() {
        let (mut set, _bus) = set(8 * GIB);
        let key = ModelKey::new("Qwen/Qwen3-4B", "q4k");
        let plan = set
            .begin_load(load_req(key.clone(), RoleClass::Worker))
            .unwrap();
        assert!(matches!(plan, LoadPlan::Fits { evicted, .. } if evicted.is_empty()));
        assert!(matches!(
            set.snapshot().models[0].state,
            SlotState::Loading { .. }
        ));
    }

    #[test]
    fn finish_load_moves_the_slot_to_loaded() {
        let (mut set, _bus) = set(8 * GIB);
        let key = ModelKey::new("Qwen/Qwen3-4B", "q4k");
        set.begin_load(load_req(key.clone(), RoleClass::Worker))
            .unwrap();
        set.finish_load(&key, None).unwrap();
        assert_eq!(set.snapshot().models[0].state, SlotState::Loaded);
    }

    #[test]
    fn finish_load_fails_for_a_key_with_no_slot() {
        let (mut set, _bus) = set(8 * GIB);
        let key = ModelKey::new("Qwen/Qwen3-4B", "q4k");
        let err = set.finish_load(&key, None).unwrap_err();
        assert_eq!(err.code, dark_contract::ErrCode::EngineLoad);
    }

    #[test]
    fn measured_bytes_replaces_the_estimate_on_finish() {
        let (mut set, _bus) = set(8 * GIB);
        let key = ModelKey::new("Qwen/Qwen3-4B", "q4k");
        set.begin_load(load_req(key.clone(), RoleClass::Worker))
            .unwrap();
        set.finish_load(&key, Some(123)).unwrap();
        assert_eq!(set.snapshot().models[0].bytes, 123);
    }

    #[test]
    fn fail_load_frees_the_reserved_memory() {
        let (mut set, _bus) = set(8 * GIB);
        let key = ModelKey::new("Qwen/Qwen3-4B", "q4k");
        set.begin_load(load_req(key.clone(), RoleClass::Worker))
            .unwrap();
        assert!(set.used_bytes() > 0);
        set.fail_load(&key).unwrap();
        assert_eq!(set.used_bytes(), 0);
        assert!(set.snapshot().models.is_empty());
    }

    #[test]
    fn loading_a_second_role_class_for_an_already_loading_key_adds_to_it_not_a_new_slot() {
        let (mut set, _bus) = set(8 * GIB);
        let key = ModelKey::new("Qwen/Qwen3-4B", "q4k");
        set.begin_load(load_req(key.clone(), RoleClass::Worker))
            .unwrap();
        let plan = set
            .begin_load(load_req(key.clone(), RoleClass::Architect))
            .unwrap();
        assert_eq!(plan, LoadPlan::AlreadyPresent);
        assert_eq!(set.snapshot().models.len(), 1);
        assert_eq!(
            set.snapshot().models[0].classes,
            vec![RoleClass::Worker, RoleClass::Architect]
        );
    }

    #[test]
    fn least_recently_used_unpinned_unleased_model_is_evicted_first() {
        // Two 4B models at ~2.2 GiB each, and a 4.5 GiB budget: only one
        // fits alongside the other, so the second load must evict the
        // first.
        let (mut set, _bus) = set((9 * GIB) / 2);
        let old = ModelKey::new("Qwen/Qwen3-4B", "q4k");
        let newer = ModelKey::new("Qwen/Qwen3-4B-instruct", "q4k");
        set.begin_load(load_req(old.clone(), RoleClass::Worker))
            .unwrap();
        set.finish_load(&old, None).unwrap();

        let plan = set
            .begin_load(load_req(newer.clone(), RoleClass::Worker))
            .unwrap();
        match plan {
            LoadPlan::Fits { evicted, .. } => assert_eq!(evicted, vec![old.clone()]),
            other => panic!("expected Fits with an eviction, got {other:?}"),
        }
        assert_eq!(set.snapshot().models.len(), 2);
        let old_model = set
            .snapshot()
            .models
            .into_iter()
            .find(|m| m.model_id == old.to_string())
            .unwrap();
        assert_eq!(old_model.state, SlotState::Evicted);
        assert_eq!(old_model.bytes, 0);
    }

    #[test]
    fn a_pinned_model_is_never_evicted() {
        let (mut set, _bus) = set((9 * GIB) / 2);
        let embed = ModelKey::new("Qwen/Qwen3-Embedding-0.6B", "q8_0");
        set.begin_load(load_req(embed.clone(), RoleClass::Embed))
            .unwrap();
        set.finish_load(&embed, None).unwrap();
        set.pin(embed.clone());

        let worker = ModelKey::new("Qwen/Qwen3-4B", "q4k");
        // Budget has room for only one 4B-class model beside the pinned
        // one, so a second worker load cannot evict the embed model — it
        // must refuse or degrade instead of touching it.
        let _ = set.begin_load(load_req(worker, RoleClass::Worker));

        let embed_after = set
            .snapshot()
            .models
            .into_iter()
            .find(|m| m.model_id == embed.to_string())
            .unwrap();
        assert_eq!(
            embed_after.state,
            SlotState::Loaded,
            "a pinned model must never be evicted, Rule 2"
        );
    }

    #[test]
    fn a_leased_model_is_never_evicted_during_a_turn() {
        let (mut set, _bus) = set((9 * GIB) / 2);
        let leased = ModelKey::new("Qwen/Qwen3-4B", "q4k");
        set.begin_load(load_req(leased.clone(), RoleClass::Worker))
            .unwrap();
        set.finish_load(&leased, None).unwrap();
        set.acquire_turn_lease(TurnId::new("turn-1"), leased.clone())
            .unwrap();

        let other = ModelKey::new("Qwen/Qwen3-4B-instruct", "q4k");
        let _ = set.begin_load(load_req(other, RoleClass::Worker));

        let leased_after = set
            .snapshot()
            .models
            .into_iter()
            .find(|m| m.model_id == leased.to_string())
            .unwrap();
        assert_eq!(
            leased_after.state,
            SlotState::Loaded,
            "a leased model must never be evicted during a turn, Rule 3"
        );
    }

    #[test]
    fn release_turn_makes_the_model_evictable_again() {
        let (mut set, _bus) = set((9 * GIB) / 2);
        let key = ModelKey::new("Qwen/Qwen3-4B", "q4k");
        set.begin_load(load_req(key.clone(), RoleClass::Worker))
            .unwrap();
        set.finish_load(&key, None).unwrap();
        let turn = TurnId::new("turn-1");
        set.acquire_turn_lease(turn.clone(), key.clone()).unwrap();
        set.release_turn(&turn);
        assert!(!set.is_leased(&key));

        let other = ModelKey::new("Qwen/Qwen3-4B-instruct", "q4k");
        let plan = set.begin_load(load_req(other, RoleClass::Worker)).unwrap();
        assert!(matches!(plan, LoadPlan::Fits { evicted, .. } if evicted == vec![key]));
    }

    #[test]
    fn wont_fit_reports_the_shortfall_in_bytes() {
        let (mut set, _bus) = set(64 * 1024 * 1024);
        let key = ModelKey::new("Qwen/Qwen3-4B", "q4k");
        let err = set
            .begin_load(load_req(key, RoleClass::Worker))
            .unwrap_err();
        assert_eq!(err.code, dark_contract::ErrCode::EngineWontFit);
        assert!(err.remedy.is_some());
        assert!(err.message.contains("bytes"));
    }

    #[test]
    fn a_request_that_needs_degradation_reports_its_steps() {
        // 3 GiB is short of what q4k needs at the full 131072-token
        // request, so the ladder must reduce the context to fit.
        let (mut set, _bus) = set(3 * GIB);
        let req = BeginLoadRequest {
            requested_context: 131_072,
            ..load_req(ModelKey::new("Qwen/Qwen3-4B", "q4k"), RoleClass::Worker)
        };
        let plan = set.begin_load(req).unwrap();
        match plan {
            LoadPlan::Fits {
                context,
                degrade_steps,
                ..
            } => {
                assert!(context < 131_072);
                assert!(!degrade_steps.is_empty());
            }
            other => panic!("expected a degraded Fits, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn begin_load_emits_residency_on_every_change() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        let mut set = ResidentSet::new(8 * GIB, bus.tx());
        let key = ModelKey::new("Qwen/Qwen3-4B", "q4k");
        set.begin_load(load_req(key.clone(), RoleClass::Worker))
            .unwrap();
        set.finish_load(&key, None).unwrap();

        for _ in 0..2 {
            let received = tokio::time::timeout(std::time::Duration::from_secs(1), rx.recv())
                .await
                .expect("a residency event should already be waiting")
                .expect("the bus is still open");
            assert!(
                matches!(received, Received::Event(Event::Residency(_))),
                "expected Event::Residency, got {received:?}"
            );
        }
    }

    #[test]
    fn free_bytes_is_the_budget_minus_used_bytes() {
        let (mut set, _bus) = set(8 * GIB);
        let key = ModelKey::new("Qwen/Qwen3-4B", "q4k");
        set.begin_load(load_req(key, RoleClass::Worker)).unwrap();
        assert_eq!(set.free_bytes(), set.budget_bytes() - set.used_bytes());
    }

    #[test]
    fn granted_context_reports_what_begin_load_decided() {
        let (mut set, _bus) = set(8 * GIB);
        let key = ModelKey::new("Qwen/Qwen3-4B", "q4k");
        set.begin_load(load_req(key.clone(), RoleClass::Worker))
            .unwrap();
        assert_eq!(set.granted_context(&key), Some(8192));
    }

    #[test]
    fn accessors_report_what_begin_load_decided() {
        let (mut set, _bus) = set(8 * GIB);
        let key = ModelKey::new("Qwen/Qwen3-4B", "q4k");
        set.begin_load(load_req(key.clone(), RoleClass::Worker))
            .unwrap();
        assert_eq!(set.quant_of(&key), Some("q4k"));
        assert_eq!(set.max_context_of(&key), Some(131_072));
        assert_eq!(set.cfg_of(&key), Some(small_cfg()));
        assert_eq!(set.classes_of(&key), Some([RoleClass::Worker].as_slice()));
        assert!(set.bytes_of(&key).unwrap() > 0);
        assert!(matches!(
            set.state_of(&key),
            Some(SlotState::Loading { .. })
        ));
    }

    #[test]
    fn key_for_class_finds_the_loaded_model_that_serves_it() {
        let (mut set, _bus) = set(8 * GIB);
        let key = ModelKey::new("Qwen/Qwen3-4B", "q4k");
        assert_eq!(set.key_for_class(RoleClass::Worker), None);
        set.begin_load(load_req(key.clone(), RoleClass::Worker))
            .unwrap();
        // Still loading: not yet a candidate.
        assert_eq!(set.key_for_class(RoleClass::Worker), None);
        set.finish_load(&key, None).unwrap();
        assert_eq!(set.key_for_class(RoleClass::Worker), Some(&key));
        assert_eq!(set.key_for_class(RoleClass::Embed), None);
    }

    #[test]
    fn snapshot_reports_a_deterministic_model_order() {
        let (mut set, _bus) = set(8 * GIB);
        set.begin_load(load_req(
            ModelKey::new("Qwen/Qwen3-4B", "q4k"),
            RoleClass::Worker,
        ))
        .unwrap();
        set.begin_load(load_req(
            ModelKey::new("Qwen/Qwen3-Embedding-0.6B", "q8_0"),
            RoleClass::Embed,
        ))
        .unwrap();
        let ids: Vec<String> = set
            .snapshot()
            .models
            .iter()
            .map(|m| m.model_id.clone())
            .collect();
        let mut sorted = ids.clone();
        sorted.sort();
        assert_eq!(ids, sorted);
    }
}
