//! Integration tests for task unit `D2`: no two sessions can win the
//! same claim, even racing in parallel.

use std::path::PathBuf;
use std::sync::Arc;
use std::thread;

use dark_cartograph::frontier::{self, ClaimOutcome, DEFAULT_LEASE_MS};
use dark_cartograph::journal::{
    self, JournalEvent, MapCreated, MapStatus, TicketCreated, TicketStatus, TicketType,
};
use dark_cartograph::store::Store;
use tempfile::TempDir;

/// Builds a map with one open ticket, ready for eight sessions to race
/// over.
fn seed_map(repo_root: &std::path::Path, maps_root: &std::path::Path) {
    let mut store = Store::open(repo_root).unwrap();
    store
        .apply(&JournalEvent::MapCreated(MapCreated {
            id: "M1".to_owned(),
            name: "Race map".to_owned(),
            destination: "A destination worth racing over".to_owned(),
            notes: None,
            created_at: 1_700_000_000_000,
            status: MapStatus::Active,
        }))
        .unwrap();
    store
        .apply(&JournalEvent::TicketCreated(TicketCreated {
            id: "T1".to_owned(),
            map_id: "M1".to_owned(),
            name: "The contested ticket".to_owned(),
            question: "Who wins the claim?".to_owned(),
            ticket_type: TicketType::Task,
            hitl: false,
            status: TicketStatus::Open,
            created_at: 1_700_000_000_000,
            ordinal: 0,
            axis: None,
            tokens_used: None,
        }))
        .unwrap();
    // Persist alongside the derived database so every racing thread's
    // own `Store::open` sees the same starting state; `apply` above only
    // touched the in-memory-backed file this `store` holds open.
    journal::append(
        maps_root,
        "M1",
        &JournalEvent::MapCreated(MapCreated {
            id: "M1".to_owned(),
            name: "Race map".to_owned(),
            destination: "A destination worth racing over".to_owned(),
            notes: None,
            created_at: 1_700_000_000_000,
            status: MapStatus::Active,
        }),
    )
    .unwrap();
    journal::append(
        maps_root,
        "M1",
        &JournalEvent::TicketCreated(TicketCreated {
            id: "T1".to_owned(),
            map_id: "M1".to_owned(),
            name: "The contested ticket".to_owned(),
            question: "Who wins the claim?".to_owned(),
            ticket_type: TicketType::Task,
            hitl: false,
            status: TicketStatus::Open,
            created_at: 1_700_000_000_000,
            ordinal: 0,
            axis: None,
            tokens_used: None,
        }),
    )
    .unwrap();
}

#[test]
fn eight_parallel_claims_produce_exactly_one_winner() {
    let tmp = TempDir::new().expect("tempdir");
    let repo_root: PathBuf = tmp.path().join("repo");
    let maps_root: PathBuf = tmp.path().join("maps");
    seed_map(&repo_root, &maps_root);

    let repo_root = Arc::new(repo_root);
    let maps_root = Arc::new(maps_root);

    let handles: Vec<_> = (0..8)
        .map(|i| {
            let repo_root = Arc::clone(&repo_root);
            let maps_root = Arc::clone(&maps_root);
            thread::spawn(move || {
                // Each session opens its own connection to the same
                // database file, the way eight separate `dark` processes
                // would.
                let mut store = Store::open(&repo_root).expect("open store");
                frontier::claim(
                    &mut store,
                    &maps_root,
                    "M1",
                    "T1",
                    &format!("session-{i}"),
                    1_700_000_000_000,
                    DEFAULT_LEASE_MS,
                )
                .expect("claim must not error, only win or lose")
            })
        })
        .collect();

    let outcomes: Vec<ClaimOutcome> = handles
        .into_iter()
        .map(|handle| handle.join().expect("claiming thread must not panic"))
        .collect();

    let winners = outcomes
        .iter()
        .filter(|outcome| matches!(outcome, ClaimOutcome::Claimed { .. }))
        .count();
    assert_eq!(winners, 1, "exactly one session must win the claim");

    let losers = outcomes
        .iter()
        .filter(|outcome| matches!(outcome, ClaimOutcome::NotAvailable))
        .count();
    assert_eq!(losers, 7);

    // The database agrees: the ticket is claimed by exactly one session.
    let store = Store::open(&repo_root).unwrap();
    let (status, claimed_by): (String, Option<String>) = store
        .connection()
        .query_row(
            "SELECT status, claimed_by FROM tickets WHERE id = 'T1'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(status, "claimed");
    assert!(claimed_by.is_some());

    // The journal agrees too: exactly one TicketUpdated claim event was
    // ever appended for T1.
    let events = journal::read_events(&maps_root, "M1").unwrap();
    let claim_events = events
        .iter()
        .filter(|event| {
            matches!(
                event,
                JournalEvent::TicketUpdated(u)
                    if u.id == "T1" && u.status == Some(TicketStatus::Claimed)
            )
        })
        .count();
    assert_eq!(claim_events, 1);
}
