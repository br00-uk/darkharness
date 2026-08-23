//! The `ticket_claim` tool.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use dark_contract::{Result, Tool, ToolCtx, ToolResult, ToolSchema, tool::tier};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::frontier::{self, ClaimOutcome, DEFAULT_LEASE_MS};

use super::{CartographSession, invalid_args, load_ticket, now_ms, open_store};

#[derive(Debug, Deserialize)]
struct Args {
    ticket_id: String,
}

/// Claims a ticket under this session's identity, before any work on it
/// starts.
///
/// Records `claimed_by` as this [`CartographSession`]'s `session_id`,
/// which is the identity the tool itself supplies — the build
/// specification's `ticket_claim(ticket_id)` signature has no place for
/// the model to name a claimant.
#[derive(Debug)]
pub struct TicketClaim {
    session: Arc<CartographSession>,
}

impl TicketClaim {
    /// Creates the tool, sharing `session` with the other mutating
    /// ticket tools in this harness session.
    #[must_use]
    pub fn new(session: Arc<CartographSession>) -> Self {
        Self { session }
    }
}

impl Tool for TicketClaim {
    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "ticket_claim".to_string(),
            description: "Claims a ticket for this session, under a two-hour lease. Claim a \
                ticket before any work on it starts."
                .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "ticket_id": {
                        "type": "string",
                        "description": "The ticket to claim.",
                    },
                },
                "required": ["ticket_id"],
            }),
            tier: tier::ESSENTIAL,
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
                serde_json::from_value(args).map_err(|err| invalid_args("ticket_claim", err))?;

            let mut store = open_store(ctx)?;
            let ticket = load_ticket(&store, &args.ticket_id)?;

            let outcome = frontier::claim(
                &mut store,
                &self.session.maps_root,
                &ticket.map_id,
                &args.ticket_id,
                &self.session.session_id,
                now_ms(),
                DEFAULT_LEASE_MS,
            )?;

            match outcome {
                ClaimOutcome::Claimed {
                    ticket_id,
                    expires_at,
                } => Ok(ToolResult::ok(format!(
                    "claimed {ticket_id}; the lease expires at {expires_at}"
                ))),
                ClaimOutcome::NotAvailable => Ok(ToolResult::error(format!(
                    "{} is not available to claim: it is already claimed, resolved, out of \
                     scope, or invalidated",
                    args.ticket_id
                ))),
            }
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
    async fn claims_an_open_ticket() {
        let dir = TempDir::new().unwrap();
        let mut store = Store::open(dir.path()).unwrap();
        seed(&mut store);
        drop(store);
        let session = Arc::new(CartographSession::new(dir.path().join("maps"), "session-a"));
        let tool = TicketClaim::new(session);

        let result = tool
            .invoke(json!({"ticket_id": "T1"}), &ctx(dir.path()))
            .await
            .unwrap();
        assert!(!result.is_error);

        let store = Store::open(dir.path()).unwrap();
        let (status, claimed_by): (String, String) = store
            .connection()
            .query_row(
                "SELECT status, claimed_by FROM tickets WHERE id = 'T1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(status, "claimed");
        assert_eq!(claimed_by, "session-a");
    }

    #[tokio::test]
    async fn claiming_an_already_claimed_ticket_is_a_tool_error_not_a_hard_error() {
        let dir = TempDir::new().unwrap();
        let mut store = Store::open(dir.path()).unwrap();
        seed(&mut store);
        drop(store);
        let maps_root = dir.path().join("maps");
        let session_a = Arc::new(CartographSession::new(maps_root.clone(), "session-a"));
        let session_b = Arc::new(CartographSession::new(maps_root, "session-b"));

        TicketClaim::new(session_a)
            .invoke(json!({"ticket_id": "T1"}), &ctx(dir.path()))
            .await
            .unwrap();

        let result = TicketClaim::new(session_b)
            .invoke(json!({"ticket_id": "T1"}), &ctx(dir.path()))
            .await
            .unwrap();
        assert!(result.is_error);
    }

    #[tokio::test]
    async fn claiming_an_unknown_ticket_is_map_not_found() {
        let dir = TempDir::new().unwrap();
        Store::open(dir.path()).unwrap();
        let session = Arc::new(CartographSession::new(dir.path().join("maps"), "session-a"));
        let tool = TicketClaim::new(session);

        let err = tool
            .invoke(json!({"ticket_id": "no-such-ticket"}), &ctx(dir.path()))
            .await
            .unwrap_err();
        assert_eq!(err.code, dark_contract::ErrCode::MapNotFound);
    }
}
