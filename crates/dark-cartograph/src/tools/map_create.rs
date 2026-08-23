//! The `map_create` tool.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use dark_contract::{Result, Tool, ToolCtx, ToolResult, ToolSchema, tool::tier};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::journal::{JournalEvent, MapCreated, MapStatus};

use super::{CartographSession, invalid_args, journal_then_apply, new_id, now_ms, open_store};

#[derive(Debug, Deserialize)]
struct Args {
    name: String,
    destination: String,
    #[serde(default)]
    notes: Option<String>,
}

/// Creates a map and returns its identifier.
///
/// A new map starts in [`MapStatus::Charting`]: charting has not finished
/// until the tickets that answer `destination` exist, which is exactly
/// the work task units `E1` to `E6` do after this tool returns.
#[derive(Debug)]
pub struct MapCreate {
    session: Arc<CartographSession>,
}

impl MapCreate {
    /// Creates the tool, sharing `session` with the other mutating
    /// ticket tools in this harness session.
    #[must_use]
    pub fn new(session: Arc<CartographSession>) -> Self {
        Self { session }
    }
}

impl Tool for MapCreate {
    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "map_create".to_string(),
            description: "Creates a map: a wayfinder map that will hold tickets, fog, and \
                scope exclusions. Returns the new map's identifier. The map starts in the \
                charting status."
                .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "The map's short name.",
                    },
                    "destination": {
                        "type": "string",
                        "description": "What the map is charting a way towards.",
                    },
                    "notes": {
                        "type": "string",
                        "description": "Free-text notes about the map, for example the domain \
                            or a pointer to prior art.",
                    },
                },
                "required": ["name", "destination"],
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
                serde_json::from_value(args).map_err(|err| invalid_args("map_create", err))?;

            let mut store = open_store(ctx)?;
            let id = new_id();
            let event = JournalEvent::MapCreated(MapCreated {
                id: id.clone(),
                name: args.name,
                destination: args.destination,
                notes: args.notes,
                created_at: now_ms(),
                status: MapStatus::Charting,
            });
            journal_then_apply(&mut store, &self.session.maps_root, &id, &event)?;

            Ok(ToolResult::ok(id))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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

    #[tokio::test]
    async fn creates_a_map_in_the_charting_status() {
        let dir = TempDir::new().unwrap();
        let maps_root = dir.path().join("maps");
        let session = Arc::new(CartographSession::new(maps_root.clone(), "session-a"));
        let tool = MapCreate::new(session);

        let result = tool
            .invoke(
                json!({"name": "Offline pack format", "destination": "A frozen pack format"}),
                &ctx(dir.path()),
            )
            .await
            .unwrap();
        assert!(!result.is_error);
        let map_id = result.content;

        let store = Store::open(dir.path()).unwrap();
        let (name, status): (String, String) = store
            .connection()
            .query_row(
                "SELECT name, status FROM maps WHERE id = ?1",
                rusqlite::params![map_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(name, "Offline pack format");
        assert_eq!(status, "charting");

        let events = crate::journal::read_events(&maps_root, &map_id).unwrap();
        assert_eq!(events.len(), 1);
    }

    #[tokio::test]
    async fn notes_are_optional() {
        let dir = TempDir::new().unwrap();
        let session = Arc::new(CartographSession::new(dir.path().join("maps"), "session-a"));
        let tool = MapCreate::new(session);

        let result = tool
            .invoke(json!({"name": "M", "destination": "D"}), &ctx(dir.path()))
            .await
            .unwrap();
        assert!(!result.is_error);
    }

    #[tokio::test]
    async fn missing_required_fields_are_invalid_args() {
        let dir = TempDir::new().unwrap();
        let session = Arc::new(CartographSession::new(dir.path().join("maps"), "session-a"));
        let tool = MapCreate::new(session);

        let err = tool
            .invoke(json!({"name": "M"}), &ctx(dir.path()))
            .await
            .unwrap_err();
        assert_eq!(err.code, dark_contract::ErrCode::ToolInvalidArgs);
    }
}
