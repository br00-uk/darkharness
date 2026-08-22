//! Integration tests for task unit `D1`: replaying the journal into the
//! database, and surviving a crash mid-write.

use std::fs;
use std::io::Write;

use dark_cartograph::journal::{
    self, AssetAdded, EdgeAdded, FogAdded, FogGraduated, JournalEvent, MapCreated, MapStatus,
    MapUpdated, ScopeExclusionAdded, TicketCreated, TicketStatus, TicketType, TicketUpdated,
};
use dark_cartograph::store::Store;
use tempfile::TempDir;

/// Appends the full set of events a working map would produce: a map, two
/// tickets, an edge between them, a fog patch that later graduates, a
/// scope exclusion, and an asset. Touches every table in the schema.
/// The map itself and the two tickets on it.
fn map_and_tickets(map_id: &str) -> Vec<JournalEvent> {
    vec![
        JournalEvent::MapCreated(MapCreated {
            id: map_id.to_owned(),
            name: "Offline pack format".to_owned(),
            destination: "A frozen pack format".to_owned(),
            notes: Some("Domain: Rust".to_owned()),
            created_at: 1_700_000_000_000,
            status: MapStatus::Charting,
        }),
        JournalEvent::MapUpdated(MapUpdated {
            id: map_id.to_owned(),
            status: Some(MapStatus::Active),
            updated_at: 1_700_000_000_500,
            ..MapUpdated::default()
        }),
        JournalEvent::TicketCreated(TicketCreated {
            id: "T-001".to_owned(),
            map_id: map_id.to_owned(),
            name: "Pack identity".to_owned(),
            question: "How is a pack identified?".to_owned(),
            ticket_type: TicketType::Research,
            hitl: false,
            status: TicketStatus::Open,
            created_at: 1_700_000_001_000,
            ordinal: 0,
            axis: Some("storage".to_owned()),
            tokens_used: None,
        }),
    ]
}

/// The edge between the two tickets, and the fog patch that later
/// graduates into one of them.
fn structure(map_id: &str) -> Vec<JournalEvent> {
    vec![
        JournalEvent::TicketCreated(TicketCreated {
            id: "T-002".to_owned(),
            map_id: map_id.to_owned(),
            name: "Chunking".to_owned(),
            question: "How is a document chunked?".to_owned(),
            ticket_type: TicketType::Task,
            hitl: false,
            status: TicketStatus::Open,
            created_at: 1_700_000_002_000,
            ordinal: 1,
            axis: None,
            tokens_used: None,
        }),
        JournalEvent::TicketUpdated(TicketUpdated {
            id: "T-001".to_owned(),
            status: Some(TicketStatus::Resolved),
            resolution: Some("blake3 of the canonical manifest".to_owned()),
            gist: Some("Pack identity is content-addressed".to_owned()),
            resolved_at: Some(1_700_000_003_000),
            ..TicketUpdated::default()
        }),
        JournalEvent::EdgeAdded(EdgeAdded {
            blocker: "T-001".to_owned(),
            blocked: "T-002".to_owned(),
        }),
    ]
}

/// What a working session hangs off a ticket: a scope exclusion and an
/// asset.
fn annotations(map_id: &str) -> Vec<JournalEvent> {
    vec![
        JournalEvent::FogAdded(FogAdded {
            id: "F-001".to_owned(),
            map_id: map_id.to_owned(),
            patch: "How packs are distributed".to_owned(),
            axis: None,
            created_at: 1_700_000_004_000,
        }),
        JournalEvent::FogGraduated(FogGraduated {
            id: "F-001".to_owned(),
            graduated_to: "T-003".to_owned(),
        }),
        JournalEvent::ScopeExclusionAdded(ScopeExclusionAdded {
            id: "S-001".to_owned(),
            map_id: map_id.to_owned(),
            gist: "Pack signing".to_owned(),
            reason: "Separate effort".to_owned(),
            ticket_id: None,
        }),
        JournalEvent::AssetAdded(AssetAdded {
            id: "A-001".to_owned(),
            ticket_id: "T-001".to_owned(),
            kind: Some("note".to_owned()),
            path: None,
            note: Some("initial spike".to_owned()),
        }),
    ]
}

/// Appends the full set of events a working map would produce: a map, two
/// tickets, an edge between them, a fog patch that later graduates, a
/// scope exclusion, and an asset. Touches every table in the schema.
fn seed_full_journal(maps_root: &std::path::Path, map_id: &str) {
    let events = map_and_tickets(map_id)
        .into_iter()
        .chain(structure(map_id))
        .chain(annotations(map_id));

    for event in events {
        journal::append(maps_root, map_id, &event).unwrap();
    }
}

#[test]
fn replay_reproduces_the_database_exactly() {
    let tmp = TempDir::new().expect("tempdir");
    let repo_root = tmp.path().join("repo");
    let maps_root = tmp.path().join("maps");
    seed_full_journal(&maps_root, "M1");

    let store = Store::rebuild(&repo_root, &maps_root).expect("rebuild");
    let conn = store.connection();

    let (name, destination, notes, status): (String, String, Option<String>, String) = conn
        .query_row(
            "SELECT name, destination, notes, status FROM maps WHERE id = 'M1'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .expect("map row");
    assert_eq!(name, "Offline pack format");
    assert_eq!(destination, "A frozen pack format");
    assert_eq!(notes.as_deref(), Some("Domain: Rust"));
    assert_eq!(status, "active", "the later MapUpdated must win");

    let (t1_status, t1_resolution, t1_gist): (String, Option<String>, Option<String>) = conn
        .query_row(
            "SELECT status, resolution, gist FROM tickets WHERE id = 'T-001'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .expect("ticket row");
    assert_eq!(t1_status, "resolved");
    assert_eq!(
        t1_resolution.as_deref(),
        Some("blake3 of the canonical manifest")
    );
    assert_eq!(
        t1_gist.as_deref(),
        Some("Pack identity is content-addressed")
    );

    let t2_status: String = conn
        .query_row("SELECT status FROM tickets WHERE id = 'T-002'", [], |row| {
            row.get(0)
        })
        .expect("ticket row");
    assert_eq!(t2_status, "open", "an update to T-001 must not touch T-002");

    let edge_count: i64 = conn
        .query_row(
            "SELECT count(*) FROM edges WHERE blocker = 'T-001' AND blocked = 'T-002'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(edge_count, 1);

    let graduated_to: Option<String> = conn
        .query_row(
            "SELECT graduated_to FROM fog WHERE id = 'F-001'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(graduated_to.as_deref(), Some("T-003"));

    let exclusion_count: i64 = conn
        .query_row(
            "SELECT count(*) FROM scope_exclusions WHERE id = 'S-001'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(exclusion_count, 1);

    let asset_note: Option<String> = conn
        .query_row("SELECT note FROM assets WHERE id = 'A-001'", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(asset_note.as_deref(), Some("initial spike"));

    // Rebuilding a second time from the same journal must land on exactly
    // the same content: replay has no hidden state that a second pass
    // could accumulate differently.
    let second = Store::rebuild(&repo_root, &maps_root).expect("second rebuild");
    let second_status: String = second
        .connection()
        .query_row("SELECT status FROM maps WHERE id = 'M1'", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(second_status, "active");
}

#[test]
fn replay_skips_a_truncated_final_line_left_by_a_crash() {
    let tmp = TempDir::new().expect("tempdir");
    let repo_root = tmp.path().join("repo");
    let maps_root = tmp.path().join("maps");
    seed_full_journal(&maps_root, "M1");

    // A crash mid-write leaves a partial JSON object on the last line: no
    // closing brace, no trailing newline.
    let path = journal::journal_path(&maps_root, "M1");
    let mut file = fs::OpenOptions::new().append(true).open(&path).unwrap();
    write!(file, "{{\"event\":\"ticket_created\",\"id\":\"T-9").unwrap();
    file.flush().unwrap();
    drop(file);

    let store = Store::rebuild(&repo_root, &maps_root).expect("rebuild must not fail");

    // Every event before the truncated line still applied.
    let map_count: i64 = store
        .connection()
        .query_row("SELECT count(*) FROM maps WHERE id = 'M1'", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(map_count, 1);
    let ticket_count: i64 = store
        .connection()
        .query_row("SELECT count(*) FROM tickets", [], |row| row.get(0))
        .unwrap();
    assert_eq!(ticket_count, 2, "the two well-formed tickets must survive");

    // The truncated ticket never made it in.
    let bad_ticket_count: i64 = store
        .connection()
        .query_row("SELECT count(*) FROM tickets WHERE id = 'T-9'", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(bad_ticket_count, 0);
}

#[test]
fn replay_fails_on_corruption_that_is_not_the_final_line() {
    let tmp = TempDir::new().expect("tempdir");
    let maps_root = tmp.path().join("maps");
    seed_full_journal(&maps_root, "M1");

    let path = journal::journal_path(&maps_root, "M1");
    let mut content = fs::read_to_string(&path).unwrap();
    let mut lines: Vec<&str> = content.lines().collect();
    // Corrupt a line in the middle, not the last one.
    let mid = lines.len() / 2;
    lines.insert(mid, "{this is not valid json");
    content = lines.join("\n");
    content.push('\n');
    fs::write(&path, content).unwrap();

    let result = journal::read_events(&maps_root, "M1");
    assert!(
        result.is_err(),
        "corruption that is not the final line must fail replay, not be silently skipped"
    );
}
