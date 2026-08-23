//! The `scope_exclude` tool.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use dark_contract::{ErrCode, Error, Result, Tool, ToolCtx, ToolResult, ToolSchema, tool::tier};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::journal::{JournalEvent, ScopeExclusionAdded, TicketStatus, TicketUpdated};

use super::{
    CartographSession, confirm_map_exists, invalid_args, journal_then_apply, load_ticket, new_id,
    now_ms, open_store,
};

#[derive(Debug, Deserialize)]
struct Args {
    map_id: String,
    gist: String,
    reason: String,
    #[serde(default)]
    ticket_id: Option<String>,
}

/// Excludes something from a map's scope. Returns the new scope
/// exclusion's identifier.
///
/// When `ticket_id` names a ticket, this tool also moves that ticket to
/// [`TicketStatus::OutOfScope`], in the same journal-then-apply step, so
/// it stops blocking anything on the frontier — see the frontier query's
/// `status NOT IN ('resolved', 'out_of_scope')` clause in
/// `crate::frontier`. Task unit `E7` step 8 says a scope boundary "is not
/// a step on the route" and must not be resolved; `out_of_scope` is the
/// status that keeps it out of the digest's decisions section while
/// still clearing it from the graph.
#[derive(Debug)]
pub struct ScopeExclude {
    session: Arc<CartographSession>,
}

impl ScopeExclude {
    /// Creates the tool, sharing `session` with the other mutating
    /// ticket tools in this harness session.
    #[must_use]
    pub fn new(session: Arc<CartographSession>) -> Self {
        Self { session }
    }
}

impl Tool for ScopeExclude {
    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "scope_exclude".to_string(),
            description: "Excludes something from a map's scope. When ticket_id names a \
                ticket, that ticket moves out of scope instead of being resolved."
                .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "map_id": {
                        "type": "string",
                        "description": "The map that excludes this.",
                    },
                    "gist": {
                        "type": "string",
                        "description": "A short summary of the excluded thing.",
                    },
                    "reason": {
                        "type": "string",
                        "description": "Why the map excludes it.",
                    },
                    "ticket_id": {
                        "type": "string",
                        "description": "The ticket that raised this exclusion, when one did. \
                            That ticket moves to the out-of-scope status.",
                    },
                },
                "required": ["map_id", "gist", "reason"],
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
                serde_json::from_value(args).map_err(|err| invalid_args("scope_exclude", err))?;

            let mut store = open_store(ctx)?;
            confirm_map_exists(&store, &args.map_id)?;
            if let Some(ticket_id) = &args.ticket_id {
                let ticket = load_ticket(&store, ticket_id)?;
                if ticket.map_id != args.map_id {
                    return Err(Error::new(
                        ErrCode::ToolInvalidArgs,
                        format!("ticket {ticket_id} does not belong to map {}", args.map_id),
                    ));
                }
            }

            let id = new_id();
            let event = JournalEvent::ScopeExclusionAdded(ScopeExclusionAdded {
                id: id.clone(),
                map_id: args.map_id.clone(),
                gist: args.gist,
                reason: args.reason,
                ticket_id: args.ticket_id.clone(),
            });
            journal_then_apply(&mut store, &self.session.maps_root, &args.map_id, &event)?;

            if let Some(ticket_id) = &args.ticket_id {
                let update = JournalEvent::TicketUpdated(TicketUpdated {
                    id: ticket_id.clone(),
                    status: Some(TicketStatus::OutOfScope),
                    resolved_at: Some(now_ms()),
                    ..TicketUpdated::default()
                });
                journal_then_apply(&mut store, &self.session.maps_root, &args.map_id, &update)?;
            }

            Ok(ToolResult::ok(id))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::journal::{MapCreated, MapStatus, TicketCreated, TicketType};
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
                status: MapStatus::Active,
            }))
            .unwrap();
    }

    fn seed_ticket(store: &mut Store, map_id: &str, id: &str) {
        store
            .apply(&JournalEvent::TicketCreated(TicketCreated {
                id: id.to_owned(),
                map_id: map_id.to_owned(),
                name: id.to_owned(),
                question: "Q?".to_owned(),
                ticket_type: TicketType::Task,
                hitl: false,
                status: TicketStatus::Open,
                created_at: 1_700_000_000_000,
                ordinal: 0,
                axis: None,
                tokens_used: None,
            }))
            .unwrap();
    }

    #[tokio::test]
    async fn excludes_something_with_no_ticket() {
        let dir = TempDir::new().unwrap();
        let mut store = Store::open(dir.path()).unwrap();
        seed_map(&mut store, "M1");
        drop(store);
        let session = Arc::new(CartographSession::new(dir.path().join("maps"), "s"));
        let tool = ScopeExclude::new(session);

        let result = tool
            .invoke(
                json!({"map_id": "M1", "gist": "Pack signing", "reason": "separate effort"}),
                &ctx(dir.path()),
            )
            .await
            .unwrap();
        assert!(!result.is_error);

        let store = Store::open(dir.path()).unwrap();
        let count: i64 = store
            .connection()
            .query_row("SELECT count(*) FROM scope_exclusions", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn excluding_a_ticket_moves_it_out_of_scope() {
        let dir = TempDir::new().unwrap();
        let mut store = Store::open(dir.path()).unwrap();
        seed_map(&mut store, "M1");
        seed_ticket(&mut store, "M1", "T1");
        drop(store);
        let session = Arc::new(CartographSession::new(dir.path().join("maps"), "s"));
        let tool = ScopeExclude::new(session);

        tool.invoke(
            json!({
                "map_id": "M1",
                "gist": "Past the destination",
                "reason": "out of scope",
                "ticket_id": "T1",
            }),
            &ctx(dir.path()),
        )
        .await
        .unwrap();

        let store = Store::open(dir.path()).unwrap();
        let status: String = store
            .connection()
            .query_row("SELECT status FROM tickets WHERE id = 'T1'", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(status, "out_of_scope");
    }

    #[tokio::test]
    async fn an_out_of_scope_ticket_does_not_appear_as_a_decision() {
        let dir = TempDir::new().unwrap();
        let mut store = Store::open(dir.path()).unwrap();
        seed_map(&mut store, "M1");
        seed_ticket(&mut store, "M1", "T1");
        drop(store);
        let session = Arc::new(CartographSession::new(dir.path().join("maps"), "s"));
        ScopeExclude::new(session)
            .invoke(
                json!({
                    "map_id": "M1",
                    "gist": "Past the destination",
                    "reason": "out of scope",
                    "ticket_id": "T1",
                }),
                &ctx(dir.path()),
            )
            .await
            .unwrap();

        let store = Store::open(dir.path()).unwrap();
        let text = crate::digest::render(&store, "M1", crate::digest::Tier::Full)
            .unwrap()
            .unwrap();
        assert!(!text.contains("DECISIONS SO FAR"));
        assert!(text.contains("OUT OF SCOPE"));
    }

    #[tokio::test]
    async fn a_ticket_from_a_different_map_is_refused() {
        let dir = TempDir::new().unwrap();
        let mut store = Store::open(dir.path()).unwrap();
        seed_map(&mut store, "M1");
        seed_map(&mut store, "M2");
        seed_ticket(&mut store, "M2", "T1");
        drop(store);
        let session = Arc::new(CartographSession::new(dir.path().join("maps"), "s"));
        let tool = ScopeExclude::new(session);

        let err = tool
            .invoke(
                json!({
                    "map_id": "M1",
                    "gist": "g",
                    "reason": "r",
                    "ticket_id": "T1",
                }),
                &ctx(dir.path()),
            )
            .await
            .unwrap_err();
        assert_eq!(err.code, dark_contract::ErrCode::ToolInvalidArgs);
    }

    #[tokio::test]
    async fn an_unknown_map_is_map_not_found() {
        let dir = TempDir::new().unwrap();
        Store::open(dir.path()).unwrap();
        let session = Arc::new(CartographSession::new(dir.path().join("maps"), "s"));
        let tool = ScopeExclude::new(session);

        let err = tool
            .invoke(
                json!({"map_id": "no-such-map", "gist": "g", "reason": "r"}),
                &ctx(dir.path()),
            )
            .await
            .unwrap_err();
        assert_eq!(err.code, dark_contract::ErrCode::MapNotFound);
    }
}
