//! Every ticket in one map, with its status and its blockers.
//!
//! [`crate::frontier::frontier`] answers a different question — which
//! tickets are takeable right now — and deliberately returns only those.
//! A map *drawing* needs the opposite: every ticket, including the
//! resolved and the out-of-scope ones, because the shape of the map is
//! what a person reads. `dark-tui`'s fog map places a ticket by its
//! distance from the destination through the blocking edges, so it needs
//! those edges too.
//!
//! This is a read. It opens no journal, writes no row, and reaches no
//! network.

use dark_contract::Result;
use rusqlite::params;

use crate::journal::TicketStatus;
use crate::store::{Store, sql_failed};

/// One ticket, as a map drawing needs it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MapTicket {
    /// The ticket identifier.
    pub id: String,
    /// The ticket's short name.
    pub name: String,
    /// Where the ticket has got to.
    pub status: TicketStatus,
    /// The identifiers of the tickets that must resolve before this one is
    /// takeable, in a stable order.
    pub blocked_by: Vec<String>,
}

/// Parses a stored `status` value.
///
/// # Errors
///
/// Returns an error for a value outside [`TicketStatus`], which only
/// happens if a row was written outside this crate — the `tickets.status`
/// CHECK constraint allows exactly these five.
fn parse_status(value: &str) -> Result<TicketStatus> {
    match value {
        "open" => Ok(TicketStatus::Open),
        "claimed" => Ok(TicketStatus::Claimed),
        "resolved" => Ok(TicketStatus::Resolved),
        "out_of_scope" => Ok(TicketStatus::OutOfScope),
        "invalidated" => Ok(TicketStatus::Invalidated),
        other => Err(sql_failed(format!("unknown ticket status {other:?}"))),
    }
}

/// Returns every ticket in `map_id`, ordered by `ordinal` then `id`.
///
/// The order is total and comes from stored columns alone, so two calls
/// against the same database return the same vector — which is what lets
/// a caller hash or compare a drawing built from it. Blocker lists are
/// ordered the same way.
///
/// # Errors
///
/// Returns an error when a query fails, or when a stored `status` value
/// does not name a [`TicketStatus`].
// `blocker` and `blocked` are the schema's own column names (see
// `store::Store::add_edge`, which carries the same allow for the same
// reason): renaming one to satisfy `similar_names` would make this harder
// to match against the schema it reads, not easier.
#[allow(clippy::similar_names)]
pub fn map_snapshot(store: &Store, map_id: &str) -> Result<Vec<MapTicket>> {
    let conn = store.connection();

    let mut stmt = conn
        .prepare(
            "SELECT id, name, status FROM tickets
             WHERE map_id = ?1
             ORDER BY ordinal, id",
        )
        .map_err(|err| sql_failed(format!("cannot prepare the map snapshot query: {err}")))?;

    let rows = stmt
        .query_map(params![map_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .map_err(|err| sql_failed(format!("cannot run the map snapshot query: {err}")))?;

    let mut tickets = Vec::new();
    for row in rows {
        let (id, name, status) =
            row.map_err(|err| sql_failed(format!("cannot read a map snapshot row: {err}")))?;
        tickets.push(MapTicket {
            id,
            name,
            status: parse_status(&status)?,
            blocked_by: Vec::new(),
        });
    }

    let mut edges = conn
        .prepare(
            "SELECT e.blocked, e.blocker FROM edges e
             JOIN tickets t ON t.id = e.blocked
             WHERE t.map_id = ?1
             ORDER BY e.blocked, e.blocker",
        )
        .map_err(|err| sql_failed(format!("cannot prepare the map edge query: {err}")))?;

    let edge_rows = edges
        .query_map(params![map_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(|err| sql_failed(format!("cannot run the map edge query: {err}")))?;

    for row in edge_rows {
        let (blocked, blocker) =
            row.map_err(|err| sql_failed(format!("cannot read a map edge row: {err}")))?;
        if let Some(ticket) = tickets.iter_mut().find(|t| t.id == blocked) {
            ticket.blocked_by.push(blocker);
        }
    }

    Ok(tickets)
}

/// Returns the destination `map_id` is charting a way towards.
///
/// Returns `None` when no map has that identifier, which is what a
/// `MapChanged` naming a map this repository does not hold looks like.
///
/// # Errors
///
/// Returns an error when the query itself fails. A missing row is `None`,
/// not an error.
pub fn map_destination(store: &Store, map_id: &str) -> Result<Option<String>> {
    store
        .connection()
        .query_row(
            "SELECT destination FROM maps WHERE id = ?1",
            params![map_id],
            |row| row.get::<_, String>(0),
        )
        .map(Some)
        .or_else(|err| match err {
            rusqlite::Error::QueryReturnedNoRows => Ok(None),
            other => Err(sql_failed(format!(
                "cannot read the map destination: {other}"
            ))),
        })
}

/// Returns every map identifier in the store, sorted.
///
/// # Errors
///
/// Returns an error when the query fails.
pub fn map_ids(store: &Store) -> Result<Vec<String>> {
    let conn = store.connection();
    let mut stmt = conn
        .prepare("SELECT id FROM maps ORDER BY id")
        .map_err(|err| sql_failed(format!("cannot prepare the map list query: {err}")))?;
    let rows = stmt
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|err| sql_failed(format!("cannot run the map list query: {err}")))?;

    let mut ids = Vec::new();
    for row in rows {
        ids.push(row.map_err(|err| sql_failed(format!("cannot read a map row: {err}")))?);
    }
    Ok(ids)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::journal::{
        JournalEvent, MapCreated, MapStatus, TicketCreated, TicketType, Timestamp,
    };
    use tempfile::TempDir;

    fn store(dir: &TempDir) -> Store {
        let mut store = Store::open(dir.path()).expect("a store opens");
        store
            .apply(&JournalEvent::MapCreated(MapCreated {
                id: "M1".to_owned(),
                name: "the map".to_owned(),
                destination: "T4".to_owned(),
                notes: None,
                created_at: 0 as Timestamp,
                status: MapStatus::Active,
            }))
            .expect("the map is created");
        store
    }

    fn create(store: &mut Store, id: &str, ordinal: i64, status: TicketStatus) {
        store
            .apply(&JournalEvent::TicketCreated(TicketCreated {
                id: id.to_owned(),
                map_id: "M1".to_owned(),
                name: format!("ticket {id}"),
                question: "why?".to_owned(),
                ticket_type: TicketType::Task,
                hitl: false,
                status,
                created_at: 0 as Timestamp,
                ordinal,
                axis: None,
                tokens_used: None,
            }))
            .expect("the ticket is created");
    }

    #[test]
    fn an_empty_map_snapshots_to_nothing() {
        let dir = TempDir::new().unwrap();
        let store = store(&dir);
        assert!(map_snapshot(&store, "M1").unwrap().is_empty());
    }

    #[test]
    fn every_status_is_returned_not_only_the_takeable_ones() {
        // `frontier` returns the open, unblocked tickets. A drawing needs
        // the resolved and out-of-scope ones too, or the map it draws is
        // not the map.
        let dir = TempDir::new().unwrap();
        let mut store = store(&dir);
        create(&mut store, "T1", 1, TicketStatus::Open);
        create(&mut store, "T2", 2, TicketStatus::Resolved);
        create(&mut store, "T3", 3, TicketStatus::OutOfScope);
        create(&mut store, "T4", 4, TicketStatus::Claimed);

        let snapshot = map_snapshot(&store, "M1").unwrap();
        let ids: Vec<&str> = snapshot.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids, ["T1", "T2", "T3", "T4"]);
        assert_eq!(snapshot[1].status, TicketStatus::Resolved);
        assert_eq!(snapshot[2].status, TicketStatus::OutOfScope);
    }

    #[test]
    fn a_blocking_edge_appears_on_the_blocked_ticket() {
        let dir = TempDir::new().unwrap();
        let mut store = store(&dir);
        create(&mut store, "T1", 1, TicketStatus::Open);
        create(&mut store, "T2", 2, TicketStatus::Open);
        store
            .add_edge(dir.path(), "M1", "T1", "T2")
            .expect("T1 blocks T2");

        let snapshot = map_snapshot(&store, "M1").unwrap();
        let t2 = snapshot.iter().find(|t| t.id == "T2").expect("T2 is there");
        assert_eq!(t2.blocked_by, ["T1"]);
        let t1 = snapshot.iter().find(|t| t.id == "T1").expect("T1 is there");
        assert!(t1.blocked_by.is_empty());
    }

    #[test]
    fn two_snapshots_of_one_store_are_identical() {
        // A drawing built from this is compared and hashed, so the order
        // must come from the stored columns, never from the order rows
        // happen to arrive in.
        let dir = TempDir::new().unwrap();
        let mut store = store(&dir);
        for (n, id) in ["T3", "T1", "T2"].iter().enumerate() {
            create(
                &mut store,
                id,
                i64::try_from(n).unwrap(),
                TicketStatus::Open,
            );
        }
        assert_eq!(
            map_snapshot(&store, "M1").unwrap(),
            map_snapshot(&store, "M1").unwrap()
        );
    }

    #[test]
    fn the_destination_comes_back_for_a_map_that_exists() {
        let dir = TempDir::new().unwrap();
        let store = store(&dir);
        assert_eq!(
            map_destination(&store, "M1").unwrap(),
            Some("T4".to_owned())
        );
    }

    #[test]
    fn a_missing_map_has_no_destination_and_is_not_an_error() {
        let dir = TempDir::new().unwrap();
        let store = store(&dir);
        assert_eq!(map_destination(&store, "nope").unwrap(), None);
    }

    #[test]
    fn map_ids_names_every_map() {
        let dir = TempDir::new().unwrap();
        let store = store(&dir);
        assert_eq!(map_ids(&store).unwrap(), ["M1"]);
    }

    #[test]
    fn another_maps_tickets_are_not_included() {
        let dir = TempDir::new().unwrap();
        let mut store = store(&dir);
        create(&mut store, "T1", 1, TicketStatus::Open);
        assert!(map_snapshot(&store, "M2").unwrap().is_empty());
    }
}
