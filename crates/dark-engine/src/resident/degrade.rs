//! The four-step degradation ladder (task unit `B3`, step 8).
//!
//! When a load does not fit the budget at the size the caller asked for,
//! [`ResidentSet::begin_load`](super::ResidentSet::begin_load) walks this
//! ladder instead of refusing outright:
//!
//! 1. Reduce the requested context.
//! 2. Use a smaller quantisation, when one is on disk.
//! 3. Alias the role class to a smaller class that is already resident.
//! 4. Refuse, and state the remedy.
//!
//! [`climb`] is a pure function of its inputs, so a test drives every rung
//! with no model file and no loaded state.

use dark_contract::{ErrCode, Error, RoleClass};

use super::estimate::{self, ModelConfig};

/// One quantisation available on disk for a model, smallest bits first.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct QuantOption<'a> {
    /// The quantisation name, for example `q4k`.
    pub name: &'a str,
    /// The bits per weight this quantisation uses.
    pub bits: f64,
}

/// What [`climb`] needs to decide how far down the ladder to go.
#[derive(Debug, Clone, Copy)]
pub struct DegradeRequest<'a> {
    /// The model's shape.
    pub cfg: ModelConfig,
    /// The context length the caller originally asked for.
    pub requested_context: u64,
    /// The narrowest context this harness offers instead of refusing.
    pub min_context: u64,
    /// The model's own maximum context length.
    pub max_context: u64,
    /// The quantisation the caller originally asked for.
    pub requested_quant: QuantOption<'a>,
    /// Quantisations on disk, smaller bit width first, `requested_quant`
    /// excluded. Step 2 tries these in order.
    pub smaller_quants_on_disk: &'a [QuantOption<'a>],
    /// A smaller role class already resident, when the caller's class may
    /// alias to one. Step 3 offers this only when it is `Some`.
    pub alias_to_class: Option<RoleClass>,
    /// The memory the resident set has free for this load.
    pub budget_bytes: u64,
}

/// One rung the ladder climbed, in the order [`climb`] tried it.
///
/// Every rung [`climb`] visits appears here, including a rung that did not
/// resolve the load: this is the "report each step" requirement in task
/// unit `B3`.
#[derive(Debug, Clone, PartialEq)]
pub enum Step {
    /// Step 1 found a smaller context, at the originally requested
    /// quantisation, that fits the budget.
    ReducedContext {
        /// The context length that fits.
        context: u64,
    },
    /// Step 1 found no context in `[min_context, requested_context]` that
    /// fits at the requested quantisation.
    ContextReductionFailed,
    /// Step 2 found a smaller quantisation, at some context in range, that
    /// fits the budget.
    SmallerQuantisation {
        /// The quantisation name that fits.
        name: String,
        /// The context length that fits at that quantisation. This may be
        /// smaller than `requested_context`: step 2 re-applies step 1's
        /// search at each candidate quantisation.
        context: u64,
    },
    /// Step 2 found no quantisation on disk that fits, at any context down
    /// to `min_context`.
    NoQuantisationFits,
    /// Step 3 aliased the role class to a smaller class that is already
    /// resident.
    AliasedRoleClass {
        /// The class the request now runs against.
        class: RoleClass,
    },
    /// Step 3 had no smaller class to alias to.
    NoAliasAvailable,
    /// Step 4: every earlier rung failed. The load is refused.
    Refused,
}

/// What the ladder decided, after trying every rung it could.
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    /// The load fits, at `context` tokens and quantisation `quant`.
    Fits {
        /// The quantisation name to load.
        quant: String,
        /// The bits per weight of `quant`.
        bits: f64,
        /// The context length to grant.
        context: u64,
    },
    /// The load does not fit at any context or quantisation, but the
    /// caller's role class may alias to `class`, which is already resident.
    Alias {
        /// The role class to use instead.
        class: RoleClass,
    },
    /// Every rung failed. The load is refused.
    Refuse(Error),
}

/// Climbs the degradation ladder for one load that did not fit at the
/// requested size.
///
/// Returns the [`Outcome`] the ladder reached and the [`Step`]s it visited
/// on the way, oldest first, so a caller can report every step it tried
/// (task unit `B3`, step 8).
#[must_use]
pub fn climb(req: &DegradeRequest<'_>) -> (Outcome, Vec<Step>) {
    let mut steps = Vec::new();

    // Step 1: reduce the requested context, at the requested quantisation.
    if let Some(context) = largest_fitting_context(
        req.cfg,
        req.requested_quant.bits,
        req.budget_bytes,
        req.min_context,
        req.requested_context,
    ) {
        steps.push(Step::ReducedContext { context });
        return (
            Outcome::Fits {
                quant: req.requested_quant.name.to_owned(),
                bits: req.requested_quant.bits,
                context,
            },
            steps,
        );
    }
    steps.push(Step::ContextReductionFailed);

    // Step 2: a smaller quantisation on disk, re-trying the context search
    // at each one.
    for candidate in req.smaller_quants_on_disk {
        if let Some(context) = largest_fitting_context(
            req.cfg,
            candidate.bits,
            req.budget_bytes,
            req.min_context,
            req.requested_context,
        ) {
            steps.push(Step::SmallerQuantisation {
                name: candidate.name.to_owned(),
                context,
            });
            return (
                Outcome::Fits {
                    quant: candidate.name.to_owned(),
                    bits: candidate.bits,
                    context,
                },
                steps,
            );
        }
    }
    steps.push(Step::NoQuantisationFits);

    // Step 3: alias the role class to a smaller, already-resident class.
    if let Some(class) = req.alias_to_class {
        steps.push(Step::AliasedRoleClass { class });
        return (Outcome::Alias { class }, steps);
    }
    steps.push(Step::NoAliasAvailable);

    // Step 4: refuse, with the shortfall in bytes and a remedy.
    steps.push(Step::Refused);
    let needed = estimate::total_bytes(req.cfg, req.requested_context, req.requested_quant.bits);
    let shortfall = needed.saturating_sub(req.budget_bytes);
    let err = Error::new(
        ErrCode::EngineWontFit,
        format!(
            "{} at {} tokens needs {needed} bytes; the budget is {} bytes short by {shortfall} \
             bytes",
            req.requested_quant.name, req.requested_context, req.budget_bytes
        ),
    );
    (Outcome::Refuse(err), steps)
}

/// Returns the largest context in `[min_context, requested_context]` whose
/// total memory, at `bits` bits per weight, fits `budget_bytes`. `None`
/// when even `min_context` does not fit.
fn largest_fitting_context(
    cfg: ModelConfig,
    bits: f64,
    budget_bytes: u64,
    min_context: u64,
    requested_context: u64,
) -> Option<u64> {
    if min_context > requested_context {
        return None;
    }
    let weights = estimate::weights_bytes(cfg.params, bits);
    let context = estimate::granted_context(cfg, weights, budget_bytes, requested_context)?;
    if context >= min_context {
        Some(context)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GIB: u64 = 1024 * 1024 * 1024;

    fn small_model() -> ModelConfig {
        ModelConfig {
            params: 4_000_000_000,
            layers: 36,
            kv_heads: 8,
            head_dim: 128,
        }
    }

    #[test]
    fn step_one_reduces_context_when_that_alone_fits() {
        let req = DegradeRequest {
            cfg: small_model(),
            requested_context: 131_072,
            min_context: 2048,
            max_context: 131_072,
            requested_quant: QuantOption {
                name: "q4k",
                bits: 4.0,
            },
            smaller_quants_on_disk: &[],
            alias_to_class: None,
            // 3 GiB fits the q4k weights (~2 GB) plus a reduced context,
            // but not the full 131072-token request.
            budget_bytes: 3 * GIB,
        };
        let (outcome, steps) = climb(&req);
        // Hand-computed: weights_bytes(4e9, 4.0) = 2_000_000_000;
        // allowance = 3 GiB * 10 / 11 = 2_928_386_792; kv_budget =
        // 928_386_792; per_token = 2*36*8*128*2 = 147_456;
        // 928_386_792 / 147_456 = 6296.
        assert_eq!(steps, vec![Step::ReducedContext { context: 6296 }]);
        match outcome {
            Outcome::Fits { quant, context, .. } => {
                assert_eq!(quant, "q4k");
                assert_eq!(context, 6296);
                assert!(context < 131_072);
                assert!(context >= 2048);
            }
            other => panic!("expected Fits, got {other:?}"),
        }
    }

    #[test]
    fn step_two_falls_back_to_a_smaller_quantisation_on_disk() {
        let req = DegradeRequest {
            cfg: small_model(),
            requested_context: 8192,
            min_context: 2048,
            max_context: 131_072,
            requested_quant: QuantOption {
                name: "q8_0",
                bits: 8.0,
            },
            smaller_quants_on_disk: &[QuantOption {
                name: "q4k",
                bits: 4.0,
            }],
            alias_to_class: None,
            // Just short of the ~4 GB that q8_0 weights need, so step 1
            // fails at every context down to the floor; q4k's ~2 GB does
            // fit with room for a context.
            budget_bytes: 3 * GIB,
        };
        let (outcome, steps) = climb(&req);
        assert_eq!(steps[0], Step::ContextReductionFailed);
        assert_eq!(
            steps[1],
            Step::SmallerQuantisation {
                name: "q4k".to_owned(),
                context: 6296,
            }
        );
        match outcome {
            Outcome::Fits { quant, context, .. } => {
                assert_eq!(quant, "q4k");
                assert_eq!(context, 6296);
            }
            other => panic!("expected Fits at q4k, got {other:?}"),
        }
    }

    #[test]
    fn step_three_aliases_the_role_class_when_nothing_else_fits() {
        let req = DegradeRequest {
            cfg: small_model(),
            requested_context: 8192,
            min_context: 2048,
            max_context: 131_072,
            requested_quant: QuantOption {
                name: "q4k",
                bits: 4.0,
            },
            smaller_quants_on_disk: &[],
            alias_to_class: Some(RoleClass::Worker),
            // Nowhere near enough for a 4B model at any quantisation.
            budget_bytes: 64 * 1024 * 1024,
        };
        let (outcome, steps) = climb(&req);
        assert_eq!(steps[0], Step::ContextReductionFailed);
        assert_eq!(steps[1], Step::NoQuantisationFits);
        assert_eq!(
            steps[2],
            Step::AliasedRoleClass {
                class: RoleClass::Worker
            }
        );
        assert_eq!(
            outcome,
            Outcome::Alias {
                class: RoleClass::Worker
            }
        );
    }

    #[test]
    fn step_four_refuses_with_the_shortfall_in_bytes_and_a_remedy() {
        let req = DegradeRequest {
            cfg: small_model(),
            requested_context: 8192,
            min_context: 2048,
            max_context: 131_072,
            requested_quant: QuantOption {
                name: "q4k",
                bits: 4.0,
            },
            smaller_quants_on_disk: &[],
            alias_to_class: None,
            budget_bytes: 64 * 1024 * 1024,
        };
        let (outcome, steps) = climb(&req);
        assert_eq!(steps.last(), Some(&Step::Refused));
        match outcome {
            Outcome::Refuse(err) => {
                assert_eq!(err.code, ErrCode::EngineWontFit);
                assert!(err.remedy.is_some(), "every refusal states a remedy");
                assert!(
                    err.message.contains("bytes"),
                    "the shortfall must be stated in bytes: {}",
                    err.message
                );
            }
            other => panic!("expected Refuse, got {other:?}"),
        }
    }

    #[test]
    fn every_step_the_ladder_visits_is_reported_in_order() {
        let req = DegradeRequest {
            cfg: small_model(),
            requested_context: 8192,
            min_context: 2048,
            max_context: 131_072,
            requested_quant: QuantOption {
                name: "q8_0",
                bits: 8.0,
            },
            smaller_quants_on_disk: &[QuantOption {
                name: "q4k",
                bits: 4.0,
            }],
            alias_to_class: Some(RoleClass::Worker),
            budget_bytes: 64 * 1024 * 1024,
        };
        let (outcome, steps) = climb(&req);
        // Every rung was visited, in ladder order, even though step 2 was
        // the one that resolved nothing and step 3 is the one that did.
        assert_eq!(
            steps,
            vec![
                Step::ContextReductionFailed,
                Step::NoQuantisationFits,
                Step::AliasedRoleClass {
                    class: RoleClass::Worker
                },
            ]
        );
        assert_eq!(
            outcome,
            Outcome::Alias {
                class: RoleClass::Worker
            }
        );
    }
}
