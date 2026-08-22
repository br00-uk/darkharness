//! A golden test on the token length of each prompt fragment.
//!
//! Prompt growth reduces working space on every turn, since the system
//! prompt sits in the cached prefix. The budget here is an assertion, not
//! a comment: a change that grows a fragment past its budget must fail
//! this test and update the budget deliberately. See task unit `I4`, step 5.

use dark_contract::{Engine, RoleClass};
use dark_engine_fake::FakeEngine;
use dark_qwen::sampling::{BASE_PROMPT, COMPACT_PROMPT, FULL_PROMPT, system_prompt_for};

/// The largest number of tokens [`BASE_PROMPT`] may use.
const BASE_BUDGET: usize = 150;
/// The largest number of tokens [`COMPACT_PROMPT`] may use.
const COMPACT_BUDGET: usize = 80;
/// The largest number of tokens [`FULL_PROMPT`] may use.
const FULL_BUDGET: usize = 220;
/// The largest number of tokens the assembled system prompt may use for a
/// model below 8B parameters (base plus compact).
const COMPACT_PROFILE_BUDGET: usize = 230;
/// The largest number of tokens the assembled system prompt may use for a
/// model of 14B parameters and above (base plus full).
const FULL_PROFILE_BUDGET: usize = 350;

fn tokens(engine: &FakeEngine, text: &str) -> usize {
    engine
        .tokenize(RoleClass::Worker, text)
        .expect("the fake tokenizer never fails")
}

fn engine() -> FakeEngine {
    FakeEngine::with_replies(Vec::<String>::new())
}

#[test]
fn the_base_fragment_stays_inside_its_budget() {
    let engine = engine();
    let count = tokens(&engine, BASE_PROMPT.text);
    assert!(
        count <= BASE_BUDGET,
        "base grew to {count} tokens, budget is {BASE_BUDGET}"
    );
}

#[test]
fn the_compact_fragment_stays_inside_its_budget() {
    let engine = engine();
    let count = tokens(&engine, COMPACT_PROMPT.text);
    assert!(
        count <= COMPACT_BUDGET,
        "compact grew to {count} tokens, budget is {COMPACT_BUDGET}"
    );
}

#[test]
fn the_full_fragment_stays_inside_its_budget() {
    let engine = engine();
    let count = tokens(&engine, FULL_PROMPT.text);
    assert!(
        count <= FULL_BUDGET,
        "full grew to {count} tokens, budget is {FULL_BUDGET}"
    );
}

#[test]
fn the_scout_and_worker_profile_system_prompt_stays_inside_its_budget() {
    // Task unit I1's scout and small-worker profiles (0.6B to 8B) select
    // the compact fragment.
    let engine = engine();
    let prompt = system_prompt_for(4.0);
    let count = tokens(&engine, &prompt);
    assert!(
        count <= COMPACT_PROFILE_BUDGET,
        "compact-profile system prompt grew to {count} tokens, budget is {COMPACT_PROFILE_BUDGET}"
    );
}

#[test]
fn the_architect_profile_system_prompt_stays_inside_its_budget() {
    // Task unit I1's 14B-and-above profiles select the full fragment.
    let engine = engine();
    let prompt = system_prompt_for(32.0);
    let count = tokens(&engine, &prompt);
    assert!(
        count <= FULL_PROFILE_BUDGET,
        "full-profile system prompt grew to {count} tokens, budget is {FULL_PROFILE_BUDGET}"
    );
}
