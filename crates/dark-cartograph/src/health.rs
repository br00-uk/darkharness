//! Map health checks. Task unit `D5`.
//!
//! [`compute`] reports on ticket sizing quality: how big resolved
//! tickets ran, and how the map's ticket mix breaks down. Two of the
//! four items task unit `D5`, step 4 asks for cannot come from this
//! crate's own schema — see [`Context`]'s documentation for why — so
//! [`compute`] takes them as an explicit parameter instead of guessing
//! at them from data this crate does not have. See this task unit's
//! top-level report for the full account.

use dark_contract::{ErrCode, Error, Result};
use rusqlite::params;

use crate::journal::TicketType;
use crate::store::{Store, sql_failed};

/// Context [`compute`] cannot derive from `crate::store`'s schema on its
/// own, supplied by the caller.
///
/// - **`known_axes`.** Task unit `D5`, step 4's third bullet asks for
///   "each axis that produced no ticket". The schema (task unit `D1`)
///   has no table listing which axes a map's charting swept — only
///   `tickets.axis` and `fog.axis`, which name an axis only when
///   something already answered it. Task unit `E2`'s axis sweep
///   confirms this: an axis that answers "nothing here" produces no
///   ticket and no fog patch, so it leaves no row anywhere this crate
///   can query (`crates/dark-plan/src/chart/pipeline.rs` filters exactly
///   those answers out before anything reaches the journal). Only the
///   caller that ran the sweep — `dark-plan`, not `dark-cartograph` —
///   knows the full axis set, so it must supply it here.
/// - **`compacted_tickets`.** Task unit `D5`, step 4's second bullet
///   asks for "each ticket that caused compaction during its
///   resolution". Compaction is a `dark-core` turn-loop concept
///   (`crates/dark-core/src/context/compact.rs`); nothing about it ever
///   reaches this crate, and neither `tickets` nor `TicketUpdated` (task
///   unit `D1`) has a field to record it. Task unit `E5`
///   (`crates/dark-plan/src/size.rs`) is named as the one that flags
///   such a ticket, but as of this task unit it does not exist yet
///   either. Until a task unit wires a real signal through, an empty
///   list here is honest; a heuristic guess would not be.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Context {
    /// Every axis this map's charting swept, whether or not it produced
    /// a ticket. Leave empty when the caller does not track this yet.
    pub known_axes: Vec<String>,
    /// Ticket identifiers whose resolution triggered a context
    /// compaction. Leave empty when the caller does not track this yet.
    pub compacted_tickets: Vec<String>,
}

/// How `tokens_used` is distributed across a map's resolved tickets.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TokenDistribution {
    /// How many resolved tickets recorded `tokens_used`.
    pub count: usize,
    /// The smallest `tokens_used` value, when any ticket recorded one.
    pub min: Option<i64>,
    /// The largest `tokens_used` value, when any ticket recorded one.
    pub max: Option<i64>,
    /// The mean `tokens_used` value, when any ticket recorded one.
    pub mean: Option<f64>,
    /// The median `tokens_used` value, when any ticket recorded one.
    pub median: Option<f64>,
}

/// One ticket task unit `E5` flagged as having caused a compaction
/// during its own resolution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompactedTicket {
    /// The ticket's identifier.
    pub ticket_id: String,
    /// The ticket's short name.
    pub name: String,
}

/// How many research tickets a map has for every grilling ticket.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TypeRatio {
    /// How many tickets are type `research`.
    pub research: i64,
    /// How many tickets are type `grilling`.
    pub grilling: i64,
}

impl TypeRatio {
    /// Returns `research` divided by `grilling`, or `None` when the map
    /// has no grilling ticket to divide by.
    #[must_use]
    pub fn ratio(self) -> Option<f64> {
        if self.grilling == 0 {
            None
        } else {
            #[allow(clippy::cast_precision_loss)]
            Some(self.research as f64 / self.grilling as f64)
        }
    }
}

/// A map's health report: the four items task unit `D5`, step 4 asks
/// for.
#[derive(Debug, Clone, PartialEq)]
pub struct Health {
    /// The distribution of `tokens_used` across resolved tickets.
    pub tokens_used: TokenDistribution,
    /// Every ticket `context.compacted_tickets` names that this map
    /// actually has, in the order `context.compacted_tickets` gave
    /// them.
    pub compacted: Vec<CompactedTicket>,
    /// The ratio of research tickets to grilling tickets.
    pub research_to_grilling: TypeRatio,
    /// Every axis in `context.known_axes` that no ticket on this map
    /// used, in the order `context.known_axes` gave them.
    pub silent_axes: Vec<String>,
}

/// Computes the health report for `map_id`.
///
/// # Errors
///
/// Returns [`ErrCode::MapNotFound`] when no map has this identifier.
/// Returns an error when the underlying database read fails.
pub fn compute(store: &Store, map_id: &str, context: &Context) -> Result<Health> {
    confirm_map_exists(store, map_id)?;

    let tokens_used = token_distribution(store, map_id)?;
    let compacted = compacted_tickets(store, map_id, &context.compacted_tickets)?;
    let research_to_grilling = type_ratio(store, map_id)?;
    let silent_axes = silent_axes(store, map_id, &context.known_axes)?;

    Ok(Health {
        tokens_used,
        compacted,
        research_to_grilling,
        silent_axes,
    })
}

/// Renders `health` as plain text, for `dark map health` to print.
#[must_use]
pub fn render(health: &Health) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();

    let dist = &health.tokens_used;
    let _ = writeln!(out, "TOKENS USED ({} resolved ticket(s))", dist.count);
    match (dist.min, dist.max, dist.mean, dist.median) {
        (Some(min), Some(max), Some(mean), Some(median)) => {
            let _ = writeln!(
                out,
                "  min {min} · max {max} · mean {mean:.0} · median {median:.0}"
            );
        }
        _ => {
            let _ = writeln!(out, "  (no resolved ticket has recorded tokens_used)");
        }
    }

    let _ = writeln!(out, "\nCOMPACTION ({})", health.compacted.len());
    if health.compacted.is_empty() {
        let _ = writeln!(out, "  (none flagged)");
    } else {
        for ticket in &health.compacted {
            let _ = writeln!(out, "  {} {}", ticket.ticket_id, ticket.name);
        }
    }

    let ratio = &health.research_to_grilling;
    let _ = write!(
        out,
        "\nRESEARCH : GRILLING = {} : {}",
        ratio.research, ratio.grilling
    );
    match ratio.ratio() {
        Some(value) => {
            let _ = writeln!(out, " ({value:.2})");
        }
        None => {
            let _ = writeln!(out, " (no grilling ticket yet)");
        }
    }

    let _ = writeln!(out, "\nSILENT AXES ({})", health.silent_axes.len());
    if health.silent_axes.is_empty() {
        let _ = writeln!(out, "  (none)");
    } else {
        for axis in &health.silent_axes {
            let _ = writeln!(out, "  {axis}");
        }
    }

    out
}

/// Confirms that a map with `map_id` exists.
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
        Err(Error::new(
            ErrCode::MapNotFound,
            format!("no map with id {map_id:?}"),
        ))
    }
}

/// Computes [`TokenDistribution`] over `map_id`'s resolved tickets.
fn token_distribution(store: &Store, map_id: &str) -> Result<TokenDistribution> {
    let conn = store.connection();
    let mut stmt = conn
        .prepare(
            "SELECT tokens_used FROM tickets
             WHERE map_id = ?1 AND status = 'resolved' AND tokens_used IS NOT NULL
             ORDER BY tokens_used",
        )
        .map_err(|err| sql_failed(format!("cannot prepare the tokens_used query: {err}")))?;
    let values: Vec<i64> = stmt
        .query_map(params![map_id], |row| row.get(0))
        .map_err(|err| sql_failed(format!("cannot run the tokens_used query: {err}")))?
        .collect::<rusqlite::Result<_>>()
        .map_err(|err| sql_failed(format!("cannot read a tokens_used row: {err}")))?;

    if values.is_empty() {
        return Ok(TokenDistribution {
            count: 0,
            min: None,
            max: None,
            mean: None,
            median: None,
        });
    }

    #[allow(clippy::cast_precision_loss)]
    let mean = values.iter().sum::<i64>() as f64 / values.len() as f64;
    let median = median_of_sorted(&values);

    Ok(TokenDistribution {
        count: values.len(),
        min: values.first().copied(),
        max: values.last().copied(),
        mean: Some(mean),
        median: Some(median),
    })
}

/// Returns the median of an already-sorted, non-empty slice.
#[allow(clippy::cast_precision_loss)]
fn median_of_sorted(sorted: &[i64]) -> f64 {
    let len = sorted.len();
    if len % 2 == 1 {
        sorted[len / 2] as f64
    } else {
        let a = sorted[len / 2 - 1] as f64;
        let b = sorted[len / 2] as f64;
        f64::midpoint(a, b)
    }
}

/// Resolves `compacted_ticket_ids` against `map_id`'s tickets, keeping
/// only the ones this map actually has, and in the order the caller
/// named them.
fn compacted_tickets(
    store: &Store,
    map_id: &str,
    compacted_ticket_ids: &[String],
) -> Result<Vec<CompactedTicket>> {
    let mut out = Vec::with_capacity(compacted_ticket_ids.len());
    for ticket_id in compacted_ticket_ids {
        let found = store.connection().query_row(
            "SELECT name FROM tickets WHERE id = ?1 AND map_id = ?2",
            params![ticket_id, map_id],
            |row| row.get::<_, String>(0),
        );
        match found {
            Ok(name) => out.push(CompactedTicket {
                ticket_id: ticket_id.clone(),
                name,
            }),
            Err(rusqlite::Error::QueryReturnedNoRows) => {
                // Not a ticket this map has: the caller's flag is stale
                // or names a ticket from another map. Skip it rather
                // than fail the whole report over it.
            }
            Err(other) => {
                return Err(sql_failed(format!(
                    "cannot look up compacted ticket {ticket_id}: {other}"
                )));
            }
        }
    }
    Ok(out)
}

/// Computes [`TypeRatio`] over every ticket on `map_id`, regardless of
/// status.
fn type_ratio(store: &Store, map_id: &str) -> Result<TypeRatio> {
    let conn = store.connection();
    let mut counts = std::collections::HashMap::new();
    let mut stmt = conn
        .prepare("SELECT type, COUNT(*) FROM tickets WHERE map_id = ?1 GROUP BY type")
        .map_err(|err| sql_failed(format!("cannot prepare the type-ratio query: {err}")))?;
    let rows = stmt
        .query_map(params![map_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })
        .map_err(|err| sql_failed(format!("cannot run the type-ratio query: {err}")))?;
    for row in rows {
        let (type_str, count) =
            row.map_err(|err| sql_failed(format!("cannot read a type-ratio row: {err}")))?;
        counts.insert(type_str, count);
    }

    let research = *counts.get(TicketType::Research.as_str()).unwrap_or(&0);
    let grilling = *counts.get(TicketType::Grilling.as_str()).unwrap_or(&0);
    Ok(TypeRatio { research, grilling })
}

/// Returns every axis in `known_axes` with zero tickets on `map_id`.
fn silent_axes(store: &Store, map_id: &str, known_axes: &[String]) -> Result<Vec<String>> {
    let conn = store.connection();
    let mut stmt = conn
        .prepare(
            "SELECT EXISTS(
               SELECT 1 FROM tickets WHERE map_id = ?1 AND axis = ?2
             )",
        )
        .map_err(|err| sql_failed(format!("cannot prepare the silent-axes query: {err}")))?;

    let mut silent = Vec::new();
    for axis in known_axes {
        let used: bool = stmt
            .query_row(params![map_id, axis], |row| row.get(0))
            .map_err(|err| sql_failed(format!("cannot check axis {axis:?}: {err}")))?;
        if !used {
            silent.push(axis.clone());
        }
    }
    Ok(silent)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::journal::{
        JournalEvent, MapCreated, MapStatus, TicketCreated, TicketStatus, TicketUpdated,
    };
    use tempfile::TempDir;

    fn open_test_store() -> (TempDir, Store) {
        let tmp = TempDir::new().expect("tempdir");
        let store = Store::open(tmp.path()).expect("open store");
        (tmp, store)
    }

    fn seed_map(store: &mut Store, id: &str) {
        store
            .apply(&JournalEvent::MapCreated(MapCreated {
                id: id.to_owned(),
                name: "Map".to_owned(),
                destination: "Destination".to_owned(),
                notes: None,
                created_at: 1_700_000_000_000,
                status: MapStatus::Active,
            }))
            .unwrap();
    }

    fn create_ticket(
        store: &mut Store,
        map_id: &str,
        id: &str,
        ordinal: i64,
        ticket_type: TicketType,
        axis: Option<&str>,
    ) {
        store
            .apply(&JournalEvent::TicketCreated(TicketCreated {
                id: id.to_owned(),
                map_id: map_id.to_owned(),
                name: format!("Ticket {id}"),
                question: format!("What does {id} answer?"),
                ticket_type,
                hitl: false,
                status: TicketStatus::Open,
                created_at: 1_700_000_000_000,
                ordinal,
                axis: axis.map(str::to_owned),
                tokens_used: None,
            }))
            .unwrap();
    }

    fn resolve_ticket(store: &mut Store, id: &str, tokens_used: i64) {
        store
            .apply(&JournalEvent::TicketUpdated(TicketUpdated {
                id: id.to_owned(),
                status: Some(TicketStatus::Resolved),
                gist: Some("a gist".to_owned()),
                resolved_at: Some(1_700_000_005_000),
                tokens_used: Some(tokens_used),
                ..TicketUpdated::default()
            }))
            .unwrap();
    }

    #[test]
    fn compute_reports_map_not_found_for_an_unknown_map() {
        let (_tmp, store) = open_test_store();
        let err = compute(&store, "no-such-map", &Context::default()).unwrap_err();
        assert_eq!(err.code, ErrCode::MapNotFound);
    }

    #[test]
    fn token_distribution_covers_min_max_mean_and_median() {
        let (_tmp, mut store) = open_test_store();
        seed_map(&mut store, "M1");
        for (i, tokens) in [100, 200, 300, 400].into_iter().enumerate() {
            let id = format!("T{i}");
            create_ticket(
                &mut store,
                "M1",
                &id,
                i64::try_from(i).unwrap(),
                TicketType::Task,
                None,
            );
            resolve_ticket(&mut store, &id, tokens);
        }

        let health = compute(&store, "M1", &Context::default()).unwrap();
        assert_eq!(health.tokens_used.count, 4);
        assert_eq!(health.tokens_used.min, Some(100));
        assert_eq!(health.tokens_used.max, Some(400));
        assert_eq!(health.tokens_used.mean, Some(250.0));
        assert_eq!(health.tokens_used.median, Some(250.0));
    }

    #[test]
    fn an_unresolved_map_has_an_empty_token_distribution() {
        let (_tmp, mut store) = open_test_store();
        seed_map(&mut store, "M1");
        create_ticket(&mut store, "M1", "T0", 0, TicketType::Task, None);

        let health = compute(&store, "M1", &Context::default()).unwrap();
        assert_eq!(health.tokens_used.count, 0);
        assert_eq!(health.tokens_used.min, None);
        assert_eq!(health.tokens_used.mean, None);
    }

    #[test]
    fn compacted_tickets_keeps_only_the_ones_this_map_actually_has() {
        let (_tmp, mut store) = open_test_store();
        seed_map(&mut store, "M1");
        create_ticket(&mut store, "M1", "T0", 0, TicketType::Task, None);

        let context = Context {
            known_axes: Vec::new(),
            compacted_tickets: vec!["T0".to_owned(), "no-such-ticket".to_owned()],
        };
        let health = compute(&store, "M1", &context).unwrap();
        assert_eq!(health.compacted.len(), 1);
        assert_eq!(health.compacted[0].ticket_id, "T0");
    }

    #[test]
    fn research_to_grilling_ratio_counts_across_every_status() {
        let (_tmp, mut store) = open_test_store();
        seed_map(&mut store, "M1");
        create_ticket(&mut store, "M1", "T0", 0, TicketType::Research, None);
        create_ticket(&mut store, "M1", "T1", 1, TicketType::Research, None);
        create_ticket(&mut store, "M1", "T2", 2, TicketType::Grilling, None);
        resolve_ticket(&mut store, "T0", 100);

        let health = compute(&store, "M1", &Context::default()).unwrap();
        assert_eq!(health.research_to_grilling.research, 2);
        assert_eq!(health.research_to_grilling.grilling, 1);
        assert_eq!(health.research_to_grilling.ratio(), Some(2.0));
    }

    #[test]
    fn ratio_is_none_with_no_grilling_ticket() {
        let (_tmp, mut store) = open_test_store();
        seed_map(&mut store, "M1");
        create_ticket(&mut store, "M1", "T0", 0, TicketType::Research, None);

        let health = compute(&store, "M1", &Context::default()).unwrap();
        assert_eq!(health.research_to_grilling.ratio(), None);
    }

    #[test]
    fn silent_axes_names_every_known_axis_with_no_ticket() {
        let (_tmp, mut store) = open_test_store();
        seed_map(&mut store, "M1");
        create_ticket(&mut store, "M1", "T0", 0, TicketType::Task, Some("options"));

        let context = Context {
            known_axes: vec!["options".to_owned(), "risks".to_owned()],
            compacted_tickets: Vec::new(),
        };
        let health = compute(&store, "M1", &context).unwrap();
        assert_eq!(health.silent_axes, vec!["risks".to_owned()]);
    }

    #[test]
    fn no_known_axes_means_no_silent_axes() {
        let (_tmp, mut store) = open_test_store();
        seed_map(&mut store, "M1");

        let health = compute(&store, "M1", &Context::default()).unwrap();
        assert!(health.silent_axes.is_empty());
    }

    #[test]
    fn render_shows_all_four_sections() {
        let (_tmp, mut store) = open_test_store();
        seed_map(&mut store, "M1");
        create_ticket(&mut store, "M1", "T0", 0, TicketType::Research, None);
        resolve_ticket(&mut store, "T0", 500);

        let context = Context {
            known_axes: vec!["risks".to_owned()],
            compacted_tickets: vec!["T0".to_owned()],
        };
        let health = compute(&store, "M1", &context).unwrap();
        let text = render(&health);

        assert!(text.contains("TOKENS USED"));
        assert!(text.contains("COMPACTION"));
        assert!(text.contains("T0"));
        assert!(text.contains("RESEARCH : GRILLING"));
        assert!(text.contains("SILENT AXES"));
        assert!(text.contains("risks"));
    }
}
