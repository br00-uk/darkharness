//! The `ticket_zoom` tool.

use std::fmt::Write as _;
use std::future::Future;
use std::pin::Pin;

use dark_contract::{Result, Tool, ToolCtx, ToolResult, ToolSchema, tool::tier};
use rusqlite::params;
use serde::Deserialize;
use serde_json::{Value, json};

use super::{TicketRow, invalid_args, load_ticket, open_store, sql_failed};

#[derive(Debug, Deserialize)]
struct Args {
    ticket_id: String,
}

/// Returns one ticket's body, resolution, and assets.
///
/// This is the detail `map_read`'s digest deliberately leaves out: the
/// digest shows a ticket's name and, once resolved, its gist; `zoom`
/// shows the full question, the full resolution text, and every asset
/// the ticket produced.
#[derive(Debug, Default)]
pub struct TicketZoom;

impl TicketZoom {
    /// Creates the tool. `ticket_zoom` needs no session-scoped state.
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

impl Tool for TicketZoom {
    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "ticket_zoom".to_string(),
            description: "Returns one ticket's full question, its resolution (once resolved), \
                and the assets it produced. Call this only when the map digest is not enough."
                .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "ticket_id": {
                        "type": "string",
                        "description": "The ticket to zoom into.",
                    },
                },
                "required": ["ticket_id"],
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
                serde_json::from_value(args).map_err(|err| invalid_args("ticket_zoom", err))?;

            let store = open_store(ctx)?;
            let ticket = load_ticket(&store, &args.ticket_id)?;
            let assets = load_assets(&store, &args.ticket_id)?;

            Ok(ToolResult::ok(render(&args.ticket_id, &ticket, &assets)))
        })
    }
}

/// One asset a ticket produced, as `ticket_zoom` shows it.
struct AssetLine {
    /// The asset's kind, when it has one.
    kind: Option<String>,
    /// The asset's path, when it lives on disk.
    path: Option<String>,
    /// A free-text note about the asset, when it has one.
    note: Option<String>,
}

/// Reads every asset `ticket_id` produced, in insertion order.
fn load_assets(store: &crate::store::Store, ticket_id: &str) -> Result<Vec<AssetLine>> {
    let conn = store.connection();
    let mut stmt = conn
        .prepare("SELECT kind, path, note FROM assets WHERE ticket_id = ?1 ORDER BY id")
        .map_err(|err| sql_failed(format!("cannot prepare the assets query: {err}")))?;
    let rows = stmt
        .query_map(params![ticket_id], |row| {
            Ok(AssetLine {
                kind: row.get(0)?,
                path: row.get(1)?,
                note: row.get(2)?,
            })
        })
        .map_err(|err| sql_failed(format!("cannot run the assets query: {err}")))?;
    rows.collect::<rusqlite::Result<_>>()
        .map_err(|err| sql_failed(format!("cannot read an asset row: {err}")))
}

/// Renders `ticket_zoom`'s output.
fn render(ticket_id: &str, ticket: &TicketRow, assets: &[AssetLine]) -> String {
    let presence = if ticket.hitl { "HITL" } else { "AFK" };
    let mut out = format!(
        "{ticket_id} {}\nTYPE: {} · STATUS: {} · {presence}\n",
        ticket.name,
        ticket.ticket_type.as_str(),
        ticket.status.as_str(),
    );
    if let Some(axis) = &ticket.axis {
        let _ = writeln!(out, "AXIS: {axis}");
    }

    let _ = write!(out, "\nQUESTION\n  {}\n", ticket.question);

    let _ = write!(out, "\nRESOLUTION\n");
    match (&ticket.resolution, &ticket.gist) {
        (Some(resolution), Some(gist)) => {
            let _ = writeln!(out, "  {resolution}\n  → {gist}");
        }
        (Some(resolution), None) => {
            let _ = writeln!(out, "  {resolution}");
        }
        (None, _) => {
            let _ = writeln!(out, "  (not yet resolved)");
        }
    }
    if let Some(tokens_used) = ticket.tokens_used {
        let _ = writeln!(out, "  tokens used: {tokens_used}");
    }

    let _ = write!(out, "\nASSETS\n");
    if assets.is_empty() {
        let _ = writeln!(out, "  (none)");
    } else {
        for asset in assets {
            let kind = asset.kind.as_deref().unwrap_or("asset");
            let path = asset.path.as_deref().unwrap_or("(no path)");
            let mut line = format!("  {kind}: {path}");
            if let Some(note) = &asset.note {
                let _ = write!(line, " — {note}");
            }
            out.push_str(&line);
            out.push('\n');
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::journal::{
        AssetAdded, JournalEvent, MapCreated, MapStatus, TicketCreated, TicketStatus, TicketType,
        TicketUpdated,
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
    async fn zooms_into_an_unresolved_ticket() {
        let dir = TempDir::new().unwrap();
        let mut store = Store::open(dir.path()).unwrap();
        seed_map(&mut store);
        store
            .apply(&JournalEvent::TicketCreated(TicketCreated {
                id: "T1".to_owned(),
                map_id: "M1".to_owned(),
                name: "Staleness policy".to_owned(),
                question: "How does a pack declare its staleness policy?".to_owned(),
                ticket_type: TicketType::Grilling,
                hitl: true,
                status: TicketStatus::Open,
                created_at: 1_700_000_000_000,
                ordinal: 0,
                axis: None,
                tokens_used: None,
            }))
            .unwrap();
        drop(store);

        let tool = TicketZoom::new();
        let result = tool
            .invoke(json!({"ticket_id": "T1"}), &ctx(dir.path()))
            .await
            .unwrap();

        assert!(result.content.contains("Staleness policy"));
        assert!(result.content.contains("HITL"));
        assert!(result.content.contains("How does a pack declare"));
        assert!(result.content.contains("not yet resolved"));
        assert!(result.content.contains("(none)"));
    }

    #[tokio::test]
    async fn zooms_into_a_resolved_ticket_with_assets() {
        let dir = TempDir::new().unwrap();
        let mut store = Store::open(dir.path()).unwrap();
        seed_map(&mut store);
        store
            .apply(&JournalEvent::TicketCreated(TicketCreated {
                id: "T1".to_owned(),
                map_id: "M1".to_owned(),
                name: "Pack identity".to_owned(),
                question: "How is a pack identified?".to_owned(),
                ticket_type: TicketType::Research,
                hitl: false,
                status: TicketStatus::Open,
                created_at: 1_700_000_000_000,
                ordinal: 0,
                axis: None,
                tokens_used: None,
            }))
            .unwrap();
        store
            .apply(&JournalEvent::TicketUpdated(TicketUpdated {
                id: "T1".to_owned(),
                status: Some(TicketStatus::Resolved),
                resolution: Some("Content-addressed by blake3 of the manifest.".to_owned()),
                gist: Some("blake3 of canonical manifest".to_owned()),
                resolved_at: Some(1_700_000_005_000),
                ..TicketUpdated::default()
            }))
            .unwrap();
        store
            .apply(&JournalEvent::AssetAdded(AssetAdded {
                id: "A1".to_owned(),
                ticket_id: "T1".to_owned(),
                kind: Some("note".to_owned()),
                path: Some("notes/pack-identity.md".to_owned()),
                note: Some("initial draft".to_owned()),
            }))
            .unwrap();
        drop(store);

        let tool = TicketZoom::new();
        let result = tool
            .invoke(json!({"ticket_id": "T1"}), &ctx(dir.path()))
            .await
            .unwrap();

        assert!(result.content.contains("Content-addressed by blake3"));
        assert!(result.content.contains("blake3 of canonical manifest"));
        assert!(result.content.contains("notes/pack-identity.md"));
        assert!(result.content.contains("initial draft"));
    }

    #[tokio::test]
    async fn an_unknown_ticket_is_map_not_found() {
        let dir = TempDir::new().unwrap();
        Store::open(dir.path()).unwrap();
        let tool = TicketZoom::new();

        let err = tool
            .invoke(json!({"ticket_id": "no-such-ticket"}), &ctx(dir.path()))
            .await
            .unwrap_err();
        assert_eq!(err.code, dark_contract::ErrCode::MapNotFound);
    }
}
