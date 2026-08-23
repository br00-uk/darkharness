//! Deterministic generation (task unit `B7`).
//!
//! [`plan`] reads a [`dark_contract::Request`] and decides what this crate
//! does differently when [`dark_contract::Request::deterministic`] is
//! `true`. [`apply`] does the same, and also mutates the sampling settings
//! in place, so a caller that just wants the request fixed up can call it
//! directly.
//!
//! # The limit: one build, one device
//!
//! Reproducibility holds for one build of the engine, on one device. It
//! does not hold across devices (CPU and CUDA sum in a different order) or
//! across engine versions (a new mistral.rs release may change kernel
//! selection). [`REPRODUCIBILITY_LIMIT`] states this for a caller that
//! surfaces it to a person, for example in `dark doctor`.
//!
//! # Where this diverges from a per-request seed
//!
//! The build specification's step 3 for this task unit is "apply the
//! seed". mistral.rs 0.8.1's public API has no per-request seed: its
//! `Engine` seeds one process-wide random generator once, from a fixed
//! constant, at start-up (`mistralrs_core::engine::SEED`), and every
//! request after that draws from the same stream. [`dark_contract::Sampling::seed`]
//! is therefore accepted for API compatibility with the contract, but this
//! crate does not forward it to mistral.rs as a literal RNG seed — there is
//! nowhere in the public API to forward it to.
//!
//! This is not a gap in practice. [`apply`] forces `top_k = Some(1)`
//! (greedy decoding) whenever `deterministic` is `true`. With exactly one
//! candidate token at each step, the sampler never consults the random
//! generator at all — whatever it would have drawn changes nothing, because
//! there is only one token to pick. Determinism under this plan comes from
//! the sampling policy having no randomness left to seed, not from the seed
//! value itself. See `docs/adr/0006` for the full record of this
//! divergence.

use dark_contract::Request;

/// States the scope of the guarantee task unit `B7` establishes.
///
/// A caller that reports determinism to a person — `dark doctor`, an ADR, a
/// test's failure message — should quote this rather than restate it, so
/// the wording stays in one place.
pub const REPRODUCIBILITY_LIMIT: &str = "Reproducibility holds for one build of the engine, on one device. It does not hold across \
     devices or across engine versions.";

/// What [`plan`] decided for one request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Plan {
    /// Force `top_k = 1` (greedy decoding). Section 4.1's KV cache formula
    /// is unaffected: this changes only which token the sampler picks, not
    /// how much memory the request needs.
    pub greedy: bool,
    /// Run this request with no other sequence sharing the engine's batch.
    ///
    /// Batched inference lets a GPU kernel sum contributions from several
    /// sequences in one pass; the order those contributions combine in can
    /// depend on which other sequences share the batch, and floating-point
    /// addition is not associative. Step 1 of this task unit ("set the
    /// batch size to 1") is this: the caller — [`crate::stream`]'s
    /// concurrency limiter — must not start another sequence while a
    /// request with `run_exclusively` set is in flight.
    pub run_exclusively: bool,
    /// Do not enable mistral.rs's `PagedAttention` for this request.
    ///
    /// `PagedAttention` batches attention across sequences in fixed-size
    /// blocks, which is exactly the kind of cross-sequence batching
    /// `run_exclusively` already guards against; leaving it enabled would
    /// undermine that guard from inside the attention kernel itself. This
    /// crate's model loader (`B2`) never enables `PagedAttention` at all
    /// (see `docs/adr/0006`), so this field exists to make that choice
    /// explicit at the request level too, for a caller that builds its own
    /// request path.
    pub disable_paged_attention: bool,
}

impl Plan {
    /// The plan for a request that does not ask for determinism: nothing
    /// changes.
    const NEUTRAL: Self = Self {
        greedy: false,
        run_exclusively: false,
        disable_paged_attention: false,
    };

    /// The plan for a request that asks for determinism.
    const DETERMINISTIC: Self = Self {
        greedy: true,
        run_exclusively: true,
        disable_paged_attention: true,
    };
}

/// Decides what this crate does differently for `request`, without
/// changing it.
///
/// Returns [`Plan::NEUTRAL`] when [`Request::deterministic`] is `false`,
/// [`Plan::DETERMINISTIC`] otherwise.
#[must_use]
pub fn plan(request: &Request) -> Plan {
    if request.deterministic {
        Plan::DETERMINISTIC
    } else {
        Plan::NEUTRAL
    }
}

/// Applies [`plan`]'s decision to `request`'s sampling settings, and
/// returns the plan.
///
/// When `request.deterministic` is `true`, this sets `sampling.top_k` to
/// `Some(1)`, so decoding is greedy (see the module documentation for why
/// that is what actually delivers determinism, given mistral.rs 0.8.1 has
/// no per-request seed). It leaves every other sampling field as the
/// caller set it: temperature, penalties, and `seed` are deterministic
/// functions of their own inputs, so none of them reintroduce randomness
/// once only one candidate token survives.
///
/// A non-deterministic request is returned unchanged.
pub fn apply(request: &mut Request) -> Plan {
    let outcome = plan(request);
    if outcome.greedy {
        request.sampling.top_k = Some(1);
    }
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;
    use dark_contract::{Message, Role, RoleClass};

    fn request(deterministic: bool) -> Request {
        let mut req = Request::new(RoleClass::Worker, vec![Message::text(Role::User, "hi")]);
        req.deterministic = deterministic;
        req
    }

    #[test]
    fn a_non_deterministic_request_gets_the_neutral_plan() {
        let plan = plan(&request(false));
        assert!(!plan.greedy);
        assert!(!plan.run_exclusively);
        assert!(!plan.disable_paged_attention);
    }

    #[test]
    fn a_deterministic_request_gets_the_deterministic_plan() {
        let plan = plan(&request(true));
        assert!(plan.greedy);
        assert!(plan.run_exclusively);
        assert!(plan.disable_paged_attention);
    }

    #[test]
    fn apply_forces_top_k_to_one_for_a_deterministic_request() {
        let mut req = request(true);
        req.sampling.top_k = Some(40);
        apply(&mut req);
        assert_eq!(req.sampling.top_k, Some(1));
    }

    #[test]
    fn apply_leaves_a_non_deterministic_request_unchanged() {
        let mut req = request(false);
        req.sampling.top_k = Some(40);
        let before = req.clone();
        apply(&mut req);
        assert_eq!(req, before);
    }

    #[test]
    fn apply_preserves_temperature_and_seed_alongside_greedy_decoding() {
        // These fields are inert once top_k = 1, but apply must not erase
        // them: a caller may still want them recorded (in a transcript, for
        // example) even though the sampler never consults them.
        let mut req = request(true);
        req.sampling.temperature = Some(0.7);
        req.sampling.seed = Some(42);
        apply(&mut req);
        assert_eq!(req.sampling.temperature, Some(0.7));
        assert_eq!(req.sampling.seed, Some(42));
        assert_eq!(req.sampling.top_k, Some(1));
    }

    #[test]
    fn applying_twice_is_idempotent() {
        let mut req = request(true);
        apply(&mut req);
        let once = req.clone();
        apply(&mut req);
        assert_eq!(req, once);
    }

    #[test]
    fn ten_applications_with_the_same_seed_agree_with_each_other() {
        // Stands in for the task unit's "ten runs with the same seed
        // produce identical output" acceptance test: with no model loaded,
        // this crate cannot generate ten real completions, but it can show
        // that the plan this module hands to the engine is the same input
        // every time for the same request, which is the precondition a
        // real run's determinism depends on.
        let mut outputs = Vec::new();
        for _ in 0..10 {
            let mut req = request(true);
            req.sampling.seed = Some(7);
            let outcome = apply(&mut req);
            outputs.push((req, outcome));
        }
        assert!(outputs.windows(2).all(|pair| pair[0] == pair[1]));
    }

    #[test]
    fn reproducibility_limit_names_both_device_and_version() {
        assert!(REPRODUCIBILITY_LIMIT.contains("device"));
        assert!(REPRODUCIBILITY_LIMIT.contains("version"));
    }
}
