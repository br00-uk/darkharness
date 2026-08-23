//! The `ticket_create` tool.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use dark_contract::{Result, Tool, ToolCtx, ToolResult, ToolSchema, tool::tier};
use rusqlite::params;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::journal::{JournalEvent, TicketCreated, TicketStatus, TicketType};

use super::{
    CartographSession, confirm_map_exists, invalid_args, journal_then_apply, new_id, now_ms,
    open_store, sql_failed,
};

#[derive(Debug, Deserialize)]
struct Args {
    map_id: String,
    name: String,
    question: String,
    #[serde(rename = "type")]
    ticket_type: String,
    #[serde(default)]
    axis: Option<String>,
}

/// Creates a ticket inside a map and returns its identifier.
///
/// The build specification's `ticket_create` signature carries no `hitl`
/// argument, and no later tool can ever set one — `TicketUpdated` (task
/// unit `D1`) has no `hitl` field, so a ticket's `hitl` flag can only ever
/// be decided here, at creation. This tool derives it from `type`, along
/// the same line task unit `E7`'s routing table draws: `prototype` and
/// `grilling` need a person; `research` and `task` do not. This is this
/// task unit's own reading of an unstated rule; see the top-level report.
#[derive(Debug)]
pub struct TicketCreate {
    session: Arc<CartographSession>,
}

impl TicketCreate {
    /// Creates the tool, sharing `session` with the other mutating
    /// ticket tools in this harness session.
    #[must_use]
    pub fn new(session: Arc<CartographSession>) -> Self {
        Self { session }
    }
}

impl Tool for TicketCreate {
    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "ticket_create".to_string(),
            description: "Creates a ticket inside a map: one decision to resolve. Returns the \
                new ticket's identifier."
                .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "map_id": {
                        "type": "string",
                        "description": "The map this ticket belongs to.",
                    },
                    "name": {
                        "type": "string",
                        "description": "The ticket's short name.",
                    },
                    "question": {
                        "type": "string",
                        "description": "The question that the ticket answers.",
                    },
                    "type": {
                        "type": "string",
                        "enum": ["research", "prototype", "grilling", "task"],
                        "description": "research: needs an answer, not a code change. \
                            prototype: needs a small throwaway implementation. grilling: needs \
                            a person to decide. task: needs ordinary implementation work.",
                    },
                    "axis": {
                        "type": "string",
                        "description": "The axis this ticket sits on, when the map has axes.",
                    },
                },
                "required": ["map_id", "name", "question", "type"],
            }),
            tier: tier::STANDARD,
            mutating: true,
        }
    }

    fn invoke<'life0, 'life1, 'async_trait>(
        &'life0 self,
        args: Value,
        ctx: &'life1 ToolCtx,
    ) -> Pin<Box<dyn Future<Output = Result<ToolResult>> + Send + 'async_trait>>
    where
        'life0: 'async_trait,
        'life1: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move {
            let args: Args =
                serde_json::from_value(args).map_err(|err| invalid_args("ticket_create", err))?;
            let ticket_type = parse_ticket_type(&args.ticket_type)?;
            let hitl = matches!(ticket_type, TicketType::Prototype | TicketType::Grilling);

            let mut store = open_store(ctx)?;
            confirm_map_exists(&store, &args.map_id)?;
            let ordinal = next_ordinal(&store, &args.map_id)?;

            let id = new_id();
            let event = JournalEvent::TicketCreated(TicketCreated {
                id: id.clone(),
                map_id: args.map_id.clone(),
                name: args.name,
                question: args.question,
                ticket_type,
                hitl,
                status: TicketStatus::Open,
                created_at: now_ms(),
                ordinal,
                axis: args.axis,
                tokens_used: None,
            });
            journal_then_apply(&mut store, &self.session.maps_root, &args.map_id, &event)?;

            Ok(ToolResult::ok(id))
        })
    }
}

/// Parses the `type` argument into a [`TicketType`].
fn parse_ticket_type(value: &str) -> Result<TicketType> {
    match value {
        "research" => Ok(TicketType::Research),
        "prototype" => Ok(TicketType::Prototype),
        "grilling" => Ok(TicketType::Grilling),
        "task" => Ok(TicketType::Task),
        other => Err(invalid_args(
            "ticket_create",
            format!("type must be research, prototype, grilling, or task, got {other:?}"),
        )),
    }
}

/// Returns the next `ordinal` for a new ticket on `map_id`: one past the
/// highest ordinal any existing ticket on this map holds, or `0` for the
/// first ticket.
fn next_ordinal(store: &crate::store::Store, map_id: &str) -> Result<i64> {
    store
        .connection()
        .query_row(
            "SELECT COALESCE(MAX(ordinal), -1) + 1 FROM tickets WHERE map_id = ?1",
            params![map_id],
            |row| row.get(0),
        )
        .map_err(|err| {
            sql_failed(format!(
                "cannot compute the next ordinal for {map_id}: {err}"
            ))
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::journal::{JournalEvent, MapCreated, MapStatus};
    use crate::store::Store;
    use tempfile::TempDir;

    // `tokio_util` is not a declared dependency of this crate (Rule 16), so
    // `CancellationToken` cannot be named here — only `ToolCtx.cancel`'s
    // already-resolved field type names it. `Default::default()` is the one
    // way to build one without naming the type.
    #[allow(clippy::default_trait_access)]
    fn ctx(root: &std::path::Path) -> ToolCtx {
        let bus = dark_contract::EventBus::new();
        ToolCtx {
            root: root.to_path_buf(),
            events: bus.tx(),
            cancel: Default::default(),
            dark: true,
            human_present: false,
        }
    }

    fn seed_map(store: &mut Store, id: &str) {
        store
            .apply(&JournalEvent::MapCreated(MapCreated {
                id: id.to_owned(),
                name: "Map".to_owned(),
                destination: "Destination".to_owned(),
                notes: None,
                created_at: 1_700_000_000_000,
                status: MapStatus::Charting,
            }))
            .unwrap();
    }

    #[tokio::test]
    async fn creates_a_research_ticket_with_hitl_false() {
        let dir = TempDir::new().unwrap();
        let mut store = Store::open(dir.path()).unwrap();
        seed_map(&mut store, "M1");
        drop(store);
        let session = Arc::new(CartographSession::new(dir.path().join("maps"), "s"));
        let tool = TicketCreate::new(session);

        let result = tool
            .invoke(
                json!({"map_id": "M1", "name": "T", "question": "Q?", "type": "research"}),
                &ctx(dir.path()),
            )
            .await
            .unwrap();
        let ticket_id = result.content;

        let store = Store::open(dir.path()).unwrap();
        let hitl: i64 = store
            .connection()
            .query_row(
                "SELECT hitl FROM tickets WHERE id = ?1",
                rusqlite::params![ticket_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(hitl, 0);
    }

    #[tokio::test]
    async fn a_grilling_ticket_gets_hitl_true() {
        let dir = TempDir::new().unwrap();
        let mut store = Store::open(dir.path()).unwrap();
        seed_map(&mut store, "M1");
        drop(store);
        let session = Arc::new(CartographSession::new(dir.path().join("maps"), "s"));
        let tool = TicketCreate::new(session);

        let result = tool
            .invoke(
                json!({"map_id": "M1", "name": "T", "question": "Q?", "type": "grilling"}),
                &ctx(dir.path()),
            )
            .await
            .unwrap();
        let ticket_id = result.content;

        let store = Store::open(dir.path()).unwrap();
        let hitl: i64 = store
            .connection()
            .query_row(
                "SELECT hitl FROM tickets WHERE id = ?1",
                rusqlite::params![ticket_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(hitl, 1);
    }

    #[tokio::test]
    async fn ordinals_increase_across_calls_on_the_same_map() {
        let dir = TempDir::new().unwrap();
        let mut store = Store::open(dir.path()).unwrap();
        seed_map(&mut store, "M1");
        drop(store);
        let session = Arc::new(CartographSession::new(dir.path().join("maps"), "s"));
        let tool = TicketCreate::new(session);

        for _ in 0..3 {
            tool.invoke(
                json!({"map_id": "M1", "name": "T", "question": "Q?", "type": "task"}),
                &ctx(dir.path()),
            )
            .await
            .unwrap();
        }

        let store = Store::open(dir.path()).unwrap();
        let mut stmt = store
            .connection()
            .prepare("SELECT ordinal FROM tickets WHERE map_id = 'M1' ORDER BY ordinal")
            .unwrap();
        let ordinals: Vec<i64> = stmt
            .query_map(rusqlite::params![], |row| row.get(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        assert_eq!(ordinals, vec![0, 1, 2]);
    }

    #[tokio::test]
    async fn an_unknown_map_is_map_not_found() {
        let dir = TempDir::new().unwrap();
        Store::open(dir.path()).unwrap();
        let session = Arc::new(CartographSession::new(dir.path().join("maps"), "s"));
        let tool = TicketCreate::new(session);

        let err = tool
            .invoke(
                json!({"map_id": "no-such-map", "name": "T", "question": "Q?", "type": "task"}),
                &ctx(dir.path()),
            )
            .await
            .unwrap_err();
        assert_eq!(err.code, dark_contract::ErrCode::MapNotFound);
    }

    #[tokio::test]
    async fn an_invalid_type_is_invalid_args() {
        let dir = TempDir::new().unwrap();
        let mut store = Store::open(dir.path()).unwrap();
        seed_map(&mut store, "M1");
        drop(store);
        let session = Arc::new(CartographSession::new(dir.path().join("maps"), "s"));
        let tool = TicketCreate::new(session);

        let err = tool
            .invoke(
                json!({"map_id": "M1", "name": "T", "question": "Q?", "type": "bogus"}),
                &ctx(dir.path()),
            )
            .await
            .unwrap_err();
        assert_eq!(err.code, dark_contract::ErrCode::ToolInvalidArgs);
    }
}
