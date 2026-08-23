//! The `map_read` tool.

use std::future::Future;
use std::pin::Pin;

use dark_contract::{ErrCode, Error, Result, Tool, ToolCtx, ToolResult, ToolSchema, tool::tier};
use rusqlite::params;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::digest::{self, Tier};

use super::{invalid_args, not_found, open_store, sql_failed};

#[derive(Debug, Deserialize)]
struct Args {
    #[serde(default)]
    map_id: Option<String>,
    tier: String,
}

/// Returns the map digest.
///
/// Reads `map_id`, when the caller names one, or falls back to the most
/// recently updated map. `ToolCtx` carries no notion of "the map this
/// session has loaded" — `dark-core` task unit `A3` is where that concept
/// eventually lives — so this fallback is what lets a model call
/// `map_read` before any such wiring exists.
#[derive(Debug, Default)]
pub struct MapRead;

impl MapRead {
    /// Creates the tool. `map_read` needs no session-scoped state.
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

impl Tool for MapRead {
    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "map_read".to_string(),
            description: "Returns the map digest: the destination, the decisions made so far, \
                the frontier, blocked tickets, fog, and scope exclusions. Omit map_id to read \
                the most recently updated map."
                .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "map_id": {
                        "type": "string",
                        "description": "The map to read. Omit to read the most recently \
                            updated map.",
                    },
                    "tier": {
                        "type": "string",
                        "enum": ["full", "frontier_only", "none"],
                        "description": "full: everything. frontier_only: destination, notes, \
                            and the frontier only. none: nothing, at no query cost.",
                    },
                },
                "required": ["tier"],
            }),
            tier: tier::ESSENTIAL,
            mutating: false,
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
                serde_json::from_value(args).map_err(|err| invalid_args("map_read", err))?;
            let requested_tier = parse_tier(&args.tier)?;

            // `Tier::None` never touches the database (see
            // `digest::render`), so it needs no map at all, named or
            // resolved.
            if matches!(requested_tier, Tier::None) {
                return Ok(ToolResult::ok(String::new()));
            }

            let store = open_store(ctx)?;
            let map_id = match args.map_id {
                Some(id) => id,
                None => most_recently_updated_map(&store)?,
            };

            let text = digest::render(&store, &map_id, requested_tier)?.unwrap_or_default();
            Ok(ToolResult::ok(text))
        })
    }
}

/// Parses `tier` into a [`Tier`]. `Tier` has no `serde::Deserialize` of
/// its own — it belongs to task unit `D3`, and adding one there would
/// touch a file this task unit does not own — so this tool parses the
/// three allowed strings directly.
fn parse_tier(tier: &str) -> Result<Tier> {
    match tier {
        "full" => Ok(Tier::Full),
        "frontier_only" => Ok(Tier::FrontierOnly),
        "none" => Ok(Tier::None),
        other => Err(Error::new(
            ErrCode::ToolInvalidArgs,
            format!("map_read: tier must be full, frontier_only, or none, got {other:?}"),
        )),
    }
}

/// Returns the identifier of the map with the greatest `updated_at`.
///
/// # Errors
///
/// Returns [`ErrCode::MapNotFound`] when no map exists yet.
fn most_recently_updated_map(store: &crate::store::Store) -> Result<String> {
    store
        .connection()
        .query_row(
            "SELECT id FROM maps ORDER BY updated_at DESC, id DESC LIMIT 1",
            params![],
            |row| row.get(0),
        )
        .map_err(|err| match err {
            rusqlite::Error::QueryReturnedNoRows => not_found("map", "(none created yet)"),
            other => sql_failed(format!(
                "cannot find the most recently updated map: {other}"
            )),
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

    fn seed_map(store: &mut Store, id: &str, updated_at: i64) {
        store
            .apply(&JournalEvent::MapCreated(MapCreated {
                id: id.to_owned(),
                name: format!("Map {id}"),
                destination: "A tested destination".to_owned(),
                notes: None,
                created_at: updated_at,
                status: MapStatus::Active,
            }))
            .unwrap();
    }

    #[tokio::test]
    async fn tier_none_needs_no_map_id_and_touches_no_database() {
        let dir = TempDir::new().unwrap();
        let tool = MapRead::new();

        // No `.dark/cartograph.db` exists yet, and no map_id is given;
        // `tier: none` must still succeed.
        let result = tool
            .invoke(json!({"tier": "none"}), &ctx(dir.path()))
            .await
            .unwrap();
        assert_eq!(result.content, "");
    }

    #[tokio::test]
    async fn tier_full_reads_the_named_map() {
        let dir = TempDir::new().unwrap();
        let mut store = Store::open(dir.path()).unwrap();
        seed_map(&mut store, "M1", 1_700_000_000_000);
        drop(store);

        let tool = MapRead::new();
        let result = tool
            .invoke(json!({"map_id": "M1", "tier": "full"}), &ctx(dir.path()))
            .await
            .unwrap();
        assert!(result.content.starts_with("MAP: Map M1"));
    }

    #[tokio::test]
    async fn an_unknown_map_id_is_map_not_found() {
        let dir = TempDir::new().unwrap();
        Store::open(dir.path()).unwrap();
        let tool = MapRead::new();

        let err = tool
            .invoke(
                json!({"map_id": "no-such-map", "tier": "full"}),
                &ctx(dir.path()),
            )
            .await
            .unwrap_err();
        assert_eq!(err.code, ErrCode::MapNotFound);
    }

    #[tokio::test]
    async fn omitting_map_id_reads_the_most_recently_updated_map() {
        let dir = TempDir::new().unwrap();
        let mut store = Store::open(dir.path()).unwrap();
        seed_map(&mut store, "M1", 1_700_000_000_000);
        seed_map(&mut store, "M2", 1_700_000_005_000);
        drop(store);

        let tool = MapRead::new();
        let result = tool
            .invoke(json!({"tier": "frontier_only"}), &ctx(dir.path()))
            .await
            .unwrap();
        assert!(result.content.contains("A tested destination"));
        // Both maps share a destination string; the identity check that
        // matters is which map's name the digest would show, but
        // FrontierOnly omits the header. Assert through Full instead.
        let result_full = tool
            .invoke(json!({"tier": "full"}), &ctx(dir.path()))
            .await
            .unwrap();
        assert!(result_full.content.starts_with("MAP: Map M2"));
    }

    #[tokio::test]
    async fn an_invalid_tier_string_is_invalid_args() {
        let dir = TempDir::new().unwrap();
        let tool = MapRead::new();

        let err = tool
            .invoke(json!({"tier": "everything"}), &ctx(dir.path()))
            .await
            .unwrap_err();
        assert_eq!(err.code, ErrCode::ToolInvalidArgs);
    }
}
