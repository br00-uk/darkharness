//! Task unit `B4`'s acceptance test: 1000 cancelled turns return memory to
//! the baseline.
//!
//! This crate has no model file and no accelerator to measure real
//! key-value cache memory against, so this test is the honest substitute
//! the module documentation on `crates/dark-engine/src/resident/mod.rs`
//! promises: it drives the resident set's own turn-lease accounting and
//! the concurrency limiter's permit accounting — the two pieces of state
//! `crates/dark-engine/src/stream/live.rs`'s `Guard` releases on every
//! code path, cancellation included — through 1000 acquire-then-cancel
//! cycles, and asserts both are back exactly where they started. What a
//! run on real hardware would add: the same 1000 cycles against a live
//! `mistralrs::Model`, with the process's resident set size measured
//! before and after, to confirm mistral.rs's own key-value cache
//! allocator (not just this crate's bookkeeping of it) releases the block
//! for a genuinely cancelled sequence.

use dark_contract::{EventBus, RoleClass};
use dark_engine::resident::{
    BeginLoadRequest, ModelConfig, ModelKey, QuantOption, ResidentSet, TurnId,
};
use dark_engine::stream::concurrency::Limiter;

const GIB: u64 = 1024 * 1024 * 1024;

fn small_model_config() -> ModelConfig {
    ModelConfig {
        params: 4_000_000_000,
        layers: 36,
        kv_heads: 8,
        head_dim: 128,
    }
}

#[test]
fn one_thousand_cancelled_turns_return_leases_and_permits_to_baseline() {
    let bus = EventBus::new();
    let mut resident = ResidentSet::new(8 * GIB, bus.tx());
    let key = ModelKey::new("Qwen/Qwen3-4B", "q4k");

    resident
        .begin_load(BeginLoadRequest {
            key: key.clone(),
            cfg: small_model_config(),
            classes: vec![RoleClass::Worker],
            requested_quant: QuantOption {
                name: "q4k",
                bits: 4.0,
            },
            smaller_quants_on_disk: &[],
            requested_context: 8192,
            max_context: 131_072,
            alias_to_class: None,
        })
        .expect("a 4B model at q4k fits comfortably in an 8 GiB budget");
    resident
        .finish_load(&key, None)
        .expect("the slot begin_load just created is in the Loading state");

    let baseline_leases = resident.outstanding_leases();
    let baseline_used_bytes = resident.used_bytes();
    assert_eq!(baseline_leases, 0, "no turn has started yet");

    let limiter = Limiter::new(4);
    let baseline_permits = limiter.available();

    for i in 0..1_000 {
        let turn = TurnId::new(format!("turn-{i}"));

        let permit = limiter
            .try_acquire()
            .expect("the limiter always has room once the previous iteration released its permit");
        resident
            .acquire_turn_lease(turn.clone(), key.clone())
            .expect(
                "the model stays Loaded across every iteration: it is never evicted while leased",
            );
        assert_eq!(resident.outstanding_leases(), 1);
        assert_eq!(limiter.available(), baseline_permits - 1);

        // Simulate cancellation: the turn ends without completing, exactly
        // as `stream::live::Guard::drop` releases both regardless of why
        // the stream ended.
        resident.release_turn(&turn);
        drop(permit);
    }

    assert_eq!(
        resident.outstanding_leases(),
        baseline_leases,
        "1000 cancelled turns must return the lease count to baseline"
    );
    assert_eq!(
        limiter.available(),
        baseline_permits,
        "1000 cancelled turns must return the concurrency permits to baseline"
    );
    assert_eq!(
        resident.used_bytes(),
        baseline_used_bytes,
        "the resident model's own memory is untouched by turn leases coming and going"
    );
}
