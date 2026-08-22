//! `A3` done-when: "A 200-turn session stays inside the budget and keeps
//! all pinned content."
//!
//! This test simulates a 200-turn session: each turn appends a user message
//! and an assistant reply, long enough that the working-space budget (Do
//! step 4: compact at 75% of `granted_context`, Rule 4: budget against
//! `Caps::granted_context`) trips more than once over the run. A few turns
//! also pin a "decision" message. After every turn the test checks the
//! session for the compaction threshold and, when it is over, folds the
//! oldest third of unpinned history through the scout role (Do step 5). At
//! the end it checks that the session stayed inside `granted_context` and
//! that every pinned message survived, verbatim.

use std::collections::HashSet;

use dark_contract::{Engine, EventBus, Message, Role, RoleClass};
use dark_core::context::{
    apply_summary, build_summary_request, count_message_tokens, select_fold_range, should_compact,
};
use dark_engine_fake::FakeEngine;
use futures_util::StreamExt;

const TURNS: usize = 200;

/// Runs one scout request to completion and returns its visible text.
///
/// `context::compact` builds the request but never sends it (see that
/// module's documentation): consuming the stream is the turn loop's job,
/// which is exactly what this test stands in for.
async fn run_scout_summary(engine: &FakeEngine, request: dark_contract::Request) -> String {
    let mut stream = engine
        .stream(request, tokio_util::sync::CancellationToken::new())
        .await
        .expect("the scripted scout turn is available");

    let mut text = String::new();
    while let Some(chunk) = stream.next().await {
        if let dark_contract::Chunk::Text(part) = chunk.expect("the scripted turn does not fail") {
            text.push_str(&part);
        }
    }
    text
}

#[tokio::test]
async fn a_200_turn_session_stays_inside_the_budget_and_keeps_pinned_content() {
    // Every compaction asks the scout role for a summary; script enough
    // replies that the cursor never runs out no matter how often the loop
    // below trips the threshold.
    let summary_text =
        "Folded turns: files changed, decisions made, errors met, and work remaining are kept.";
    let engine = FakeEngine::with_replies(vec![summary_text.to_owned(); TURNS]);
    let bus = EventBus::new();
    let events = bus.tx();

    let granted_context = 32_000_usize;
    let class = RoleClass::Worker;

    let mut history: Vec<Message> = Vec::new();
    let mut pinned_texts: Vec<String> = Vec::new();
    let mut compactions = 0_usize;

    for turn in 0..TURNS {
        // A long-enough pair of messages that the budget actually gets
        // pressured well before turn 200, so compaction has real work to do
        // rather than never firing.
        let filler = "change ".repeat(80);
        history.push(Message::text(
            Role::User,
            format!("turn {turn} request: {filler}"),
        ));
        history.push(Message::text(
            Role::Assistant,
            format!("turn {turn} reply: {filler}"),
        ));

        // Pin a decision message every 25 turns. These must never be folded
        // and must still be present, verbatim, at the end of the run.
        if turn % 25 == 0 {
            let decision = format!("decision at turn {turn}: keep the fake engine for CI");
            history.push(Message::text(Role::System, decision.clone()).pinned());
            pinned_texts.push(decision);
        }

        // Rule 7: compact only at a turn boundary. This loop iteration is
        // that boundary for `turn`.
        let used = count_message_tokens(&engine, class, &history).expect("tokenize the history");
        if should_compact(used, granted_context) {
            if let Some(selection) = select_fold_range(&history) {
                let request = build_summary_request(&history, &selection);
                assert_eq!(request.class, RoleClass::Scout);
                let summary = run_scout_summary(&engine, request).await;
                history = apply_summary(&history, &selection, &summary, &events);
                compactions += 1;
            }
        }
    }

    let final_tokens =
        count_message_tokens(&engine, class, &history).expect("tokenize the final history");

    assert!(
        compactions > 0,
        "the 200-turn run never crossed the compaction threshold; \
         the test fixture no longer exercises compaction"
    );
    assert!(
        final_tokens <= granted_context,
        "the session ended over its granted context: {final_tokens} > {granted_context}"
    );

    // Every pinned message from every turn survived, in full, somewhere in
    // the final history — compaction never folds a pinned message, and
    // nothing else in this test removes one either.
    let pinned_survivors: HashSet<String> = history
        .iter()
        .filter(|message| message.pinned)
        .map(Message::text_content)
        .collect();

    for decision in &pinned_texts {
        assert!(
            pinned_survivors.contains(decision),
            "lost a pinned decision: {decision:?}"
        );
    }

    // And every message that carries a pinned decision's exact text is
    // still marked pinned: compaction and summarising never silently drop
    // the flag while leaving the text behind.
    for message in &history {
        if pinned_texts.contains(&message.text_content()) {
            assert!(message.pinned, "a pinned decision lost its pinned flag");
        }
    }
}
