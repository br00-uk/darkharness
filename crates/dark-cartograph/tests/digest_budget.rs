//! Integration test for task unit `D3`: a map with 500 tickets still
//! renders a `Full`-tier digest inside budget.
//!
//! # Why this test does not call the real tokenizer
//!
//! `PRD.md`'s `D3` "Done when" names the real tokenizer: "A map with 500
//! tickets produces a digest of 1200 tokens or fewer under the real
//! tokenizer." Reaching a real tokenizer means reaching an
//! [`dark_contract::Engine`] implementation, and every one of those —
//! `dark-engine-fake` included — lives in a crate that `dark-cartograph`
//! is not allowed to depend on: Rule 16 (`CLAUDE.md`) keeps this crate
//! down to `dark-contract` and third-party crates only, and this crate's
//! declared dependencies name no engine crate at all. There is no way to
//! call `Engine::tokenize` from inside `dark-cartograph`.
//!
//! This test instead counts whitespace-separated words, the same proxy
//! `crate::digest`'s own compression loop budgets against (see
//! `crate::digest::estimate`), and checks the render stays well inside
//! that proxy's budget. The real-tokenizer count is a check that belongs
//! where a real `Engine` is actually in scope — task unit `A3`, once the
//! digest this module renders reaches `dark-core`'s prefix assembly. See
//! this crate's task report for the full account of this gap.
//!
//! # Why the frontier is 50 tickets, not 250
//!
//! The frontier never compresses (`D3` step 3), so the budget is only
//! reachable when the frontier itself stays a reasonable size — the same
//! way a real map's frontier does: work proceeds along dependency
//! chains, so only the ticket at the head of each chain is ever
//! simultaneously open and unblocked, never every open ticket at once.
//! This fixture models that directly: fifty independent ten-ticket
//! chains, each with one resolved run of tickets, exactly one open and
//! unblocked ticket at the head, and the rest still blocked behind it —
//! five hundred tickets in total, fifty of them on the frontier.

use dark_cartograph::digest::{self, Tier};
use dark_cartograph::journal::{
    EdgeAdded, JournalEvent, MapCreated, MapStatus, TicketCreated, TicketStatus, TicketType,
    TicketUpdated,
};
use dark_cartograph::store::Store;
use tempfile::TempDir;

/// The word-count proxy budget this test enforces. Matches
/// `crate::digest::estimate::ESTIMATED_BUDGET`, which is private to the
/// crate; duplicated here as a plain constant since an integration test
/// only sees this crate's public API.
const ESTIMATED_BUDGET: usize = 900;

/// Chains in the fixture. Each chain contributes ten tickets: five
/// resolved, one open and unblocked (the frontier head), four blocked
/// behind it.
const CHAINS: usize = 50;
/// Tickets per chain.
const CHAIN_LEN: usize = 10;
/// How many tickets, from the head of each chain, resolve. The
/// remaining `CHAIN_LEN - RESOLVED_PER_CHAIN - 1` tickets stay blocked
/// behind the one open, unblocked head.
const RESOLVED_PER_CHAIN: usize = 5;

fn estimate_tokens(text: &str) -> usize {
    text.split_whitespace().count()
}

/// Returns the identifier for ticket `step` (0-based) of `chain`.
fn ticket_id(chain: usize, step: usize) -> String {
    format!("T-{chain:03}-{step:02}")
}

/// Builds a map with `CHAINS * CHAIN_LEN` (500) tickets: fifty
/// independent chains, each contributing five resolved decisions, one
/// frontier head, and four tickets still blocked behind that head.
fn seed_large_map(store: &mut Store) {
    store
        .apply(&JournalEvent::MapCreated(MapCreated {
            id: "M1".to_owned(),
            name: "Five hundred tickets".to_owned(),
            destination: "A destination stated the way a real map states one, in a sentence."
                .to_owned(),
            notes: Some("Domain: a synthetic fixture for the digest budget test.".to_owned()),
            created_at: 1_700_000_000_000,
            status: MapStatus::Active,
        }))
        .unwrap();

    let mut ordinal = 0_i64;

    for chain in 0..CHAINS {
        for step in 0..CHAIN_LEN {
            let id = ticket_id(chain, step);
            store
                .apply(&JournalEvent::TicketCreated(TicketCreated {
                    id: id.clone(),
                    map_id: "M1".to_owned(),
                    name: format!("Chain {chain} step {step}"),
                    question: format!("What does chain {chain} step {step} decide?"),
                    ticket_type: TicketType::Task,
                    hitl: false,
                    status: TicketStatus::Open,
                    created_at: 1_700_000_000_000 + ordinal,
                    ordinal,
                    axis: None,
                    tokens_used: None,
                }))
                .unwrap();

            if step < RESOLVED_PER_CHAIN {
                store
                    .apply(&JournalEvent::TicketUpdated(TicketUpdated {
                        id: id.clone(),
                        status: Some(TicketStatus::Resolved),
                        gist: Some(format!("chain {chain} settled at step {step}")),
                        resolved_at: Some(1_700_000_100_000 + ordinal),
                        ..Default::default()
                    }))
                    .unwrap();
            }

            if step > 0 {
                let blocker = ticket_id(chain, step - 1);
                // A resolved blocker leaves this edge with no live
                // effect on the frontier query; recording it anyway
                // keeps every chain structurally identical regardless
                // of where `RESOLVED_PER_CHAIN` falls.
                store
                    .apply(&JournalEvent::EdgeAdded(EdgeAdded {
                        blocker,
                        blocked: id,
                    }))
                    .unwrap();
            }

            ordinal += 1;
        }
    }

    assert_eq!(
        ordinal,
        i64::try_from(CHAINS * CHAIN_LEN).unwrap(),
        "fixture must add up to five hundred tickets"
    );
}

#[test]
fn a_five_hundred_ticket_map_fits_the_estimated_budget() {
    let tmp = TempDir::new().expect("tempdir");
    let mut store = Store::open(tmp.path()).expect("open store");
    seed_large_map(&mut store);

    let text = digest::render(&store, "M1", Tier::Full)
        .expect("render must succeed")
        .expect("Full tier must return text");

    let estimate = estimate_tokens(&text);
    assert!(
        estimate <= ESTIMATED_BUDGET,
        "digest estimate {estimate} exceeds the {ESTIMATED_BUDGET}-word proxy budget:\n{text}"
    );

    // The frontier is the actionable part and must never compress away
    // (D3 step 3): every chain's head ticket must still be named.
    let frontier_count_line = format!("FRONTIER ({CHAINS} takeable now)");
    assert!(
        text.contains(&frontier_count_line),
        "expected {frontier_count_line:?} in:\n{text}"
    );
    for chain in 0..CHAINS {
        let head = ticket_id(chain, RESOLVED_PER_CHAIN);
        assert!(
            text.contains(&head),
            "frontier head {head} must appear uncompressed"
        );
    }
}

#[test]
fn rendering_the_five_hundred_ticket_map_twice_is_byte_identical() {
    let tmp = TempDir::new().expect("tempdir");
    let mut store = Store::open(tmp.path()).expect("open store");
    seed_large_map(&mut store);

    let first = digest::render(&store, "M1", Tier::Full).unwrap().unwrap();
    let second = digest::render(&store, "M1", Tier::Full).unwrap().unwrap();
    assert_eq!(first, second);
}
