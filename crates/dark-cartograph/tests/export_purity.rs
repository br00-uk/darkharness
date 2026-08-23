//! Integration test for task unit `D5`: export is a pure function of the
//! map's stored state — no clock, no absolute path, no iteration order
//! that varies between runs. Mirrors the shape `tests/digest_budget.rs`
//! uses to pin the same property for the digest (task unit `D3`).

use dark_cartograph::export::{self, Format};
use dark_cartograph::journal::{
    EdgeAdded, FogAdded, JournalEvent, MapCreated, MapStatus, ScopeExclusionAdded, TicketCreated,
    TicketStatus, TicketType, TicketUpdated,
};
use dark_cartograph::store::Store;
use tempfile::TempDir;

/// Builds a map that touches every section every export format renders:
/// a resolved ticket, an open ticket blocked by another, fog, and a
/// scope exclusion.
fn seed_every_section(store: &mut Store) {
    store
        .apply(&JournalEvent::MapCreated(MapCreated {
            id: "M1".to_owned(),
            name: "Offline pack format".to_owned(),
            destination: "A frozen pack format".to_owned(),
            notes: Some("Domain: Rust".to_owned()),
            created_at: 1_700_000_000_000,
            status: MapStatus::Active,
        }))
        .unwrap();
    store
        .apply(&JournalEvent::TicketCreated(TicketCreated {
            id: "T1".to_owned(),
            map_id: "M1".to_owned(),
            name: "Pack identity is content-addressed".to_owned(),
            question: "How is a pack identified?".to_owned(),
            ticket_type: TicketType::Research,
            hitl: false,
            status: TicketStatus::Open,
            created_at: 1_700_000_000_000,
            ordinal: 0,
            axis: None,
            tokens_used: None,
        }))
        .unwrap();
    store
        .apply(&JournalEvent::TicketUpdated(TicketUpdated {
            id: "T1".to_owned(),
            status: Some(TicketStatus::Resolved),
            resolution: Some("blake3 of the canonical manifest".to_owned()),
            gist: Some("blake3 of canonical manifest".to_owned()),
            resolved_at: Some(1_700_000_005_000),
            tokens_used: Some(400),
            ..TicketUpdated::default()
        }))
        .unwrap();
    store
        .apply(&JournalEvent::TicketCreated(TicketCreated {
            id: "T2".to_owned(),
            map_id: "M1".to_owned(),
            name: "Chunking".to_owned(),
            question: "How is a document chunked?".to_owned(),
            ticket_type: TicketType::Task,
            hitl: false,
            status: TicketStatus::Open,
            created_at: 1_700_000_001_000,
            ordinal: 1,
            axis: Some("format".to_owned()),
            tokens_used: None,
        }))
        .unwrap();
    store
        .apply(&JournalEvent::TicketCreated(TicketCreated {
            id: "T3".to_owned(),
            map_id: "M1".to_owned(),
            name: "Registry lookup".to_owned(),
            question: "What does the registry return?".to_owned(),
            ticket_type: TicketType::Grilling,
            hitl: true,
            status: TicketStatus::Open,
            created_at: 1_700_000_002_000,
            ordinal: 2,
            axis: None,
            tokens_used: None,
        }))
        .unwrap();
    store
        .apply(&JournalEvent::EdgeAdded(EdgeAdded {
            blocker: "T2".to_owned(),
            blocked: "T3".to_owned(),
        }))
        .unwrap();
    store
        .apply(&JournalEvent::FogAdded(FogAdded {
            id: "F1".to_owned(),
            map_id: "M1".to_owned(),
            patch: "How packs are distributed.".to_owned(),
            axis: None,
            created_at: 1_700_000_003_000,
        }))
        .unwrap();
    store
        .apply(&JournalEvent::ScopeExclusionAdded(ScopeExclusionAdded {
            id: "S1".to_owned(),
            map_id: "M1".to_owned(),
            gist: "Pack signing".to_owned(),
            reason: "Separate effort".to_owned(),
            ticket_id: None,
        }))
        .unwrap();
}

#[test]
fn every_format_produces_byte_identical_output_across_two_independent_stores() {
    // Two separate `Store` connections opened from two separate temp
    // directories, each fed the same journal events in the same order:
    // this is closer to "the same map exported by two different
    // processes" than reusing one open connection would be.
    let tmp_a = TempDir::new().expect("tempdir");
    let mut store_a = Store::open(tmp_a.path()).expect("open store a");
    seed_every_section(&mut store_a);

    let tmp_b = TempDir::new().expect("tempdir");
    let mut store_b = Store::open(tmp_b.path()).expect("open store b");
    seed_every_section(&mut store_b);

    for format in [Format::Github, Format::Markdown, Format::Mermaid] {
        let from_a = export::export(&store_a, "M1", format).unwrap();
        let from_b = export::export(&store_b, "M1", format).unwrap();
        assert_eq!(
            from_a, from_b,
            "{format:?} export must depend only on map state, not on which store rendered it"
        );
        assert_eq!(from_a.as_bytes(), from_b.as_bytes());
    }
}

#[test]
fn every_format_produces_byte_identical_output_on_repeated_calls() {
    let tmp = TempDir::new().expect("tempdir");
    let mut store = Store::open(tmp.path()).expect("open store");
    seed_every_section(&mut store);

    for format in [Format::Github, Format::Markdown, Format::Mermaid] {
        let first = export::export(&store, "M1", format).unwrap();
        let second = export::export(&store, "M1", format).unwrap();
        let third = export::export(&store, "M1", format).unwrap();
        assert_eq!(first, second);
        assert_eq!(second, third);
    }
}

#[test]
fn markdown_and_mermaid_contain_no_absolute_path() {
    let tmp = TempDir::new().expect("tempdir");
    let mut store = Store::open(tmp.path()).expect("open store");
    seed_every_section(&mut store);

    let tmp_path = tmp.path().to_string_lossy().into_owned();
    for format in [Format::Markdown, Format::Mermaid] {
        let text = export::export(&store, "M1", format).unwrap();
        assert!(
            !text.contains(&tmp_path),
            "{format:?} export must not leak the store's filesystem path"
        );
    }
}
