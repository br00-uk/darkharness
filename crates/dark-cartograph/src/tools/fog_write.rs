//! The `fog_write` tool.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use dark_contract::{Result, Tool, ToolCtx, ToolResult, ToolSchema, tool::tier};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::journal::{FogAdded, JournalEvent};

use super::{
    CartographSession, confirm_map_exists, invalid_args, journal_then_apply, new_id, now_ms,
    open_store,
};

#[derive(Debug, Deserialize)]
struct Args {
    map_id: String,
    patch: String,
    #[serde(default)]
    axis: Option<String>,
}

/// Records a patch of fog: a question the map cannot yet state precisely
/// enough to become a ticket. Returns the new fog patch's identifier.
#[derive(Debug)]
pub struct FogWrite {
    session: Arc<CartographSession>,
}

impl FogWrite {
    /// Creates the tool, sharing `session` with the other mutating
    /// ticket tools in this harness session.
    #[must_use]
    pub fn new(session: Arc<CartographSession>) -> Self {
        Self { session }
    }
}

impl Tool for FogWrite {
    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "fog_write".to_string(),
            description: "Records a patch of fog: a question the map cannot yet state \
                precisely enough to become a ticket. Returns the new fog patch's identifier."
                .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "map_id": {
                        "type": "string",
                        "description": "The map this fog patch belongs to.",
                    },
                    "patch": {
                        "type": "string",
                        "description": "The text of the unanswered question.",
                    },
                    "axis": {
                        "type": "string",
                        "description": "The axis this fog patch sits on, when the map has \
                            axes.",
                    },
                },
                "required": ["map_id", "patch"],
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
                serde_json::from_value(args).map_err(|err| invalid_args("fog_write", err))?;

            let mut store = open_store(ctx)?;
            confirm_map_exists(&store, &args.map_id)?;

            let id = new_id();
            let event = JournalEvent::FogAdded(FogAdded {
                id: id.clone(),
                map_id: args.map_id.clone(),
                patch: args.patch,
                axis: args.axis,
                created_at: now_ms(),
            });
            journal_then_apply(&mut store, &self.session.maps_root, &args.map_id, &event)?;

            Ok(ToolResult::ok(id))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::journal::{MapCreated, MapStatus};
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

    fn seed_map(store: &mut Store) {
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
    }

    #[tokio::test]
    async fn writes_a_fog_patch() {
        let dir = TempDir::new().unwrap();
        let mut store = Store::open(dir.path()).unwrap();
        seed_map(&mut store);
        drop(store);
        let session = Arc::new(CartographSession::new(dir.path().join("maps"), "s"));
        let tool = FogWrite::new(session);

        let result = tool
            .invoke(
                json!({"map_id": "M1", "patch": "How packs are distributed."}),
                &ctx(dir.path()),
            )
            .await
            .unwrap();
        assert!(!result.is_error);
        let fog_id = result.content;

        let store = Store::open(dir.path()).unwrap();
        let patch: String = store
            .connection()
            .query_row(
                "SELECT patch FROM fog WHERE id = ?1",
                rusqlite::params![fog_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(patch, "How packs are distributed.");
    }

    #[tokio::test]
    async fn an_unknown_map_is_map_not_found() {
        let dir = TempDir::new().unwrap();
        Store::open(dir.path()).unwrap();
        let session = Arc::new(CartographSession::new(dir.path().join("maps"), "s"));
        let tool = FogWrite::new(session);

        let err = tool
            .invoke(
                json!({"map_id": "no-such-map", "patch": "?"}),
                &ctx(dir.path()),
            )
            .await
            .unwrap_err();
        assert_eq!(err.code, dark_contract::ErrCode::MapNotFound);
    }
}
