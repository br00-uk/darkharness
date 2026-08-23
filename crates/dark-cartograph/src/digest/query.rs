//! Reads the plain data a digest renders from, out of the derived
//! database.
//!
//! Every function here only reads. None of them touches a clock, an
//! environment variable, or anything outside the rows `map_id` names, so
//! the same map state always yields the same [`MapSnapshot`] — the
//! purity [`crate::digest::render`] depends on to keep the digest stable
//! within a turn (see Rule 5, `CLAUDE.md`).

use dark_contract::{ErrCode, Error, Result};
use rusqlite::params;

use crate::frontier::{self, FrontierTicket};
use crate::journal::MapStatus;
use crate::store::Store;

/// One resolved ticket, ready to render as a decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Decision {
    /// The ticket's identifier.
    pub id: String,
    /// The ticket's short name — the decision statement itself.
    pub name: String,
    /// A short summary of the resolution, when the ticket has one.
    pub gist: Option<String>,
}

/// One open ticket that the frontier query excludes because a blocker
/// has not resolved or left scope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Blocked {
    /// The blocked ticket's identifier.
    pub id: String,
    /// The blocked ticket's short name.
    pub name: String,
    /// The identifiers of the tickets still blocking it, in blocker-id
    /// order.
    pub blockers: Vec<String>,
}

/// One patch of fog: a question the map has not yet turned into a
/// ticket.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Fog {
    /// The text of the unanswered question.
    pub patch: String,
}

/// One thing the map excludes from its scope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ScopeExclusion {
    /// A short summary of the excluded thing.
    pub gist: String,
    /// Why the map excludes it.
    pub reason: String,
    /// The ticket that raised this exclusion, when one did.
    pub ticket_id: Option<String>,
}

/// Everything a digest needs about one map, read in one pass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct MapSnapshot {
    /// The map's short name.
    pub name: String,
    /// The destination: what the map is charting a way towards.
    pub destination: String,
    /// Free-text notes about the map, when it has any.
    pub notes: Option<String>,
    /// The map's status.
    pub status: MapStatus,
    /// How many tickets the map has in total.
    pub ticket_count: i64,
    /// How many of those tickets have resolved.
    pub resolved_count: i64,
    /// Resolved tickets, most recently resolved first.
    pub decisions: Vec<Decision>,
    /// Tickets the frontier excludes because a blocker has not cleared,
    /// in ticket-ordinal order.
    pub blocked: Vec<Blocked>,
    /// Fog patches that have not yet graduated into a ticket.
    pub fog: Vec<Fog>,
    /// Things the map excludes from its scope.
    pub scope_exclusions: Vec<ScopeExclusion>,
    /// The frontier itself: open, unblocked tickets, in ordinal order.
    pub frontier: Vec<FrontierTicket>,
}

/// Reads the full [`MapSnapshot`] for `map_id`.
///
/// # Errors
///
/// Returns [`ErrCode::MapNotFound`] when no map has this identifier.
/// Returns an error when a query fails, or when a stored `maps.status`
/// value does not match a known [`MapStatus`] variant — which only
/// happens if a row was written outside this crate.
pub(super) fn load(store: &Store, map_id: &str) -> Result<MapSnapshot> {
    let conn = store.connection();

    let (name, destination, notes, status_str): (String, String, Option<String>, String) = conn
        .query_row(
            "SELECT name, destination, notes, status FROM maps WHERE id = ?1",
            params![map_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .map_err(|err| match err {
            rusqlite::Error::QueryReturnedNoRows => {
                Error::new(ErrCode::MapNotFound, format!("no map with id {map_id}"))
            }
            other => sql_failed(format!("cannot read map {map_id}: {other}")),
        })?;
    let status = parse_map_status(&status_str)?;

    let (ticket_count, resolved_count): (i64, i64) = conn
        .query_row(
            "SELECT COUNT(*), COALESCE(SUM(CASE WHEN status = 'resolved' THEN 1 ELSE 0 END), 0)
             FROM tickets WHERE map_id = ?1",
            params![map_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(|err| sql_failed(format!("cannot count tickets for {map_id}: {err}")))?;

    let decisions = load_decisions(store, map_id)?;
    let blocked = load_blocked(store, map_id)?;
    let fog = load_fog(store, map_id)?;
    let scope_exclusions = load_scope_exclusions(store, map_id)?;
    let frontier = frontier::frontier(store, map_id)?;

    Ok(MapSnapshot {
        name,
        destination,
        notes,
        status,
        ticket_count,
        resolved_count,
        decisions,
        blocked,
        fog,
        scope_exclusions,
        frontier,
    })
}

/// Reads resolved tickets, most recently resolved first, then by
/// identifier — a stable order even when two tickets share a
/// `resolved_at` value.
fn load_decisions(store: &Store, map_id: &str) -> Result<Vec<Decision>> {
    let conn = store.connection();
    let mut stmt = conn
        .prepare(
            "SELECT id, name, gist FROM tickets
             WHERE map_id = ?1 AND status = 'resolved'
             ORDER BY resolved_at DESC, id ASC",
        )
        .map_err(|err| sql_failed(format!("cannot prepare the decisions query: {err}")))?;
    let rows = stmt
        .query_map(params![map_id], |row| {
            Ok(Decision {
                id: row.get(0)?,
                name: row.get(1)?,
                gist: row.get(2)?,
            })
        })
        .map_err(|err| sql_failed(format!("cannot run the decisions query: {err}")))?;
    rows.collect::<rusqlite::Result<_>>()
        .map_err(|err| sql_failed(format!("cannot read a decision row: {err}")))
}

/// Reads every open ticket that has at least one blocker that has not
/// resolved or left scope, together with those blockers, in
/// ticket-ordinal then blocker-id order.
fn load_blocked(store: &Store, map_id: &str) -> Result<Vec<Blocked>> {
    let conn = store.connection();
    let mut stmt = conn
        .prepare(
            "SELECT t.id, t.name, e.blocker FROM tickets t
             JOIN edges e ON e.blocked = t.id
             JOIN tickets bt ON bt.id = e.blocker
             WHERE t.map_id = ?1 AND t.status = 'open'
               AND bt.status NOT IN ('resolved','out_of_scope')
             ORDER BY t.ordinal, e.blocker",
        )
        .map_err(|err| sql_failed(format!("cannot prepare the blocked query: {err}")))?;
    let rows = stmt
        .query_map(params![map_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .map_err(|err| sql_failed(format!("cannot run the blocked query: {err}")))?;

    let mut blocked: Vec<Blocked> = Vec::new();
    for row in rows {
        let (ticket_id, ticket_name, blocker_id) =
            row.map_err(|err| sql_failed(format!("cannot read a blocked row: {err}")))?;
        match blocked.last_mut() {
            Some(entry) if entry.id == ticket_id => entry.blockers.push(blocker_id),
            _ => blocked.push(Blocked {
                id: ticket_id,
                name: ticket_name,
                blockers: vec![blocker_id],
            }),
        }
    }
    Ok(blocked)
}

/// Reads fog patches that have not yet graduated into a ticket, in
/// creation order.
fn load_fog(store: &Store, map_id: &str) -> Result<Vec<Fog>> {
    let conn = store.connection();
    let mut stmt = conn
        .prepare(
            "SELECT patch FROM fog
             WHERE map_id = ?1 AND graduated_to IS NULL
             ORDER BY created_at ASC, id ASC",
        )
        .map_err(|err| sql_failed(format!("cannot prepare the fog query: {err}")))?;
    let rows = stmt
        .query_map(params![map_id], |row| Ok(Fog { patch: row.get(0)? }))
        .map_err(|err| sql_failed(format!("cannot run the fog query: {err}")))?;
    rows.collect::<rusqlite::Result<_>>()
        .map_err(|err| sql_failed(format!("cannot read a fog row: {err}")))
}

/// Reads scope exclusions in identifier order.
fn load_scope_exclusions(store: &Store, map_id: &str) -> Result<Vec<ScopeExclusion>> {
    let conn = store.connection();
    let mut stmt = conn
        .prepare(
            "SELECT gist, reason, ticket_id FROM scope_exclusions
             WHERE map_id = ?1
             ORDER BY id ASC",
        )
        .map_err(|err| sql_failed(format!("cannot prepare the scope-exclusions query: {err}")))?;
    let rows = stmt
        .query_map(params![map_id], |row| {
            Ok(ScopeExclusion {
                gist: row.get(0)?,
                reason: row.get(1)?,
                ticket_id: row.get(2)?,
            })
        })
        .map_err(|err| sql_failed(format!("cannot run the scope-exclusions query: {err}")))?;
    rows.collect::<rusqlite::Result<_>>()
        .map_err(|err| sql_failed(format!("cannot read a scope-exclusion row: {err}")))
}

/// Parses a `maps.status` value back into a [`MapStatus`].
fn parse_map_status(value: &str) -> Result<MapStatus> {
    match value {
        "charting" => Ok(MapStatus::Charting),
        "active" => Ok(MapStatus::Active),
        "complete" => Ok(MapStatus::Complete),
        "abandoned" => Ok(MapStatus::Abandoned),
        other => Err(sql_failed(format!(
            "unrecognised maps.status value {other:?}"
        ))),
    }
}

/// Builds an [`Error`] for a database failure that no more specific code
/// covers. Mirrors `crate::store::sql_failed`, which is private to that
/// module.
fn sql_failed(message: String) -> Error {
    Error::new(ErrCode::ToolFailed, message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::journal::{
        EdgeAdded, FogAdded, JournalEvent, MapCreated, ScopeExclusionAdded, TicketCreated,
        TicketStatus, TicketType, TicketUpdated,
    };
    use tempfile::TempDir;

    fn open_test_store() -> (TempDir, Store) {
        let tmp = TempDir::new().expect("tempdir");
        let store = Store::open(tmp.path()).expect("open store");
        (tmp, store)
    }

    #[test]
    fn load_reports_map_not_found_for_an_unknown_map() {
        let (_tmp, store) = open_test_store();
        let err = load(&store, "no-such-map").unwrap_err();
        assert_eq!(err.code, ErrCode::MapNotFound);
    }

    /// Seeds a map on `store` that touches every section a digest
    /// renders: a resolved ticket (a decision), an open ticket blocked
    /// by another open ticket, an ungraduated fog patch, and a scope
    /// exclusion. Split out of `load_reads_every_section` to keep that
    /// test's own body short.
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
                gist: Some("blake3 of the canonical manifest".to_owned()),
                resolved_at: Some(1_700_000_005_000),
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
                axis: None,
                tokens_used: None,
            }))
            .unwrap();
        store
            .apply(&JournalEvent::TicketCreated(TicketCreated {
                id: "T3".to_owned(),
                map_id: "M1".to_owned(),
                name: "Registry lookup".to_owned(),
                question: "What does the registry return?".to_owned(),
                ticket_type: TicketType::Research,
                hitl: false,
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
    fn load_reads_every_section() {
        let (_tmp, mut store) = open_test_store();
        seed_every_section(&mut store);

        let snapshot = load(&store, "M1").unwrap();
        assert_eq!(snapshot.name, "Offline pack format");
        assert_eq!(snapshot.notes.as_deref(), Some("Domain: Rust"));
        assert_eq!(snapshot.status, MapStatus::Active);
        assert_eq!(snapshot.ticket_count, 3);
        assert_eq!(snapshot.resolved_count, 1);
        assert_eq!(snapshot.decisions.len(), 1);
        assert_eq!(snapshot.decisions[0].id, "T1");
        assert_eq!(snapshot.blocked.len(), 1);
        assert_eq!(snapshot.blocked[0].id, "T3");
        assert_eq!(snapshot.blocked[0].blockers, vec!["T2".to_owned()]);
        assert_eq!(snapshot.fog.len(), 1);
        assert_eq!(snapshot.scope_exclusions.len(), 1);
        assert_eq!(
            snapshot
                .frontier
                .iter()
                .map(|t| t.id.clone())
                .collect::<Vec<_>>(),
            vec!["T2".to_owned()]
        );
    }

    #[test]
    fn load_omits_a_graduated_fog_patch() {
        let (_tmp, mut store) = open_test_store();
        store
            .apply(&JournalEvent::MapCreated(MapCreated {
                id: "M1".to_owned(),
                name: "Map".to_owned(),
                destination: "Dest".to_owned(),
                notes: None,
                created_at: 1_700_000_000_000,
                status: MapStatus::Active,
            }))
            .unwrap();
        store
            .apply(&JournalEvent::FogAdded(FogAdded {
                id: "F1".to_owned(),
                map_id: "M1".to_owned(),
                patch: "Graduated already.".to_owned(),
                axis: None,
                created_at: 1_700_000_000_000,
            }))
            .unwrap();
        store
            .apply(&JournalEvent::FogGraduated(crate::journal::FogGraduated {
                id: "F1".to_owned(),
                graduated_to: "T9".to_owned(),
            }))
            .unwrap();

        let snapshot = load(&store, "M1").unwrap();
        assert!(snapshot.fog.is_empty());
    }
}
