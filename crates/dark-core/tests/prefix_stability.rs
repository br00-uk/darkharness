//! `A3` done-when: "The first N tokens are identical across five round-trips
//! in one turn."
//!
//! This test assembles the prefix once, the way a turn loop does at the
//! start of a turn (Rule 5), then simulates five round trips within that
//! same turn — each one landing one more tool result, the way a multi-step
//! tool-calling turn grows its tail (Rule 8). Every round it re-assembles
//! the prefix from the same inputs and checks the result against the first
//! round's, both as serialized bytes and as a real tokenizer count, and it
//! checks that the prefix bytes at the front of the full request never move
//! once the tail is appended.

use dark_contract::{Message, Role, RoleClass};
use dark_core::context::{
    PrefixInputs, PrefixTracker, TailInputs, assemble_prefix, assemble_tail, count_message_tokens,
};
use dark_engine_fake::FakeEngine;

#[tokio::test]
async fn the_first_n_tokens_are_identical_across_five_round_trips() {
    let engine = FakeEngine::with_replies(Vec::<String>::new());

    let inputs = PrefixInputs {
        system_prompt: "you are dark, a local coding harness.",
        agents_chain: "root AGENTS.md rules\nnested crates/dark-core/AGENTS.md rules",
        environment_date: "2026-08-22",
        map_digest: Some("42 nodes, 3 blocked edges, frontier: T-014, T-019"),
        ticket_body: Some("T-014: wire the resident set manager to Caps::granted_context"),
    };

    // Rule 5: assemble the prefix once, at the start of the turn, and reuse
    // it. A turn loop that calls a bus for a prefix-changed notice would use
    // PrefixTracker the same way this test does, though a fresh tracker on
    // its first observation never fires one (nothing to compare against).
    let mut tracker = PrefixTracker::new();
    let bus = dark_contract::EventBus::new();
    let mut events_rx = bus.subscribe();
    tracker.observe(&inputs, &bus.tx());

    let prefix = assemble_prefix(&inputs);
    let prefix_messages = prefix.messages();
    let prefix_json = serde_json::to_vec(&prefix_messages).expect("prefix messages serialize");
    let prefix_tokens = count_message_tokens(&engine, RoleClass::Worker, &prefix_messages)
        .expect("tokenize the prefix");

    assert_eq!(
        prefix_messages.len(),
        5,
        "every one of the five sections should be present"
    );

    let history = vec![Message::text(
        Role::User,
        "earlier turn: renamed dark-cli to dark",
    )];
    let input = Message::text(Role::User, "fix the flaky retry test in dark-airlock");

    for round in 0..5usize {
        // Each round trip lands one more tool result, the way a multi-step
        // tool-calling turn grows. The prefix must not react to this at all.
        let tool_results: Vec<Message> = (0..=round)
            .map(|i| Message::tool_reply(format!("call-{i}"), format!("tool result body {i}")))
            .collect();

        let tail_inputs = TailInputs {
            tool_schemas: &[],
            lexicon_chunks: &[],
            history: &history,
            input: &input,
            tool_results: &tool_results,
        };
        let tail = assemble_tail(&tail_inputs);

        // A turn loop re-reading its inputs mid-turn (for example to log
        // them) must still get byte-identical prefix content back:
        // assemble_prefix is pure.
        let round_prefix = assemble_prefix(&inputs);
        let round_prefix_messages = round_prefix.messages();
        let round_prefix_json =
            serde_json::to_vec(&round_prefix_messages).expect("prefix messages serialize");
        assert_eq!(
            round_prefix_json, prefix_json,
            "round {round}: assemble_prefix produced different bytes for the same inputs"
        );

        let round_prefix_tokens =
            count_message_tokens(&engine, RoleClass::Worker, &round_prefix_messages)
                .expect("tokenize the prefix");
        assert_eq!(
            round_prefix_tokens, prefix_tokens,
            "round {round}: the prefix's real token count changed"
        );

        // The observed hash must also track: same inputs, no notice.
        tracker.observe(&inputs, &bus.tx());
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(20), events_rx.recv())
                .await
                .is_err(),
            "round {round}: an unchanged prefix must not fire a change notice"
        );

        // The full request a turn loop would send: prefix, then tail. The
        // first prefix_messages.len() entries of that request must stay
        // byte-identical to the standalone prefix, every round, even as the
        // tail after it grows by one more tool result each time.
        let mut full_request = round_prefix_messages.clone();
        full_request.extend(tail.messages());
        let leading_slice = &full_request[..prefix_messages.len()];
        let leading_json = serde_json::to_vec(leading_slice).expect("leading slice serializes");
        assert_eq!(
            leading_json, prefix_json,
            "round {round}: the prefix bytes at the front of the request moved once the tail grew"
        );
    }
}
