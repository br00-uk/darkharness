//! Proves that no outbound request carries a reasoning field.
//!
//! Task unit `I2`, step 5: thinking text never travels back to a model.
//! This test builds a message history that carries reasoning from a past
//! turn, runs it through [`dark_qwen::think::prepare_outbound`], sends the
//! result through [`FakeEngine`], and inspects every request the engine
//! received.
//!
//! `dark-qwen` depends on no streaming crate of its own, so this test never
//! names `tokio_util` or `futures_util` directly: `Default::default()`
//! resolves to a `CancellationToken` by inference from
//! [`dark_contract::Engine::stream`]'s signature, and the request is
//! recorded before the returned stream is ever polled, so nothing here
//! needs to drain it.

use dark_contract::{Engine, Message, Request, Role, RoleClass};
use dark_engine_fake::FakeEngine;
use dark_qwen::think::prepare_outbound;

#[tokio::test]
async fn no_outbound_request_carries_a_reasoning_field() {
    let engine = FakeEngine::with_replies(["ok"]);

    let history_with_reasoning = vec![
        Message::text(Role::User, "explain the seam"),
        Message {
            reasoning: Some("I should look at the call graph first...".to_owned()),
            ..Message::text(Role::Assistant, "The seam is bounded to this module.")
        },
        Message::text(Role::User, "and now?"),
    ];

    // Sanity check: the fixture really does carry a reasoning field before
    // preparation, so the test is not vacuously true.
    assert!(history_with_reasoning.iter().any(|m| m.reasoning.is_some()));

    let prepared = prepare_outbound(history_with_reasoning);
    assert!(prepared.iter().all(|m| m.reasoning.is_none()));

    let request = Request::new(RoleClass::Worker, prepared);
    // Recording happens inside `stream` before it returns, so awaiting the
    // call is enough; nothing here needs to poll the resulting stream.
    // `dark-qwen` has no dependency of its own on `tokio_util`, so the
    // cancellation token is built through inference rather than named
    // directly; that is exactly what `clippy::default_trait_access` warns
    // against in the ordinary case, so it is silenced here deliberately.
    #[allow(clippy::default_trait_access)]
    let _stream = engine
        .stream(request, Default::default())
        .await
        .expect("stream starts");

    let seen = engine.seen_requests();
    assert_eq!(seen.len(), 1);
    assert!(
        seen.iter()
            .all(|req| req.messages.iter().all(|m| m.reasoning.is_none())),
        "an outbound request carried a reasoning field"
    );
}

#[tokio::test]
async fn a_multi_turn_conversation_never_leaks_reasoning_across_turns() {
    let engine = FakeEngine::with_replies(["first", "second", "third"]);
    let mut history = Vec::new();

    for turn_text in ["one", "two", "three"] {
        history.push(Message::text(Role::User, turn_text));

        let request = Request::new(RoleClass::Worker, prepare_outbound(history.clone()));
        #[allow(clippy::default_trait_access)]
        let _stream = engine
            .stream(request, Default::default())
            .await
            .expect("stream starts");

        // Simulate the harness lifting a <think> block, if there were one,
        // into Message::reasoning on the stored history turn, without
        // depending on a stream-draining crate to read the real reply text.
        history.push(Message {
            reasoning: Some(format!("thinking about {turn_text}")),
            ..Message::text(Role::Assistant, format!("reply to {turn_text}"))
        });
    }

    let seen = engine.seen_requests();
    assert_eq!(seen.len(), 3);
    for request in &seen {
        assert!(
            request.messages.iter().all(|m| m.reasoning.is_none()),
            "a later turn leaked reasoning from an earlier one"
        );
    }
}
