//! The `ticket_block` tool.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use dark_contract::{ErrCode, Error, Result, Tool, ToolCtx, ToolResult, ToolSchema, tool::tier};
use serde::Deserialize;
use serde_json::{Value, json};

use super::{CartographSession, invalid_args, load_ticket, open_store};

#[derive(Debug, Deserialize)]
struct Args {
    blocker: String,
    blocked: String,
}

/// Adds a blocking edge: `blocked` cannot join the frontier until
/// `blocker` resolves or leaves scope.
///
/// Delegates to [`crate::store::Store::add_edge`] (task unit `D1`), which
/// rejects an edge that would close a cycle. This tool has no cheaper way
/// to preview that check: the cycle detector
/// [`crate::store::Store::add_edge`] calls is private to the `store`
/// module, so `ticket_block` cannot run it without also running the
/// insert. [`dark_contract::Tool::preview`]'s default (`Ok(None)`) is
/// correct here.
#[derive(Debug)]
pub struct TicketBlock {
    session: Arc<CartographSession>,
}

impl TicketBlock {
    /// Creates the tool, sharing `session` with the other mutating
    /// ticket tools in this harness session.
    #[must_use]
    pub fn new(session: Arc<CartographSession>) -> Self {
        Self { session }
    }
}

impl Tool for TicketBlock {
    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "ticket_block".to_string(),
            description: "Adds a blocking edge between two tickets in the same map: blocked \
                cannot join the frontier until blocker resolves or leaves scope. Refuses an \
                edge that would close a cycle."
                .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "blocker": {
                        "type": "string",
                        "description": "The ticket that must resolve first.",
                    },
                    "blocked": {
                        "type": "string",
                        "description": "The ticket that waits on blocker.",
                    },
                },
                "required": ["blocker", "blocked"],
            }),
            tier: tier::STANDARD,
            mutating: true,
        }
    }

    // `blocker` and `blocked` are the tool's own argument names (they
    // match the PRD signature and the schema's column names): renaming
    // one to satisfy `similar_names` would make the code harder to
    // match against the tool it implements, not easier to read. See
    // `crate::store::Store::add_edge` for the same call.
    #[allow(clippy::similar_names)]
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
                serde_json::from_value(args).map_err(|err| invalid_args("ticket_block", err))?;

            let mut store = open_store(ctx)?;
            let blocker = load_ticket(&store, &args.blocker)?;
            let blocked = load_ticket(&store, &args.blocked)?;
            if blocker.map_id != blocked.map_id {
                return Err(Error::new(
                    ErrCode::ToolInvalidArgs,
                    format!(
                        "blocker {} and blocked {} belong to different maps",
                        args.blocker, args.blocked
                    ),
                ));
            }

            store.add_edge(
                &self.session.maps_root,
                &blocker.map_id,
                &args.blocker,
                &args.blocked,
            )?;

            Ok(ToolResult::ok(format!(
                "{} now blocks {}",
                args.blocker, args.blocked
            )))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::journal::{
        JournalEvent, MapCreated, MapStatus, TicketCreated, TicketStatus, TicketType,
    };
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

    fn seed_ticket(store: &mut Store, map_id: &str, id: &str, ordinal: i64) {
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
                ordinal,
                axis: None,
                tokens_used: None,
            }))
            .unwrap();
    }

    #[tokio::test]
    async fn adds_an_edge() {
        let dir = TempDir::new().unwrap();
        let mut store = Store::open(dir.path()).unwrap();
        store
            .apply(&JournalEvent::MapCreated(MapCreated {
                id: "M1".to_owned(),
                name: "Map".to_owned(),
                destination: "Destination".to_owned(),
                notes: None,
                created_at: 1_700_000_000_000,
                status: MapStatus::Active,
            }))
            .unwrap();
        seed_ticket(&mut store, "M1", "T1", 0);
        seed_ticket(&mut store, "M1", "T2", 1);
        drop(store);
        let session = Arc::new(CartographSession::new(dir.path().join("maps"), "s"));
        let tool = TicketBlock::new(session);

        let result = tool
            .invoke(json!({"blocker": "T1", "blocked": "T2"}), &ctx(dir.path()))
            .await
            .unwrap();
        assert!(!result.is_error);

        let store = Store::open(dir.path()).unwrap();
        let count: i64 = store
            .connection()
            .query_row("SELECT count(*) FROM edges", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn a_cycle_is_refused() {
        let dir = TempDir::new().unwrap();
        let mut store = Store::open(dir.path()).unwrap();
        store
            .apply(&JournalEvent::MapCreated(MapCreated {
                id: "M1".to_owned(),
                name: "Map".to_owned(),
                destination: "Destination".to_owned(),
                notes: None,
                created_at: 1_700_000_000_000,
                status: MapStatus::Active,
            }))
            .unwrap();
        seed_ticket(&mut store, "M1", "T1", 0);
        seed_ticket(&mut store, "M1", "T2", 1);
        drop(store);
        let session = Arc::new(CartographSession::new(dir.path().join("maps"), "s"));

        TicketBlock::new(session.clone())
            .invoke(json!({"blocker": "T1", "blocked": "T2"}), &ctx(dir.path()))
            .await
            .unwrap();
        let err = TicketBlock::new(session)
            .invoke(json!({"blocker": "T2", "blocked": "T1"}), &ctx(dir.path()))
            .await
            .unwrap_err();
        assert_eq!(err.code, dark_contract::ErrCode::MapCycle);
    }

    #[tokio::test]
    async fn tickets_from_different_maps_are_refused() {
        let dir = TempDir::new().unwrap();
        let mut store = Store::open(dir.path()).unwrap();
        for id in ["M1", "M2"] {
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
        seed_ticket(&mut store, "M1", "T1", 0);
        seed_ticket(&mut store, "M2", "T2", 0);
        drop(store);
        let session = Arc::new(CartographSession::new(dir.path().join("maps"), "s"));
        let tool = TicketBlock::new(session);

        let err = tool
            .invoke(json!({"blocker": "T1", "blocked": "T2"}), &ctx(dir.path()))
            .await
            .unwrap_err();
        assert_eq!(err.code, dark_contract::ErrCode::ToolInvalidArgs);
    }

    #[tokio::test]
    async fn an_unknown_ticket_is_map_not_found() {
        let dir = TempDir::new().unwrap();
        Store::open(dir.path()).unwrap();
        let session = Arc::new(CartographSession::new(dir.path().join("maps"), "s"));
        let tool = TicketBlock::new(session);

        let err = tool
            .invoke(
                json!({"blocker": "nope", "blocked": "also-nope"}),
                &ctx(dir.path()),
            )
            .await
            .unwrap_err();
        assert_eq!(err.code, dark_contract::ErrCode::MapNotFound);
    }
}
