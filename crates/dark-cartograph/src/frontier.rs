//! The frontier: tickets a session can take right now, and claim leases.
//!
//! The frontier is every ticket that is [`TicketStatus::Open`] and not
//! blocked by a ticket that has not yet resolved or left scope (see
//! [`frontier`]). Taking a ticket off the frontier means claiming it
//! ([`claim`]), which holds it under a lease so that two sessions never
//! work the same ticket at once. [`reap_expired`] returns a claim whose
//! lease ran out, so a crashed session cannot hold a ticket forever.
//!
//! # Why a claim writes the database before the journal
//!
//! Everywhere else in this crate the journal is the source of truth and
//! a caller appends to it before touching the database (see
//! `crate::store::Store::add_edge`). A claim is the one exception, and
//! deliberately so: only the database can arbitrate between two sessions
//! that call [`claim`] on the same ticket at the same instant, because
//! `SQLite` serialises the conditional `UPDATE` below across processes
//! and connections, while two journal files being appended to
//! concurrently have no such arbiter. [`claim`] therefore issues the
//! conditional `UPDATE` first — it is the compare-and-swap that decides
//! the one winner — and appends the matching event to the journal only
//! after the database confirms this caller won.
//!
//! This leaves one narrow window: a crash between the winning `UPDATE`
//! and the journal append leaves the database saying `claimed` while the
//! journal, and so a later [`crate::store::Store::rebuild`], say `open`.
//! That is the safe direction to fail in: a rebuild after such a crash
//! makes the ticket claimable again, exactly as [`reap_expired`] would
//! have done anyway once the lease ran out. No rebuild ever invents a
//! claim that a session did not actually win.

use std::path::Path;
use std::time::Duration;

use dark_contract::{EventTx, Result};
use rusqlite::params;

use crate::journal::{self, JournalEvent, TicketStatus, TicketType, TicketUpdated, Timestamp};
use crate::store::{Store, sql_failed};

/// The default claim lease: two hours, in milliseconds.
///
/// Matches the build specification's default (task unit `D2`, step 3).
/// [`Timestamp`] is milliseconds since the epoch, so this constant adds
/// directly to a `claimed_at` value.
pub const DEFAULT_LEASE_MS: i64 = 2 * 60 * 60 * 1000;

/// How long a database write waits for a lock that another connection
/// holds, before giving up.
///
/// Several sessions call [`claim`] against the same `SQLite` file from
/// separate connections. Without a busy timeout, `SQLite` returns
/// `SQLITE_BUSY` immediately on contention instead of waiting its turn,
/// which would turn a benign race into a spurious error.
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

/// One ticket that a session could claim right now.
///
/// Built from the frontier query in task unit `D2`, step 1: open,
/// unblocked, ordered by `ordinal`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrontierTicket {
    /// The ticket's identifier.
    pub id: String,
    /// The ticket's short name.
    pub name: String,
    /// The question that the ticket answers.
    pub question: String,
    /// The kind of ticket.
    pub ticket_type: TicketType,
    /// `true` when the ticket needs a person (a human in the loop).
    pub hitl: bool,
    /// The ticket's position among its siblings. Lower sorts first.
    pub ordinal: i64,
}

/// The result of one call to [`claim`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClaimOutcome {
    /// The caller now holds the ticket.
    Claimed {
        /// The claimed ticket's identifier.
        ticket_id: String,
        /// When this claim's lease expires. [`reap_expired`] returns the
        /// ticket to the frontier once `now` reaches this value.
        expires_at: Timestamp,
    },
    /// The ticket was not open — another session already claimed it, it
    /// does not exist, it belongs to a different map, or it has already
    /// resolved. The caller holds nothing.
    NotAvailable,
}

/// Returns every ticket on `map_id`'s frontier: open, and not blocked by
/// a ticket that has not resolved or left scope.
///
/// Runs the query task unit `D2` names in step 1, verbatim except for
/// the map filter and the trailing order, which the build specification
/// already gives. A cycle in the blocking edges cannot appear here: `D1`
/// rejects one at insert time (`crate::store::Store::add_edge`), so this
/// function never walks the edge graph itself and cannot loop on one.
///
/// # Errors
///
/// Returns an error when the underlying query fails, or when a stored
/// `type` value does not match [`TicketType::as_str`] for a known
/// variant — which only happens if a row was written outside this crate.
pub fn frontier(store: &Store, map_id: &str) -> Result<Vec<FrontierTicket>> {
    let conn = store.connection();
    let mut stmt = conn
        .prepare(
            "WITH blocked AS (
               SELECT e.blocked AS id FROM edges e JOIN tickets t ON t.id = e.blocker
               WHERE t.status NOT IN ('resolved','out_of_scope')
             )
             SELECT id, name, question, type, hitl, ordinal FROM tickets
             WHERE map_id = ?1 AND status = 'open' AND id NOT IN (SELECT id FROM blocked)
             ORDER BY ordinal",
        )
        .map_err(|err| sql_failed(format!("cannot prepare the frontier query: {err}")))?;

    let rows = stmt
        .query_map(params![map_id], |row| {
            let type_str: String = row.get(3)?;
            let hitl_int: i64 = row.get(4)?;
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                type_str,
                hitl_int != 0,
                row.get::<_, i64>(5)?,
            ))
        })
        .map_err(|err| sql_failed(format!("cannot run the frontier query: {err}")))?;

    let mut tickets = Vec::new();
    for row in rows {
        let (id, name, question, type_str, hitl, ordinal) =
            row.map_err(|err| sql_failed(format!("cannot read a frontier row: {err}")))?;
        let ticket_type = parse_ticket_type(&type_str)?;
        tickets.push(FrontierTicket {
            id,
            name,
            question,
            ticket_type,
            hitl,
            ordinal,
        });
    }
    Ok(tickets)
}

/// Claims `ticket_id` on `map_id` for `claimed_by`, under a lease that
/// runs for `lease_ms` from `now`.
///
/// Only one caller ever wins a claim on a given ticket, even when many
/// callers race on it at once — see the module documentation for how the
/// database arbitrates that. A winning claim durably appends a
/// [`JournalEvent::TicketUpdated`] recording the new status, the
/// claimant, and the claim time, then applies the same change here.
///
/// Returns [`ClaimOutcome::NotAvailable`] rather than an error when the
/// ticket cannot be claimed: losing a race is an ordinary outcome for a
/// tool a model calls routinely, not a failure.
///
/// # Errors
///
/// Returns an error when the database cannot be reached or written, or
/// when a winning claim's journal append fails. A failed append after a
/// winning `UPDATE` leaves the database ahead of the journal; see the
/// module documentation for why that is the safe direction to fail in.
pub fn claim(
    store: &mut Store,
    maps_root: &Path,
    map_id: &str,
    ticket_id: &str,
    claimed_by: &str,
    now: Timestamp,
    lease_ms: i64,
) -> Result<ClaimOutcome> {
    let conn = store.connection();
    conn.busy_timeout(BUSY_TIMEOUT)
        .map_err(|err| sql_failed(format!("cannot set the busy timeout: {err}")))?;

    // The `WHERE status = 'open'` guard is the compare-and-swap: SQLite
    // serialises this statement against every other writer on the same
    // file, so at most one caller's `UPDATE` can match this row before
    // its status stops being 'open'.
    let changed = conn
        .execute(
            "UPDATE tickets SET status = 'claimed', claimed_by = ?1, claimed_at = ?2
             WHERE id = ?3 AND map_id = ?4 AND status = 'open'",
            params![claimed_by, now, ticket_id, map_id],
        )
        .map_err(|err| sql_failed(format!("cannot claim ticket {ticket_id}: {err}")))?;

    if changed == 0 {
        return Ok(ClaimOutcome::NotAvailable);
    }

    let event = JournalEvent::TicketUpdated(TicketUpdated {
        id: ticket_id.to_owned(),
        status: Some(TicketStatus::Claimed),
        claimed_by: Some(claimed_by.to_owned()),
        claimed_at: Some(now),
        ..TicketUpdated::default()
    });
    journal::append(maps_root, map_id, &event)?;

    Ok(ClaimOutcome::Claimed {
        ticket_id: ticket_id.to_owned(),
        expires_at: now + lease_ms,
    })
}

/// Returns every claim on `map_id` whose lease expired at or before
/// `now` to the frontier, and emits a [`dark_contract::Event::Notice`]
/// on `events` for each one.
///
/// A ticket's lease expires at `claimed_at + lease_ms`. Reaping it sets
/// its status back to [`TicketStatus::Open`] — through the same
/// journal-then-apply order every other write in this crate uses, since
/// no concurrent claim can race a reap for the same ticket: a ticket
/// that is not `'claimed'` and past its lease was already reaped or
/// re-claimed by the time this function's `UPDATE` would run, and that
/// `UPDATE` carries the same `'claimed'` guard as [`claim`]'s to make
/// sure of it.
///
/// The `claimed_by` and `claimed_at` columns are left as they were.
/// [`crate::store::Store::apply`] only ever replaces a field that the
/// matching journal event names, and a reap has no new claimant to name
/// — the stale values point at the session whose lease ran out, and the
/// next successful [`claim`] overwrites them anyway.
///
/// Returns the identifiers of every ticket reaped, in no particular
/// order.
///
/// # Errors
///
/// Returns an error when the database cannot be reached, or when a
/// journal append fails partway through — in which case the database and
/// the journal may disagree about the tickets reaped so far, in the same
/// safe direction the module documentation describes for [`claim`].
pub fn reap_expired(
    store: &mut Store,
    maps_root: &Path,
    map_id: &str,
    now: Timestamp,
    lease_ms: i64,
    events: &EventTx,
) -> Result<Vec<String>> {
    let cutoff = now - lease_ms;

    let expired: Vec<(String, Option<String>)> = {
        let conn = store.connection();
        let mut stmt = conn
            .prepare(
                "SELECT id, claimed_by FROM tickets
                 WHERE map_id = ?1 AND status = 'claimed'
                   AND claimed_at IS NOT NULL AND claimed_at <= ?2
                 ORDER BY id",
            )
            .map_err(|err| sql_failed(format!("cannot prepare the expired-lease query: {err}")))?;
        let rows = stmt
            .query_map(params![map_id, cutoff], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
            })
            .map_err(|err| sql_failed(format!("cannot run the expired-lease query: {err}")))?;
        rows.collect::<rusqlite::Result<_>>()
            .map_err(|err| sql_failed(format!("cannot read an expired-lease row: {err}")))?
    };

    let mut reaped = Vec::new();
    for (ticket_id, claimed_by) in expired {
        let event = JournalEvent::TicketUpdated(TicketUpdated {
            id: ticket_id.clone(),
            status: Some(TicketStatus::Open),
            // Clear the claimant as well as the status. A session that
            // abandoned a ticket must not stay recorded against it: the
            // next person to read the row would see an owner who is not
            // working on it.
            release_claim: true,
            ..TicketUpdated::default()
        });
        journal::append(maps_root, map_id, &event)?;
        store.apply(&event)?;

        let holder = claimed_by.as_deref().unwrap_or("an unknown session");
        events.notice(format!(
            "ticket {ticket_id}'s claim lease expired (held by {holder}); \
             it returned to the frontier"
        ));
        reaped.push(ticket_id);
    }
    Ok(reaped)
}

/// Parses a `tickets.type` value back into a [`TicketType`].
fn parse_ticket_type(value: &str) -> Result<TicketType> {
    match value {
        "research" => Ok(TicketType::Research),
        "prototype" => Ok(TicketType::Prototype),
        "grilling" => Ok(TicketType::Grilling),
        "task" => Ok(TicketType::Task),
        other => Err(sql_failed(format!(
            "unrecognised tickets.type value {other:?}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::journal::{MapCreated, MapStatus, TicketCreated};
    use dark_contract::{Event, EventBus, Received};
    use tempfile::TempDir;

    struct Fixture {
        _tmp: TempDir,
        maps_root: std::path::PathBuf,
        store: Store,
    }

    fn setup() -> Fixture {
        let tmp = TempDir::new().expect("tempdir");
        let repo_root = tmp.path().join("repo");
        let maps_root = tmp.path().join("maps");
        let store = Store::open(&repo_root).expect("open store");
        Fixture {
            _tmp: tmp,
            maps_root,
            store,
        }
    }

    fn create_map(store: &mut Store, id: &str) {
        store
            .apply(&JournalEvent::MapCreated(MapCreated {
                id: id.to_owned(),
                name: "Test map".to_owned(),
                destination: "A tested destination".to_owned(),
                notes: None,
                created_at: 1_700_000_000_000,
                status: MapStatus::Active,
            }))
            .unwrap();
    }

    fn create_ticket(store: &mut Store, map_id: &str, id: &str, ordinal: i64) {
        store
            .apply(&JournalEvent::TicketCreated(TicketCreated {
                id: id.to_owned(),
                map_id: map_id.to_owned(),
                name: format!("Ticket {id}"),
                question: format!("What does {id} answer?"),
                ticket_type: TicketType::Task,
                hitl: false,
                status: TicketStatus::Open,
                created_at: 1_700_000_000_000,
                ordinal,
                axis: None,
                tokens_used: None,
            }))
            .unwrap();
    }

    #[test]
    fn frontier_lists_open_unblocked_tickets_in_ordinal_order() {
        let mut fx = setup();
        create_map(&mut fx.store, "M1");
        create_ticket(&mut fx.store, "M1", "T2", 1);
        create_ticket(&mut fx.store, "M1", "T1", 0);

        let tickets = frontier(&fx.store, "M1").unwrap();
        let ids: Vec<&str> = tickets.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids, vec!["T1", "T2"], "must be ordered by ordinal");
    }

    #[test]
    fn frontier_excludes_a_ticket_blocked_by_an_unresolved_ticket() {
        let mut fx = setup();
        create_map(&mut fx.store, "M1");
        create_ticket(&mut fx.store, "M1", "T1", 0);
        create_ticket(&mut fx.store, "M1", "T2", 1);
        fx.store.add_edge(&fx.maps_root, "M1", "T1", "T2").unwrap();

        let ids: Vec<String> = frontier(&fx.store, "M1")
            .unwrap()
            .into_iter()
            .map(|t| t.id)
            .collect();
        assert_eq!(ids, vec!["T1"], "T2 is blocked by open T1");
    }

    #[test]
    fn frontier_includes_a_ticket_blocked_only_by_a_resolved_blocker() {
        let mut fx = setup();
        create_map(&mut fx.store, "M1");
        create_ticket(&mut fx.store, "M1", "T1", 0);
        create_ticket(&mut fx.store, "M1", "T2", 1);
        fx.store.add_edge(&fx.maps_root, "M1", "T1", "T2").unwrap();
        fx.store
            .apply(&JournalEvent::TicketUpdated(TicketUpdated {
                id: "T1".to_owned(),
                status: Some(TicketStatus::Resolved),
                resolved_at: Some(1_700_000_010_000),
                ..TicketUpdated::default()
            }))
            .unwrap();

        let ids: Vec<String> = frontier(&fx.store, "M1")
            .unwrap()
            .into_iter()
            .map(|t| t.id)
            .collect();
        assert_eq!(ids, vec!["T2"], "T1 resolved, so T2 is takeable");
    }

    #[test]
    fn frontier_excludes_a_claimed_ticket() {
        let mut fx = setup();
        create_map(&mut fx.store, "M1");
        create_ticket(&mut fx.store, "M1", "T1", 0);
        claim(
            &mut fx.store,
            &fx.maps_root,
            "M1",
            "T1",
            "session-a",
            1_700_000_000_000,
            DEFAULT_LEASE_MS,
        )
        .unwrap();

        assert!(frontier(&fx.store, "M1").unwrap().is_empty());
    }

    #[test]
    fn claim_succeeds_on_an_open_ticket() {
        let mut fx = setup();
        create_map(&mut fx.store, "M1");
        create_ticket(&mut fx.store, "M1", "T1", 0);

        let outcome = claim(
            &mut fx.store,
            &fx.maps_root,
            "M1",
            "T1",
            "session-a",
            1_700_000_000_000,
            DEFAULT_LEASE_MS,
        )
        .unwrap();

        assert_eq!(
            outcome,
            ClaimOutcome::Claimed {
                ticket_id: "T1".to_owned(),
                expires_at: 1_700_000_000_000 + DEFAULT_LEASE_MS,
            }
        );

        let status: String = fx
            .store
            .connection()
            .query_row("SELECT status FROM tickets WHERE id = 'T1'", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(status, "claimed");

        let events = journal::read_events(&fx.maps_root, "M1").unwrap();
        assert!(events.iter().any(|e| matches!(
            e,
            JournalEvent::TicketUpdated(u)
                if u.id == "T1" && u.claimed_by.as_deref() == Some("session-a")
        )));
    }

    #[test]
    fn claim_on_an_already_claimed_ticket_returns_not_available() {
        let mut fx = setup();
        create_map(&mut fx.store, "M1");
        create_ticket(&mut fx.store, "M1", "T1", 0);
        claim(
            &mut fx.store,
            &fx.maps_root,
            "M1",
            "T1",
            "session-a",
            1_700_000_000_000,
            DEFAULT_LEASE_MS,
        )
        .unwrap();

        let outcome = claim(
            &mut fx.store,
            &fx.maps_root,
            "M1",
            "T1",
            "session-b",
            1_700_000_001_000,
            DEFAULT_LEASE_MS,
        )
        .unwrap();

        assert_eq!(outcome, ClaimOutcome::NotAvailable);
    }

    #[test]
    fn claim_on_an_unknown_ticket_returns_not_available_not_an_error() {
        let mut fx = setup();
        create_map(&mut fx.store, "M1");

        let outcome = claim(
            &mut fx.store,
            &fx.maps_root,
            "M1",
            "no-such-ticket",
            "session-a",
            1_700_000_000_000,
            DEFAULT_LEASE_MS,
        )
        .unwrap();

        assert_eq!(outcome, ClaimOutcome::NotAvailable);
    }

    #[tokio::test]
    async fn reap_expired_returns_a_stale_claim_to_the_frontier_and_notices() {
        let mut fx = setup();
        create_map(&mut fx.store, "M1");
        create_ticket(&mut fx.store, "M1", "T1", 0);
        claim(
            &mut fx.store,
            &fx.maps_root,
            "M1",
            "T1",
            "session-a",
            1_700_000_000_000,
            DEFAULT_LEASE_MS,
        )
        .unwrap();

        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        let now = 1_700_000_000_000 + DEFAULT_LEASE_MS + 1;
        let reaped = reap_expired(
            &mut fx.store,
            &fx.maps_root,
            "M1",
            now,
            DEFAULT_LEASE_MS,
            &bus.tx(),
        )
        .unwrap();

        assert_eq!(reaped, vec!["T1".to_owned()]);
        assert_eq!(
            frontier(&fx.store, "M1").unwrap().len(),
            1,
            "T1 must be back on the frontier"
        );

        let received = rx.recv().await.expect("a notice was sent");
        let Received::Event(Event::Notice(text)) = received else {
            panic!("expected a Notice event, got {received:?}");
        };
        assert!(text.contains("T1"));

        // The claimant must be cleared, not merely the status. Every other
        // field on TicketUpdated means "None leaves it alone", so this only
        // works through `release_claim`. See `TicketUpdated::release_claim`.
        let claimed_by: Option<String> = fx
            .store
            .connection()
            .query_row(
                "SELECT claimed_by FROM tickets WHERE id = 'T1'",
                [],
                |row| row.get(0),
            )
            .expect("the ticket row is readable");
        assert_eq!(
            claimed_by, None,
            "a reaped ticket must not keep the session that abandoned it"
        );

        let events = journal::read_events(&fx.maps_root, "M1").unwrap();
        assert!(events.iter().any(|e| matches!(
            e,
            JournalEvent::TicketUpdated(u) if u.id == "T1" && u.status == Some(TicketStatus::Open)
        )));
    }

    #[test]
    fn reap_expired_leaves_a_claim_inside_its_lease_alone() {
        let mut fx = setup();
        create_map(&mut fx.store, "M1");
        create_ticket(&mut fx.store, "M1", "T1", 0);
        claim(
            &mut fx.store,
            &fx.maps_root,
            "M1",
            "T1",
            "session-a",
            1_700_000_000_000,
            DEFAULT_LEASE_MS,
        )
        .unwrap();

        let bus = EventBus::new();
        let still_leased = 1_700_000_000_000 + (DEFAULT_LEASE_MS / 2);
        let reaped = reap_expired(
            &mut fx.store,
            &fx.maps_root,
            "M1",
            still_leased,
            DEFAULT_LEASE_MS,
            &bus.tx(),
        )
        .unwrap();

        assert!(reaped.is_empty());
        assert!(frontier(&fx.store, "M1").unwrap().is_empty());
    }

    #[test]
    fn frontier_never_loops_on_a_cyclical_looking_query_shape() {
        // D1 rejects a cycle at insert time, so this crate never has to
        // walk the edge graph to protect the frontier query itself. This
        // test pins that: even a dense, deliberately tangled but acyclic
        // set of edges must return promptly.
        let mut fx = setup();
        create_map(&mut fx.store, "M1");
        for i in 0..20 {
            create_ticket(&mut fx.store, "M1", &format!("T{i}"), i);
        }
        for i in 0..19 {
            fx.store
                .add_edge(
                    &fx.maps_root,
                    "M1",
                    &format!("T{i}"),
                    &format!("T{}", i + 1),
                )
                .unwrap();
        }

        let ids: Vec<String> = frontier(&fx.store, "M1")
            .unwrap()
            .into_iter()
            .map(|t| t.id)
            .collect();
        assert_eq!(ids, vec!["T0".to_owned()]);
    }
}
