//! The ticket tools: the [`dark_contract::Tool`] implementations that let a
//! model change a map.
//!
//! Every mutating tool here writes through the same two-step path every
//! other write in this crate uses: append a
//! [`crate::journal::JournalEvent`] to the journal, then apply that same
//! event to the [`crate::store::Store`] (see [`journal_then_apply`]). A
//! tool never issues an `UPDATE` or `INSERT` of its own. [`ticket_block`]
//! is the one exception, because it needs [`crate::store::Store::add_edge`]
//! for the cycle check that only that function performs.
//!
//! # Two things `ToolCtx` does not carry
//!
//! [`dark_contract::ToolCtx`] gives a tool the repository root
//! (`ctx.root`, which this module treats as `Store`'s `repo_root`) and
//! whether a person is present for this call (`ctx.human_present`, Rule
//! 19). It carries nothing else this module needs, so two more things
//! travel through [`CartographSession`] instead, built once per harness
//! session and shared, through an [`std::sync::Arc`], across every
//! mutating tool — the same shape `dark_tools::fs::file_tools` uses for
//! its own session-scoped [`state`](dark_tools) (`ReadState`):
//!
//! - **`maps_root`**, because a map's `journal.jsonl` lives under
//!   `$DARK_HOME/maps`, a path with no relationship to the repository
//!   root. See the module documentation on [`crate::journal`] for why
//!   this crate never reads `$DARK_HOME` itself.
//! - **The claimant identity and Rule 20's one-resolution counter**,
//!   because both must survive from one tool call to the next inside a
//!   session, and `ToolCtx` is rebuilt fresh for every call.
//!
//! # Tool tiers
//!
//! [`ticket_tools`] assigns [`dark_contract::tool::tier::ESSENTIAL`] to
//! `map_read`, `ticket_claim`, `ticket_zoom`, and `ticket_resolve` — the
//! four calls that resolving a ticket needs, which task unit `E7` routes
//! to every ticket type down to a sub-8B scout. Every other tool here
//! authors a map (creates a ticket, wires an edge, writes fog, excludes
//! something from scope), and task unit `I3`'s `allow_charting` flag
//! already draws that same line at the 8B boundary, so this module
//! assigns those tools [`dark_contract::tool::tier::STANDARD`]. The build
//! specification does not name tiers for these tools directly; this is
//! this task unit's own reading of that boundary.

mod fog_graduate;
mod fog_write;
mod map_create;
mod map_read;
mod scope_exclude;
mod ticket_block;
mod ticket_claim;
mod ticket_create;
mod ticket_invalidate;
mod ticket_resolve;
mod ticket_zoom;

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::{SystemTime, UNIX_EPOCH};

use dark_contract::{ErrCode, Error, Result, Tool};
use rusqlite::params;

use crate::journal::{self, JournalEvent, TicketStatus, TicketType, Timestamp};
use crate::store::Store;

pub use fog_graduate::FogGraduate;
pub use fog_write::FogWrite;
pub use map_create::MapCreate;
pub use map_read::MapRead;
pub use scope_exclude::ScopeExclude;
pub use ticket_block::TicketBlock;
pub use ticket_claim::TicketClaim;
pub use ticket_create::TicketCreate;
pub use ticket_invalidate::TicketInvalidate;
pub use ticket_resolve::TicketResolve;
pub use ticket_zoom::TicketZoom;

/// State that every mutating ticket tool needs and that must outlive one
/// tool call. See the module documentation for why `ToolCtx` cannot carry
/// this.
///
/// Build one instance per harness session and share it, through an
/// [`Arc`], across every tool [`ticket_tools`] returns: [`ticket_tools`]
/// does exactly that, and a caller that constructs tools by hand should
/// do the same, because a fresh instance per call would silently reset
/// Rule 20's counter every time.
#[derive(Debug)]
pub struct CartographSession {
    /// Where a map's journal lives: `$DARK_HOME/maps`.
    maps_root: PathBuf,
    /// The identity this session records as `claimed_by` on a ticket it
    /// claims (`ticket_claim`).
    session_id: String,
    /// Set once this session resolves a non-research ticket.
    ///
    /// Rule 20: a second non-research resolution then fails with
    /// [`ErrCode::SessionResolutionLimit`]. A research resolution never
    /// reads or sets this flag — Rule 20 exempts research tickets from
    /// the limit entirely.
    resolved_non_research: AtomicBool,
}

impl CartographSession {
    /// Creates the shared state for one harness session.
    ///
    /// `maps_root` is normally `$DARK_HOME/maps`, resolved once outside
    /// this crate. `session_id` is recorded verbatim as `claimed_by` on
    /// every ticket this session claims.
    #[must_use]
    pub fn new(maps_root: PathBuf, session_id: impl Into<String>) -> Self {
        Self {
            maps_root,
            session_id: session_id.into(),
            resolved_non_research: AtomicBool::new(false),
        }
    }
}

/// Builds the eleven ticket tools for one harness session.
///
/// Call this once per session, the same way `dark_tools::fs::file_tools`
/// builds its file tools once: the returned tools share one
/// [`CartographSession`], and building a fresh set per call would reset
/// Rule 20's one-resolution counter on every turn.
#[must_use]
pub fn ticket_tools(maps_root: PathBuf, session_id: impl Into<String>) -> Vec<Box<dyn Tool>> {
    let session = Arc::new(CartographSession::new(maps_root, session_id));
    vec![
        Box::new(MapRead::new()),
        Box::new(MapCreate::new(session.clone())),
        Box::new(TicketCreate::new(session.clone())),
        Box::new(TicketClaim::new(session.clone())),
        Box::new(TicketZoom::new()),
        Box::new(TicketResolve::new(session.clone())),
        Box::new(TicketBlock::new(session.clone())),
        Box::new(TicketInvalidate::new(session.clone())),
        Box::new(FogWrite::new(session.clone())),
        Box::new(FogGraduate::new(session.clone())),
        Box::new(ScopeExclude::new(session)),
    ]
}

/// Opens the `Store` for the repository that `ctx` names.
///
/// Every tool call opens its own connection rather than holding one open
/// across calls. `SQLite` already has to arbitrate independent
/// connections against the same file — see the module documentation on
/// [`crate::frontier`] — so this costs a file open per call and buys a
/// tool that needs no interior mutability of its own.
fn open_store(ctx: &dark_contract::ToolCtx) -> Result<Store> {
    Store::open(&ctx.root)
}

/// Returns the current time as a [`Timestamp`].
///
/// This is a tool a model calls; it is not part of the context prefix
/// (Rule 6 forbids a clock there, not here), so reading the wall clock at
/// call time is correct.
fn now_ms() -> Timestamp {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    Timestamp::try_from(duration.as_millis()).unwrap_or(Timestamp::MAX)
}

/// Generates a fresh identifier for a map, a ticket, a fog patch, or a
/// scope exclusion.
fn new_id() -> String {
    ulid::Ulid::new().to_string()
}

/// Appends `event` to the journal for `map_id`, then applies it to
/// `store`.
///
/// This is the order every write in this crate uses, [`crate::frontier`]
/// aside: durable first, derived second, so a crash between the two
/// leaves the journal — the source of truth — ahead of the database, and
/// [`Store::rebuild`](crate::store::Store::rebuild) repairs the gap.
///
/// # Errors
///
/// Returns an error when the journal append or the store apply fails.
fn journal_then_apply(
    store: &mut Store,
    maps_root: &std::path::Path,
    map_id: &str,
    event: &JournalEvent,
) -> Result<()> {
    journal::append(maps_root, map_id, event)?;
    store.apply(event)
}

/// Builds an [`Error`] for a database failure that no more specific code
/// covers. Mirrors `crate::store::sql_failed`, which is private to that
/// module.
fn sql_failed(message: String) -> Error {
    Error::new(ErrCode::ToolFailed, message)
}

/// Builds an [`Error`] reporting that `kind` (`"map"`, `"ticket"`, or
/// `"fog patch"`) `id` does not exist.
///
/// [`ErrCode::MapNotFound`] is documented as covering "the map or the
/// ticket"; this crate has no narrower code for a missing fog patch or
/// scope exclusion, so this helper reuses it for those too.
fn not_found(kind: &str, id: &str) -> Error {
    Error::new(ErrCode::MapNotFound, format!("no {kind} with id {id:?}"))
}

/// Builds an [`Error`] reporting that `tool`'s arguments failed to parse.
fn invalid_args(tool: &str, err: impl std::fmt::Display) -> Error {
    Error::new(ErrCode::ToolInvalidArgs, format!("{tool} arguments: {err}"))
}

/// One ticket row, read in full for the tools that need more than the
/// frontier query's narrow projection (`ticket_zoom`, `ticket_resolve`,
/// `ticket_claim`, `ticket_invalidate`, `ticket_block`).
struct TicketRow {
    /// The map this ticket belongs to.
    map_id: String,
    /// The ticket's short name.
    name: String,
    /// The question that the ticket answers.
    question: String,
    /// The kind of ticket.
    ticket_type: TicketType,
    /// `true` when the ticket needs a person.
    hitl: bool,
    /// The ticket's current status.
    status: TicketStatus,
    /// The resolution text, once the ticket has one.
    resolution: Option<String>,
    /// The resolution gist, once the ticket has one.
    gist: Option<String>,
    /// The axis this ticket sits on, when the map has axes.
    axis: Option<String>,
    /// Tokens spent on this ticket, once it has resolved.
    tokens_used: Option<i64>,
}

/// Reads the full row for `ticket_id`.
///
/// # Errors
///
/// Returns [`ErrCode::MapNotFound`] when no ticket has this identifier.
/// Returns an error when the query fails, or when a stored `type` or
/// `status` value does not match a known variant.
fn load_ticket(store: &Store, ticket_id: &str) -> Result<TicketRow> {
    let conn = store.connection();
    let row = conn.query_row(
        "SELECT map_id, name, question, type, hitl, status, resolution, gist, axis, tokens_used
         FROM tickets WHERE id = ?1",
        params![ticket_id],
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, i64>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, Option<String>>(6)?,
                row.get::<_, Option<String>>(7)?,
                row.get::<_, Option<String>>(8)?,
                row.get::<_, Option<i64>>(9)?,
            ))
        },
    );
    match row {
        Ok((
            map_id,
            name,
            question,
            type_str,
            hitl,
            status_str,
            resolution,
            gist,
            axis,
            tokens_used,
        )) => Ok(TicketRow {
            map_id,
            name,
            question,
            ticket_type: parse_ticket_type(&type_str)?,
            hitl: hitl != 0,
            status: parse_ticket_status(&status_str)?,
            resolution,
            gist,
            axis,
            tokens_used,
        }),
        Err(rusqlite::Error::QueryReturnedNoRows) => Err(not_found("ticket", ticket_id)),
        Err(other) => Err(sql_failed(format!(
            "cannot read ticket {ticket_id}: {other}"
        ))),
    }
}

/// The `fog` row fields the tools in this module need.
struct FogRow {
    /// The map this fog patch belongs to.
    map_id: String,
    /// The ticket this patch already graduated to, when it has.
    graduated_to: Option<String>,
}

/// Reads the row for `fog_id`.
///
/// # Errors
///
/// Returns [`ErrCode::MapNotFound`] when no fog patch has this
/// identifier. Returns an error when the query fails.
fn load_fog(store: &Store, fog_id: &str) -> Result<FogRow> {
    let conn = store.connection();
    let row = conn.query_row(
        "SELECT map_id, graduated_to FROM fog WHERE id = ?1",
        params![fog_id],
        |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?)),
    );
    match row {
        Ok((map_id, graduated_to)) => Ok(FogRow {
            map_id,
            graduated_to,
        }),
        Err(rusqlite::Error::QueryReturnedNoRows) => Err(not_found("fog patch", fog_id)),
        Err(other) => Err(sql_failed(format!(
            "cannot read fog patch {fog_id}: {other}"
        ))),
    }
}

/// Confirms that a map with `map_id` exists.
///
/// # Errors
///
/// Returns [`ErrCode::MapNotFound`] when it does not. Returns an error
/// when the query fails.
fn confirm_map_exists(store: &Store, map_id: &str) -> Result<()> {
    let exists: bool = store
        .connection()
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM maps WHERE id = ?1)",
            params![map_id],
            |row| row.get(0),
        )
        .map_err(|err| sql_failed(format!("cannot look up map {map_id}: {err}")))?;
    if exists {
        Ok(())
    } else {
        Err(not_found("map", map_id))
    }
}

/// Parses a `tickets.type` value back into a [`TicketType`].
///
/// A private copy of the same parse this crate already has in
/// `crate::frontier`, which keeps it private to its own module — see that
/// module's `parse_ticket_type` for the precedent this follows.
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

/// Returns an error when `status` is already terminal: resolved,
/// out-of-scope, or invalidated.
///
/// `ticket_resolve` and `ticket_invalidate` both call this before they
/// close a ticket a second way, so a ticket the map already closed keeps
/// the reason it first closed for.
fn require_not_terminal(ticket_id: &str, status: TicketStatus) -> Result<()> {
    match status {
        TicketStatus::Resolved | TicketStatus::OutOfScope | TicketStatus::Invalidated => {
            Err(Error::new(
                ErrCode::ToolInvalidArgs,
                format!(
                    "ticket {ticket_id} is already {}; nothing to change",
                    status.as_str()
                ),
            ))
        }
        TicketStatus::Open | TicketStatus::Claimed => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ticket_tools_returns_the_eleven_tools_at_their_documented_tiers() {
        let tools = ticket_tools(PathBuf::from("/tmp/maps"), "session-a");
        let schemas: Vec<_> = tools.iter().map(|t| t.schema()).collect();

        let names: Vec<&str> = schemas.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "map_read",
                "map_create",
                "ticket_create",
                "ticket_claim",
                "ticket_zoom",
                "ticket_resolve",
                "ticket_block",
                "ticket_invalidate",
                "fog_write",
                "fog_graduate",
                "scope_exclude",
            ]
        );

        for schema in &schemas {
            assert!(
                !schema.description.is_empty(),
                "{} has no description",
                schema.name
            );
            assert!(
                schema.tier == dark_contract::tool::tier::ESSENTIAL
                    || schema.tier == dark_contract::tool::tier::STANDARD,
                "{} has an unexpected tier",
                schema.name
            );
        }

        let mutating: Vec<bool> = schemas.iter().map(|s| s.mutating).collect();
        assert_eq!(
            mutating,
            vec![
                false, true, true, true, false, true, true, true, true, true, true
            ]
        );
    }

    #[test]
    fn now_ms_is_a_plausible_recent_timestamp() {
        // Sanity check, not a golden value: must be after this crate's own
        // fixtures (year 2023) and must not panic on a real clock.
        assert!(now_ms() > 1_700_000_000_000);
    }

    #[test]
    fn new_id_returns_distinct_identifiers() {
        assert_ne!(new_id(), new_id());
    }

    #[test]
    fn require_not_terminal_accepts_open_and_claimed() {
        assert!(require_not_terminal("T1", TicketStatus::Open).is_ok());
        assert!(require_not_terminal("T1", TicketStatus::Claimed).is_ok());
    }

    #[test]
    fn require_not_terminal_rejects_every_closed_status() {
        for status in [
            TicketStatus::Resolved,
            TicketStatus::OutOfScope,
            TicketStatus::Invalidated,
        ] {
            let err = require_not_terminal("T1", status).unwrap_err();
            assert_eq!(err.code, ErrCode::ToolInvalidArgs);
        }
    }
}
