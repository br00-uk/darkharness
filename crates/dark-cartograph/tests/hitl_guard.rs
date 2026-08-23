//! Integration tests for task unit `D4`'s two guards on `ticket_resolve`:
//! Rule 19 (a human-in-the-loop ticket needs a person present) and Rule
//! 20 (a session resolves at most one non-research ticket).

use std::sync::Arc;

use dark_cartograph::journal::{
    self, JournalEvent, MapCreated, MapStatus, TicketCreated, TicketStatus, TicketType,
};
use dark_cartograph::store::Store;
use dark_cartograph::tools::{CartographSession, TicketResolve};
use dark_contract::{ErrCode, EventBus, Tool, ToolCtx};
use serde_json::json;
use tempfile::TempDir;

/// One map with a grilling (human-in-the-loop) ticket and two ordinary
/// task tickets, on disk in both the journal and the derived database.
fn seed(repo_root: &std::path::Path, maps_root: &std::path::Path) {
    let mut store = Store::open(repo_root).unwrap();
    let events = [
        JournalEvent::MapCreated(MapCreated {
            id: "M1".to_owned(),
            name: "Offline pack format".to_owned(),
            destination: "A frozen pack format".to_owned(),
            notes: None,
            created_at: 1_700_000_000_000,
            status: MapStatus::Active,
        }),
        JournalEvent::TicketCreated(TicketCreated {
            id: "T-grilling".to_owned(),
            map_id: "M1".to_owned(),
            name: "Staleness policy".to_owned(),
            question: "How does a pack declare its staleness policy?".to_owned(),
            ticket_type: TicketType::Grilling,
            hitl: true,
            status: TicketStatus::Open,
            created_at: 1_700_000_000_000,
            ordinal: 0,
            axis: None,
            tokens_used: None,
        }),
        JournalEvent::TicketCreated(TicketCreated {
            id: "T-task-1".to_owned(),
            map_id: "M1".to_owned(),
            name: "First task".to_owned(),
            question: "What must this task do?".to_owned(),
            ticket_type: TicketType::Task,
            hitl: false,
            status: TicketStatus::Open,
            created_at: 1_700_000_000_000,
            ordinal: 1,
            axis: None,
            tokens_used: None,
        }),
        JournalEvent::TicketCreated(TicketCreated {
            id: "T-task-2".to_owned(),
            map_id: "M1".to_owned(),
            name: "Second task".to_owned(),
            question: "What must this second task do?".to_owned(),
            ticket_type: TicketType::Task,
            hitl: false,
            status: TicketStatus::Open,
            created_at: 1_700_000_000_000,
            ordinal: 2,
            axis: None,
            tokens_used: None,
        }),
    ];
    for event in &events {
        store.apply(event).unwrap();
        journal::append(maps_root, "M1", event).unwrap();
    }
}

// `tokio_util` is not a declared dependency of this crate (Rule 16), so
// `CancellationToken` cannot be named here — only `ToolCtx.cancel`'s
// already-resolved field type names it. `Default::default()` is the one
// way to build one without naming the type.
#[allow(clippy::default_trait_access)]
fn ctx(root: &std::path::Path, human_present: bool) -> ToolCtx {
    let bus = EventBus::new();
    ToolCtx {
        root: root.to_path_buf(),
        events: bus.tx(),
        cancel: Default::default(),
        dark: true,
        human_present,
    }
}

fn resolve_args(ticket_id: &str) -> serde_json::Value {
    json!({
        "ticket_id": ticket_id,
        "resolution": "The full answer.",
        "gist": "a short gist",
        "tokens_used": 500,
    })
}

/// Task unit `D4`'s "Done when": headless work on a grilling ticket
/// returns `E_HITL_REQUIRES_HUMAN`.
#[tokio::test]
async fn headless_work_on_a_grilling_ticket_returns_hitl_requires_human() {
    let tmp = TempDir::new().unwrap();
    let repo_root = tmp.path().join("repo");
    let maps_root = tmp.path().join("maps");
    seed(&repo_root, &maps_root);

    let session = Arc::new(CartographSession::new(maps_root.clone(), "session-a"));
    let tool = TicketResolve::new(session);

    let err = tool
        .invoke(resolve_args("T-grilling"), &ctx(&repo_root, false))
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrCode::HitlRequiresHuman);
    assert_eq!(
        err.remedy.as_deref(),
        Some("Open the terminal application. Confirm in the modal.")
    );

    // Nothing changed: neither the database nor the journal recorded a
    // resolution.
    let store = Store::open(&repo_root).unwrap();
    let status: String = store
        .connection()
        .query_row(
            "SELECT status FROM tickets WHERE id = 'T-grilling'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(status, "open");
    let events = journal::read_events(&maps_root, "M1").unwrap();
    assert!(
        !events.iter().any(|e| matches!(
            e,
            JournalEvent::TicketUpdated(u) if u.id == "T-grilling"
        )),
        "a rejected resolution must not be journalled"
    );
}

/// The same grilling ticket resolves once a human-present token is held.
#[tokio::test]
async fn a_grilling_ticket_resolves_once_a_human_is_present() {
    let tmp = TempDir::new().unwrap();
    let repo_root = tmp.path().join("repo");
    let maps_root = tmp.path().join("maps");
    seed(&repo_root, &maps_root);

    let session = Arc::new(CartographSession::new(maps_root, "session-a"));
    let tool = TicketResolve::new(session);

    let result = tool
        .invoke(resolve_args("T-grilling"), &ctx(&repo_root, true))
        .await
        .unwrap();
    assert!(!result.is_error);
}

/// Task unit `D4`'s "Done when": a second resolution returns
/// `E_SESSION_RESOLUTION_LIMIT`.
#[tokio::test]
async fn a_second_non_research_resolution_returns_session_resolution_limit() {
    let tmp = TempDir::new().unwrap();
    let repo_root = tmp.path().join("repo");
    let maps_root = tmp.path().join("maps");
    seed(&repo_root, &maps_root);

    let session = Arc::new(CartographSession::new(maps_root.clone(), "session-a"));

    let first = TicketResolve::new(session.clone())
        .invoke(resolve_args("T-task-1"), &ctx(&repo_root, false))
        .await
        .unwrap();
    assert!(!first.is_error);

    let err = TicketResolve::new(session)
        .invoke(resolve_args("T-task-2"), &ctx(&repo_root, false))
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrCode::SessionResolutionLimit);
    assert_eq!(err.remedy.as_deref(), Some("Start a new session."));

    // The second ticket is untouched: the guard fired before any write.
    let store = Store::open(&repo_root).unwrap();
    let status: String = store
        .connection()
        .query_row(
            "SELECT status FROM tickets WHERE id = 'T-task-2'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(status, "open");
}

/// Both guards compose: a session that already resolved a task ticket
/// can still resolve a human-in-the-loop ticket only with a human
/// present, and the resolution limit still applies to it once granted.
#[tokio::test]
async fn the_two_guards_compose_across_calls() {
    let tmp = TempDir::new().unwrap();
    let repo_root = tmp.path().join("repo");
    let maps_root = tmp.path().join("maps");
    seed(&repo_root, &maps_root);

    let session = Arc::new(CartographSession::new(maps_root, "session-a"));

    TicketResolve::new(session.clone())
        .invoke(resolve_args("T-task-1"), &ctx(&repo_root, false))
        .await
        .unwrap();

    // Rule 19 still applies to the grilling ticket, independent of Rule
    // 20's counter.
    let err = TicketResolve::new(session.clone())
        .invoke(resolve_args("T-grilling"), &ctx(&repo_root, false))
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrCode::HitlRequiresHuman);

    // With a human present, Rule 20 is the one that now fires, because
    // this session already spent its one non-research resolution.
    let err = TicketResolve::new(session)
        .invoke(resolve_args("T-grilling"), &ctx(&repo_root, true))
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrCode::SessionResolutionLimit);
}
