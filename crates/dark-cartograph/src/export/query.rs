//! Reads the plain data every export format renders from.
//!
//! Mirrors `crate::digest::query`'s shape and its purity guarantee — no
//! clock, no environment variable, only the rows `map_id` names — for the
//! same reason: [`super::export`] must render the same bytes for the
//! same map state every time (task unit `D5`, step 1's "pure function of
//! the map state" requirement, which the top-level report on this task
//! unit treats the same way task unit `D3` treats the digest). This is a
//! private copy rather than a shared one: `crate::digest::query`'s types
//! are `pub(super)` to `crate::digest`, not visible here, and the crate
//! already accepts this small duplication elsewhere — see
//! `crate::frontier::parse_ticket_type` and `crate::digest::query`'s own
//! `parse_map_status` for the precedent.

use dark_contract::{ErrCode, Error, Result};
use rusqlite::params;

use crate::journal::{MapStatus, TicketStatus, TicketType};
use crate::store::{Store, sql_failed};

/// One ticket, exactly as an export format needs it.
pub(super) struct ExportTicket {
    /// The ticket's identifier.
    pub id: String,
    /// The ticket's short name.
    pub name: String,
    /// The question that the ticket answers.
    pub question: String,
    /// The kind of ticket.
    pub ticket_type: TicketType,
    /// The ticket's current status.
    pub status: TicketStatus,
    /// The resolution text, once the ticket has one.
    pub resolution: Option<String>,
    /// The resolution gist, once the ticket has one.
    pub gist: Option<String>,
    /// The axis this ticket sits on, when the map has axes.
    pub axis: Option<String>,
}

/// One fog patch that has not yet graduated.
pub(super) struct ExportFog {
    /// The text of the unanswered question.
    pub patch: String,
    /// The axis this fog patch sits on, when the map has axes.
    pub axis: Option<String>,
}

/// One thing the map excludes from its scope.
pub(super) struct ExportScopeExclusion {
    /// A short summary of the excluded thing.
    pub gist: String,
    /// Why the map excludes it.
    pub reason: String,
    /// The ticket that raised this exclusion, when one did.
    pub ticket_id: Option<String>,
}

/// Everything an export format needs about one map, read in one pass.
pub(super) struct ExportSnapshot {
    /// The map's own identifier.
    pub map_id: String,
    /// The map's short name.
    pub name: String,
    /// The destination: what the map is charting a way towards.
    pub destination: String,
    /// Free-text notes about the map, when it has any.
    pub notes: Option<String>,
    /// The map's status.
    pub status: MapStatus,
    /// Every ticket, in ordinal order (then identifier, for a total
    /// order when two tickets share an ordinal).
    pub tickets: Vec<ExportTicket>,
    /// Every blocking edge, as `(blocker, blocked)`, ordered by
    /// `blocked`'s ordinal then `blocker`'s identifier — the order a
    /// reader meets the blocked ticket in [`ExportSnapshot::tickets`].
    pub edges: Vec<(String, String)>,
    /// Fog patches that have not yet graduated, in creation order.
    pub fog: Vec<ExportFog>,
    /// Things the map excludes from its scope, in identifier order.
    pub scope_exclusions: Vec<ExportScopeExclusion>,
}

/// Reads the full [`ExportSnapshot`] for `map_id`.
///
/// # Errors
///
/// Returns [`ErrCode::MapNotFound`] when no map has this identifier.
/// Returns an error when a query fails, or when a stored `type`,
/// `status`, or `maps.status` value does not match a known variant.
pub(super) fn load(store: &Store, map_id: &str) -> Result<ExportSnapshot> {
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

    let tickets = load_tickets(store, map_id)?;
    let edges = load_edges(store, map_id)?;
    let fog = load_fog(store, map_id)?;
    let scope_exclusions = load_scope_exclusions(store, map_id)?;

    Ok(ExportSnapshot {
        map_id: map_id.to_owned(),
        name,
        destination,
        notes,
        status,
        tickets,
        edges,
        fog,
        scope_exclusions,
    })
}

/// Reads every ticket on `map_id`, in ordinal then identifier order.
fn load_tickets(store: &Store, map_id: &str) -> Result<Vec<ExportTicket>> {
    let conn = store.connection();
    let mut stmt = conn
        .prepare(
            "SELECT id, name, question, type, status, resolution, gist, axis
             FROM tickets WHERE map_id = ?1 ORDER BY ordinal, id",
        )
        .map_err(|err| sql_failed(format!("cannot prepare the export tickets query: {err}")))?;
    let rows = stmt
        .query_map(params![map_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, Option<String>>(5)?,
                row.get::<_, Option<String>>(6)?,
                row.get::<_, Option<String>>(7)?,
            ))
        })
        .map_err(|err| sql_failed(format!("cannot run the export tickets query: {err}")))?;

    let mut tickets = Vec::new();
    for row in rows {
        let (id, name, question, type_str, status_str, resolution, gist, axis) =
            row.map_err(|err| sql_failed(format!("cannot read an export ticket row: {err}")))?;
        tickets.push(ExportTicket {
            id,
            name,
            question,
            ticket_type: parse_ticket_type(&type_str)?,
            status: parse_ticket_status(&status_str)?,
            resolution,
            gist,
            axis,
        });
    }
    Ok(tickets)
}

/// Reads every blocking edge on `map_id`, ordered by the blocked
/// ticket's ordinal then the blocker's identifier.
fn load_edges(store: &Store, map_id: &str) -> Result<Vec<(String, String)>> {
    let conn = store.connection();
    let mut stmt = conn
        .prepare(
            "SELECT e.blocker, e.blocked FROM edges e
             JOIN tickets t ON t.id = e.blocked
             WHERE t.map_id = ?1
             ORDER BY t.ordinal, e.blocker",
        )
        .map_err(|err| sql_failed(format!("cannot prepare the export edges query: {err}")))?;
    let rows = stmt
        .query_map(params![map_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(|err| sql_failed(format!("cannot run the export edges query: {err}")))?;
    rows.collect::<rusqlite::Result<_>>()
        .map_err(|err| sql_failed(format!("cannot read an export edge row: {err}")))
}

/// Reads fog patches that have not yet graduated, in creation order.
fn load_fog(store: &Store, map_id: &str) -> Result<Vec<ExportFog>> {
    let conn = store.connection();
    let mut stmt = conn
        .prepare(
            "SELECT patch, axis FROM fog
             WHERE map_id = ?1 AND graduated_to IS NULL
             ORDER BY created_at ASC, id ASC",
        )
        .map_err(|err| sql_failed(format!("cannot prepare the export fog query: {err}")))?;
    let rows = stmt
        .query_map(params![map_id], |row| {
            Ok(ExportFog {
                patch: row.get(0)?,
                axis: row.get(1)?,
            })
        })
        .map_err(|err| sql_failed(format!("cannot run the export fog query: {err}")))?;
    rows.collect::<rusqlite::Result<_>>()
        .map_err(|err| sql_failed(format!("cannot read an export fog row: {err}")))
}

/// Reads scope exclusions in identifier order.
fn load_scope_exclusions(store: &Store, map_id: &str) -> Result<Vec<ExportScopeExclusion>> {
    let conn = store.connection();
    let mut stmt = conn
        .prepare(
            "SELECT gist, reason, ticket_id FROM scope_exclusions
             WHERE map_id = ?1
             ORDER BY id ASC",
        )
        .map_err(|err| {
            sql_failed(format!(
                "cannot prepare the export scope-exclusions query: {err}"
            ))
        })?;
    let rows = stmt
        .query_map(params![map_id], |row| {
            Ok(ExportScopeExclusion {
                gist: row.get(0)?,
                reason: row.get(1)?,
                ticket_id: row.get(2)?,
            })
        })
        .map_err(|err| {
            sql_failed(format!(
                "cannot run the export scope-exclusions query: {err}"
            ))
        })?;
    rows.collect::<rusqlite::Result<_>>()
        .map_err(|err| sql_failed(format!("cannot read an export scope-exclusion row: {err}")))
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

/// Parses a `tickets.status` value back into a [`TicketStatus`].
fn parse_ticket_status(value: &str) -> Result<TicketStatus> {
    match value {
        "open" => Ok(TicketStatus::Open),
        "claimed" => Ok(TicketStatus::Claimed),
        "resolved" => Ok(TicketStatus::Resolved),
        "out_of_scope" => Ok(TicketStatus::OutOfScope),
        "invalidated" => Ok(TicketStatus::Invalidated),
        other => Err(sql_failed(format!(
            "unrecognised tickets.status value {other:?}"
        ))),
    }
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
