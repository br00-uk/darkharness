//! The `ticket_resolve` tool.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use dark_contract::{ErrCode, Error, Result, Tool, ToolCtx, ToolResult, ToolSchema, tool::tier};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::journal::{JournalEvent, TicketStatus, TicketType, TicketUpdated};

use super::{
    CartographSession, TicketRow, invalid_args, journal_then_apply, load_ticket, now_ms,
    open_store, require_not_terminal,
};

#[derive(Debug, Deserialize)]
struct Args {
    ticket_id: String,
    resolution: String,
    gist: String,
    tokens_used: i64,
}

/// Resolves a ticket in one transaction: records the answer, closes the
/// ticket, and — by setting `gist` in the same `UPDATE` as `status` —
/// makes it appear in the digest's decisions section. See task unit `D4`,
/// step 2.
///
/// Enforces two rules from `CLAUDE.md`'s constraints section:
///
/// - **Rule 19.** A human-in-the-loop ticket needs
///   [`dark_contract::ToolCtx::human_present`]. Without it this tool
///   returns [`ErrCode::HitlRequiresHuman`] and changes nothing.
/// - **Rule 20.** A session resolves at most one non-research ticket. A
///   second one returns [`ErrCode::SessionResolutionLimit`]. Research
///   tickets are exempt: resolving one never sets the limit, and never
///   trips it.
#[derive(Debug)]
pub struct TicketResolve {
    session: Arc<CartographSession>,
}

impl TicketResolve {
    /// Creates the tool, sharing `session` with the other mutating
    /// ticket tools in this harness session.
    #[must_use]
    pub fn new(session: Arc<CartographSession>) -> Self {
        Self { session }
    }
}

impl Tool for TicketResolve {
    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "ticket_resolve".to_string(),
            description: "Resolves a ticket: records the answer, closes it, and indexes the \
                gist as a decision. A human-in-the-loop ticket needs a person present. A \
                session resolves at most one non-research ticket."
                .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "ticket_id": {
                        "type": "string",
                        "description": "The ticket to resolve.",
                    },
                    "resolution": {
                        "type": "string",
                        "description": "The full answer.",
                    },
                    "gist": {
                        "type": "string",
                        "description": "A short summary of the resolution, for the digest's \
                            decisions section.",
                    },
                    "tokens_used": {
                        "type": "integer",
                        "description": "Tokens spent resolving this ticket.",
                    },
                },
                "required": ["ticket_id", "resolution", "gist", "tokens_used"],
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
                serde_json::from_value(args).map_err(|err| invalid_args("ticket_resolve", err))?;

            let mut store = open_store(ctx)?;
            let ticket = load_ticket(&store, &args.ticket_id)?;
            require_not_terminal(&args.ticket_id, ticket.status)?;
            check_hitl(&args.ticket_id, &ticket, ctx.human_present)?;
            check_session_limit(&args.ticket_id, ticket.ticket_type, &self.session)?;

            let event = JournalEvent::TicketUpdated(TicketUpdated {
                id: args.ticket_id.clone(),
                status: Some(TicketStatus::Resolved),
                resolution: Some(args.resolution),
                gist: Some(args.gist),
                resolved_at: Some(now_ms()),
                tokens_used: Some(args.tokens_used),
                ..TicketUpdated::default()
            });
            journal_then_apply(&mut store, &self.session.maps_root, &ticket.map_id, &event)?;

            if !matches!(ticket.ticket_type, TicketType::Research) {
                self.session
                    .resolved_non_research
                    .store(true, Ordering::Release);
            }

            Ok(ToolResult::ok(format!("resolved {}", args.ticket_id)))
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
            let args: Args =
                serde_json::from_value(args).map_err(|err| invalid_args("ticket_resolve", err))?;

            let store = open_store(ctx)?;
            let ticket = load_ticket(&store, &args.ticket_id)?;

            let diff = format!(
                "--- {ticket_id} (before)\n\
                 +++ {ticket_id} (after)\n\
                 -status: {old_status}\n\
                 +status: resolved\n\
                 -resolution: {old_resolution}\n\
                 +resolution: {new_resolution:?}\n\
                 -gist: {old_gist}\n\
                 +gist: {new_gist:?}\n",
                ticket_id = args.ticket_id,
                old_status = ticket.status.as_str(),
                old_resolution = ticket.resolution.as_deref().unwrap_or("(none)"),
                new_resolution = args.resolution,
                old_gist = ticket.gist.as_deref().unwrap_or("(none)"),
                new_gist = args.gist,
            );
            let summary = format!("would resolve {}", args.ticket_id);
            Ok(Some(ToolResult::ok(summary).with_diff(diff)))
        })
    }
}

/// Enforces Rule 19: a human-in-the-loop ticket needs a person present.
fn check_hitl(ticket_id: &str, ticket: &TicketRow, human_present: bool) -> Result<()> {
    if ticket.hitl && !human_present {
        return Err(Error::new(
            ErrCode::HitlRequiresHuman,
            format!("ticket {ticket_id} needs a person; no human-present token is held"),
        ));
    }
    Ok(())
}

/// Enforces Rule 20: a session resolves at most one non-research ticket.
/// Research tickets are exempt from the check entirely.
fn check_session_limit(
    ticket_id: &str,
    ticket_type: TicketType,
    session: &CartographSession,
) -> Result<()> {
    if matches!(ticket_type, TicketType::Research) {
        return Ok(());
    }
    if session.resolved_non_research.load(Ordering::Acquire) {
        return Err(Error::new(
            ErrCode::SessionResolutionLimit,
            format!(
                "this session already resolved a non-research ticket; cannot resolve \
                 {ticket_id} in the same session"
            ),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::journal::{JournalEvent, MapCreated, MapStatus, TicketCreated};
    use crate::store::Store;
    use tempfile::TempDir;

    // `tokio_util` is not a declared dependency of this crate (Rule 16), so
    // `CancellationToken` cannot be named here — only `ToolCtx.cancel`'s
    // already-resolved field type names it. `Default::default()` is the one
    // way to build one without naming the type.
    #[allow(clippy::default_trait_access)]
    fn ctx(root: &std::path::Path, human_present: bool) -> ToolCtx {
        let bus = dark_contract::EventBus::new();
        ToolCtx {
            root: root.to_path_buf(),
            events: bus.tx(),
            cancel: Default::default(),
            dark: true,
            human_present,
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

    fn seed_ticket(store: &mut Store, id: &str, ticket_type: TicketType, hitl: bool) {
        store
            .apply(&JournalEvent::TicketCreated(TicketCreated {
                id: id.to_owned(),
                map_id: "M1".to_owned(),
                name: id.to_owned(),
                question: "Q?".to_owned(),
                ticket_type,
                hitl,
                status: TicketStatus::Open,
                created_at: 1_700_000_000_000,
                ordinal: 0,
                axis: None,
                tokens_used: None,
            }))
            .unwrap();
    }

    fn resolve_args(ticket_id: &str) -> Value {
        json!({
            "ticket_id": ticket_id,
            "resolution": "The answer.",
            "gist": "short gist",
            "tokens_used": 1200,
        })
    }

    #[tokio::test]
    async fn resolves_a_task_ticket_in_one_transaction() {
        let dir = TempDir::new().unwrap();
        let mut store = Store::open(dir.path()).unwrap();
        seed_map(&mut store);
        seed_ticket(&mut store, "T1", TicketType::Task, false);
        drop(store);
        let session = Arc::new(CartographSession::new(dir.path().join("maps"), "s"));
        let tool = TicketResolve::new(session);

        let result = tool
            .invoke(resolve_args("T1"), &ctx(dir.path(), false))
            .await
            .unwrap();
        assert!(!result.is_error);

        let store = Store::open(dir.path()).unwrap();
        let (status, resolution, gist, tokens_used): (String, String, String, i64) = store
            .connection()
            .query_row(
                "SELECT status, resolution, gist, tokens_used FROM tickets WHERE id = 'T1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert_eq!(status, "resolved");
        assert_eq!(resolution, "The answer.");
        assert_eq!(gist, "short gist");
        assert_eq!(tokens_used, 1200);
    }

    #[tokio::test]
    async fn a_resolved_ticket_appears_in_the_digest_decisions_immediately() {
        let dir = TempDir::new().unwrap();
        let mut store = Store::open(dir.path()).unwrap();
        seed_map(&mut store);
        seed_ticket(&mut store, "T1", TicketType::Task, false);
        drop(store);
        let session = Arc::new(CartographSession::new(dir.path().join("maps"), "s"));
        TicketResolve::new(session)
            .invoke(resolve_args("T1"), &ctx(dir.path(), false))
            .await
            .unwrap();

        let store = Store::open(dir.path()).unwrap();
        let text = crate::digest::render(&store, "M1", crate::digest::Tier::Full)
            .unwrap()
            .unwrap();
        assert!(text.contains("short gist"));
    }

    #[tokio::test]
    async fn a_hitl_ticket_without_a_human_fails_rule_19() {
        let dir = TempDir::new().unwrap();
        let mut store = Store::open(dir.path()).unwrap();
        seed_map(&mut store);
        seed_ticket(&mut store, "T1", TicketType::Grilling, true);
        drop(store);
        let session = Arc::new(CartographSession::new(dir.path().join("maps"), "s"));
        let tool = TicketResolve::new(session);

        let err = tool
            .invoke(resolve_args("T1"), &ctx(dir.path(), false))
            .await
            .unwrap_err();
        assert_eq!(err.code, dark_contract::ErrCode::HitlRequiresHuman);

        let store = Store::open(dir.path()).unwrap();
        let status: String = store
            .connection()
            .query_row("SELECT status FROM tickets WHERE id = 'T1'", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(
            status, "open",
            "a rejected resolution must not change anything"
        );
    }

    #[tokio::test]
    async fn a_hitl_ticket_with_a_human_present_succeeds() {
        let dir = TempDir::new().unwrap();
        let mut store = Store::open(dir.path()).unwrap();
        seed_map(&mut store);
        seed_ticket(&mut store, "T1", TicketType::Grilling, true);
        drop(store);
        let session = Arc::new(CartographSession::new(dir.path().join("maps"), "s"));
        let tool = TicketResolve::new(session);

        let result = tool
            .invoke(resolve_args("T1"), &ctx(dir.path(), true))
            .await
            .unwrap();
        assert!(!result.is_error);
    }

    #[tokio::test]
    async fn a_second_non_research_resolution_fails_rule_20() {
        let dir = TempDir::new().unwrap();
        let mut store = Store::open(dir.path()).unwrap();
        seed_map(&mut store);
        seed_ticket(&mut store, "T1", TicketType::Task, false);
        seed_ticket(&mut store, "T2", TicketType::Task, false);
        drop(store);
        let session = Arc::new(CartographSession::new(dir.path().join("maps"), "s"));

        TicketResolve::new(session.clone())
            .invoke(resolve_args("T1"), &ctx(dir.path(), false))
            .await
            .unwrap();

        let err = TicketResolve::new(session)
            .invoke(resolve_args("T2"), &ctx(dir.path(), false))
            .await
            .unwrap_err();
        assert_eq!(err.code, dark_contract::ErrCode::SessionResolutionLimit);
    }

    #[tokio::test]
    async fn research_tickets_are_exempt_from_the_session_limit() {
        let dir = TempDir::new().unwrap();
        let mut store = Store::open(dir.path()).unwrap();
        seed_map(&mut store);
        seed_ticket(&mut store, "T1", TicketType::Research, false);
        seed_ticket(&mut store, "T2", TicketType::Research, false);
        seed_ticket(&mut store, "T3", TicketType::Research, false);
        drop(store);
        let session = Arc::new(CartographSession::new(dir.path().join("maps"), "s"));

        for id in ["T1", "T2", "T3"] {
            let result = TicketResolve::new(session.clone())
                .invoke(resolve_args(id), &ctx(dir.path(), false))
                .await
                .unwrap();
            assert!(!result.is_error);
        }
    }

    #[tokio::test]
    async fn a_research_resolution_does_not_block_a_later_non_research_one() {
        let dir = TempDir::new().unwrap();
        let mut store = Store::open(dir.path()).unwrap();
        seed_map(&mut store);
        seed_ticket(&mut store, "T1", TicketType::Research, false);
        seed_ticket(&mut store, "T2", TicketType::Task, false);
        drop(store);
        let session = Arc::new(CartographSession::new(dir.path().join("maps"), "s"));

        TicketResolve::new(session.clone())
            .invoke(resolve_args("T1"), &ctx(dir.path(), false))
            .await
            .unwrap();
        let result = TicketResolve::new(session)
            .invoke(resolve_args("T2"), &ctx(dir.path(), false))
            .await
            .unwrap();
        assert!(!result.is_error);
    }

    #[tokio::test]
    async fn resolving_an_already_resolved_ticket_is_refused() {
        let dir = TempDir::new().unwrap();
        let mut store = Store::open(dir.path()).unwrap();
        seed_map(&mut store);
        seed_ticket(&mut store, "T1", TicketType::Task, false);
        drop(store);
        let session = Arc::new(CartographSession::new(dir.path().join("maps"), "s"));

        TicketResolve::new(session.clone())
            .invoke(resolve_args("T1"), &ctx(dir.path(), false))
            .await
            .unwrap();
        let err = TicketResolve::new(session)
            .invoke(resolve_args("T1"), &ctx(dir.path(), false))
            .await
            .unwrap_err();
        assert_eq!(err.code, dark_contract::ErrCode::ToolInvalidArgs);
    }

    #[tokio::test]
    async fn preview_shows_the_change_without_applying_it() {
        let dir = TempDir::new().unwrap();
        let mut store = Store::open(dir.path()).unwrap();
        seed_map(&mut store);
        seed_ticket(&mut store, "T1", TicketType::Task, false);
        drop(store);
        let session = Arc::new(CartographSession::new(dir.path().join("maps"), "s"));
        let tool = TicketResolve::new(session);

        let preview = tool
            .preview(resolve_args("T1"), &ctx(dir.path(), false))
            .await
            .unwrap()
            .expect("ticket_resolve can preview");
        let diff = preview.diff.expect("a preview fills in the diff");
        assert!(diff.contains("-status: open"));
        assert!(diff.contains("+status: resolved"));
        assert!(diff.contains("The answer."));

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
