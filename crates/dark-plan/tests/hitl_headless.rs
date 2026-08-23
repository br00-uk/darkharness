//! Task unit `E7`. "Done when: headless work on a grilling ticket returns
//! `E_HITL_REQUIRES_HUMAN`."
//!
//! An integration test, not a unit test in `src/work.rs`, because it
//! exercises the module's public surface the way a caller (`dark-core`)
//! would: select a ticket off a frontier, then try to route it with no
//! human present, the same two calls `/plan work` makes in sequence.

use dark_plan::chart::TicketKind;
use dark_plan::work::{WorkTicket, route, select_ticket};

fn frontier() -> Vec<WorkTicket> {
    vec![
        WorkTicket {
            id: "T-004".to_owned(),
            name: "Pack identity is content-addressed".to_owned(),
            question: "How is a pack identified?".to_owned(),
            ticket_type: TicketKind::Research,
            hitl: false,
            ordinal: 0,
        },
        WorkTicket {
            id: "T-018".to_owned(),
            name: "Pack staleness policy".to_owned(),
            question: "How does a pack declare its staleness policy?".to_owned(),
            ticket_type: TicketKind::Grilling,
            hitl: true,
            ordinal: 1,
        },
    ]
}

#[test]
fn headless_work_on_a_grilling_ticket_returns_hitl_requires_human() {
    let frontier = frontier();
    let ticket = select_ticket(Some("T-018"), &frontier).expect("T-018 is on the frontier");

    let err = route(ticket, false).expect_err("no human is present");

    assert_eq!(err.code, dark_contract::ErrCode::HitlRequiresHuman);
    assert!(err.message.contains("T-018"));
    assert!(
        err.remedy.is_some(),
        "every error carries a remedy (CLAUDE.md conventions)"
    );
}

#[test]
fn headless_work_on_the_frontiers_research_ticket_still_routes() {
    let frontier = frontier();
    // No name given: the first frontier ticket by ordinal is T-004
    // (research), not T-018 (grilling) — Rule 21 ("`/plan --headless`
    // creates, wires, claims, and resolves research tickets only") holds
    // even when a headless caller never names a ticket at all.
    let ticket = select_ticket(None, &frontier).expect("the frontier is not empty");
    assert_eq!(ticket.id, "T-004");

    let method = route(ticket, false).expect("a research ticket needs no human");
    assert_eq!(method, dark_plan::work::WorkMethod::Research);
}

#[test]
fn a_task_ticket_with_hitl_set_by_hand_still_needs_a_human() {
    // Do step 5's routing table: "grilling and anything with hitl set
    // needs a person" — the kind alone is not the whole rule.
    let ticket = WorkTicket {
        id: "T-042".to_owned(),
        name: "Ship the retry policy".to_owned(),
        question: "Does the retry policy ship as written?".to_owned(),
        ticket_type: TicketKind::Task,
        hitl: true,
        ordinal: 0,
    };

    let err = route(&ticket, false).expect_err("hitl is set, so a human must be present");
    assert_eq!(err.code, dark_contract::ErrCode::HitlRequiresHuman);
}
