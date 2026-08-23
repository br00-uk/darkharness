//! Task unit `E7`. "Done when: the sub-agent count respects a synthetic
//! low-memory state."
//!
//! Do step 11: "Limit parallel sub-agents. Read the headroom from the
//! resident set. The default is 2." Do not step: "Do not start eight
//! research sub-agents. That exhausts memory."

use dark_contract::ResidencySnapshot;
use dark_plan::work::{DEFAULT_SUBAGENT_LIMIT, research_parallelism};

fn synthetic_residency(budget_bytes: u64, used_bytes: u64) -> ResidencySnapshot {
    ResidencySnapshot {
        budget_bytes,
        used_bytes,
        models: Vec::new(),
    }
}

#[test]
fn a_healthy_machine_gets_the_default_of_two() {
    let plenty = synthetic_residency(48_000_000_000, 20_000_000_000); // 28 GB headroom
    assert_eq!(research_parallelism(&plenty, 2_000_000_000), 2);
}

#[test]
fn a_synthetic_low_memory_state_drops_the_count_below_the_default() {
    // Only 1.2 GB of headroom left; each sub-agent's key-value cache
    // costs an estimated 2 GB. Not even one fits.
    let starved = synthetic_residency(20_000_000_000, 18_800_000_000);
    let count = research_parallelism(&starved, 2_000_000_000);

    assert!(
        count < DEFAULT_SUBAGENT_LIMIT,
        "a starved resident set must report fewer than the default 2, got {count}"
    );
    assert_eq!(count, 0);
}

#[test]
fn headroom_for_exactly_one_agent_reports_one_not_the_default() {
    let tight = synthetic_residency(20_000_000_000, 18_000_000_000); // 2 GB headroom
    assert_eq!(research_parallelism(&tight, 2_000_000_000), 1);
}

#[test]
fn no_headroom_at_all_reports_zero_sub_agents() {
    let empty = synthetic_residency(10_000_000_000, 10_000_000_000);
    assert_eq!(research_parallelism(&empty, 2_000_000_000), 0);
}

#[test]
fn vast_headroom_never_exceeds_the_default_cap() {
    // Do not: "Do not start eight research sub-agents." Even headroom
    // that could fit eight must report at most the default of two.
    let vast = synthetic_residency(200_000_000_000, 0);
    assert_eq!(
        research_parallelism(&vast, 2_000_000_000),
        DEFAULT_SUBAGENT_LIMIT
    );
}
