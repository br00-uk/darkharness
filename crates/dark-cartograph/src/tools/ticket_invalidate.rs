//! The `ticket_invalidate` tool.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use dark_contract::{Result, Tool, ToolCtx, ToolResult, ToolSchema, tool::tier};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::journal::{JournalEvent, TicketStatus, TicketUpdated};

use super::{
    CartographSession, invalid_args, journal_then_apply, load_ticket, now_ms, open_store,
    require_not_terminal,
};

#[derive(Debug, Deserialize)]
struct Args {
    ticket_id: String,
    reason: String,
}

/// Marks a ticket invalidated: a later decision made it void.
///
/// [`crate::journal::TicketUpdated`] (task unit `D1`) has no field of its
/// own for an invalidation reason — only `resolution`, `gist`, and the
/// other fields `ticket_resolve` uses. This tool records `reason` in
/// `resolution`, the same column a resolution's answer lives in, since a
/// reason for voiding a ticket plays the same role there that an answer
/// plays for a resolved one. It does not set `gist`, so an invalidated
/// ticket does not appear in the digest's decisions section, which reads
/// only resolved tickets. See the top-level report for this reading.
#[derive(Debug)]
pub struct TicketInvalidate {
    session: Arc<CartographSession>,
}

impl TicketInvalidate {
    /// Creates the tool, sharing `session` with the other mutating
    /// ticket tools in this harness session.
    #[must_use]
    pub fn new(session: Arc<CartographSession>) -> Self {
        Self { session }
    }
}

impl Tool for TicketInvalidate {
    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "ticket_invalidate".to_string(),
            description: "Marks a ticket invalidated: a later decision made it void. Refuses \
                a ticket that already resolved, left scope, or was already invalidated."
                .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "ticket_id": {
                        "type": "string",
                        "description": "The ticket to invalidate.",
                    },
                    "reason": {
                        "type": "string",
                        "description": "Why the ticket no longer applies.",
                    },
                },
                "required": ["ticket_id", "reason"],
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
            let args: Args = serde_json::from_value(args)
                .map_err(|err| invalid_args("ticket_invalidate", err))?;

            let mut store = open_store(ctx)?;
            let ticket = load_ticket(&store, &args.ticket_id)?;
            require_not_terminal(&args.ticket_id, ticket.status)?;

            let event = JournalEvent::TicketUpdated(TicketUpdated {
                id: args.ticket_id.clone(),
                status: Some(TicketStatus::Invalidated),
                resolution: Some(args.reason),
                resolved_at: Some(now_ms()),
                ..TicketUpdated::default()
            });
            journal_then_apply(&mut store, &self.session.maps_root, &ticket.map_id, &event)?;

            Ok(ToolResult::ok(format!("invalidated {}", args.ticket_id)))
        })
    }

    fn preview<'life0, 'life1, 'async_trait>(
        &'life0 self,
        args: Value,
        ctx: &'life1 ToolCtx,
    ) -> Pin<Box<dyn Future<Output = Result<Option<ToolResult>>> + Send + 'async_trait>>
    where
        'life0: 'async_trait,
        'life1: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move {
            let args: Args = serde_json::from_value(args)
                .map_err(|err| invalid_args("ticket_invalidate", err))?;

            let store = open_store(ctx)?;
            let ticket = load_ticket(&store, &args.ticket_id)?;

            let diff = format!(
                "--- {ticket_id} (before)\n\
                 +++ {ticket_id} (after)\n\
                 -status: {old_status}\n\
                 +status: invalidated\n\
                 -resolution: {old_resolution}\n\
                 +resolution: {new_reason:?}\n",
                ticket_id = args.ticket_id,
                old_status = ticket.status.as_str(),
                old_resolution = ticket.resolution.as_deref().unwrap_or("(none)"),
                new_reason = args.reason,
            );
            let summary = format!("would invalidate {}", args.ticket_id);
            Ok(Some(ToolResult::ok(summary).with_diff(diff)))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::journal::{JournalEvent, MapCreated, MapStatus, TicketCreated, TicketType};
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

    fn seed(store: &mut Store) {
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
        store
            .apply(&JournalEvent::TicketCreated(TicketCreated {
                id: "T1".to_owned(),
                map_id: "M1".to_owned(),
                name: "T1".to_owned(),
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
    async fn invalidates_an_open_ticket() {
        let dir = TempDir::new().unwrap();
        let mut store = Store::open(dir.path()).unwrap();
        seed(&mut store);
        drop(store);
        let session = Arc::new(CartographSession::new(dir.path().join("maps"), "s"));
        let tool = TicketInvalidate::new(session);

        let result = tool
            .invoke(
                json!({"ticket_id": "T1", "reason": "superseded by T5"}),
                &ctx(dir.path()),
            )
            .await
            .unwrap();
        assert!(!result.is_error);

        let store = Store::open(dir.path()).unwrap();
        let (status, resolution): (String, String) = store
            .connection()
            .query_row(
                "SELECT status, resolution FROM tickets WHERE id = 'T1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(status, "invalidated");
        assert_eq!(resolution, "superseded by T5");
    }

    #[tokio::test]
    async fn an_invalidated_ticket_does_not_appear_as_a_decision() {
        let dir = TempDir::new().unwrap();
        let mut store = Store::open(dir.path()).unwrap();
        seed(&mut store);
        drop(store);
        let session = Arc::new(CartographSession::new(dir.path().join("maps"), "s"));
        TicketInvalidate::new(session)
            .invoke(
                json!({"ticket_id": "T1", "reason": "superseded"}),
                &ctx(dir.path()),
            )
            .await
            .unwrap();

        let store = Store::open(dir.path()).unwrap();
        let text = crate::digest::render(&store, "M1", crate::digest::Tier::Full)
            .unwrap()
            .unwrap();
        assert!(!text.contains("DECISIONS SO FAR"));
    }

    #[tokio::test]
    async fn invalidating_an_already_resolved_ticket_is_refused() {
        let dir = TempDir::new().unwrap();
        let mut store = Store::open(dir.path()).unwrap();
        seed(&mut store);
        store
            .apply(&JournalEvent::TicketUpdated(TicketUpdated {
                id: "T1".to_owned(),
                status: Some(TicketStatus::Resolved),
                resolved_at: Some(1_700_000_005_000),
                ..TicketUpdated::default()
            }))
            .unwrap();
        drop(store);
        let session = Arc::new(CartographSession::new(dir.path().join("maps"), "s"));
        let tool = TicketInvalidate::new(session);

        let err = tool
            .invoke(
                json!({"ticket_id": "T1", "reason": "too late"}),
                &ctx(dir.path()),
            )
            .await
            .unwrap_err();
        assert_eq!(err.code, dark_contract::ErrCode::ToolInvalidArgs);
    }

    #[tokio::test]
    async fn an_unknown_ticket_is_map_not_found() {
        let dir = TempDir::new().unwrap();
        Store::open(dir.path()).unwrap();
        let session = Arc::new(CartographSession::new(dir.path().join("maps"), "s"));
        let tool = TicketInvalidate::new(session);

        let err = tool
            .invoke(
                json!({"ticket_id": "no-such-ticket", "reason": "why"}),
                &ctx(dir.path()),
            )
            .await
            .unwrap_err();
        assert_eq!(err.code, dark_contract::ErrCode::MapNotFound);
    }

    #[tokio::test]
    async fn preview_shows_the_change_without_applying_it() {
        let dir = TempDir::new().unwrap();
        let mut store = Store::open(dir.path()).unwrap();
        seed(&mut store);
        drop(store);
        let session = Arc::new(CartographSession::new(dir.path().join("maps"), "s"));
        let tool = TicketInvalidate::new(session);

        let preview = tool
            .preview(
                json!({"ticket_id": "T1", "reason": "superseded"}),
                &ctx(dir.path()),
            )
            .await
            .unwrap()
            .expect("ticket_invalidate can preview");
        let diff = preview.diff.expect("a preview fills in the diff");
        assert!(diff.contains("+status: invalidated"));
        assert!(diff.contains("superseded"));

        let store = Store::open(dir.path()).unwrap();
        let status: String = store
            .connection()
            .query_row("SELECT status FROM tickets WHERE id = 'T1'", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(status, "open", "a preview must not change anything");
    }
}
