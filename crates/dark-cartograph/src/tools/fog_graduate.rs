//! The `fog_graduate` tool.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use dark_contract::{ErrCode, Error, Result, Tool, ToolCtx, ToolResult, ToolSchema, tool::tier};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::journal::{FogGraduated, JournalEvent};

use super::{CartographSession, invalid_args, journal_then_apply, load_fog, open_store};

#[derive(Debug, Deserialize)]
struct Args {
    fog_id: String,
    ticket_ids: Vec<String>,
}

/// Marks a fog patch graduated: the question it held is now precise
/// enough to be one or more tickets. Clears the patch — task unit `D4`,
/// step 6 — so it then exists only as the tickets it names.
///
/// The build specification gives this tool's signature as
/// `fog_graduate(fog_id, ticket_ids[])`, plural, but
/// [`crate::journal::FogGraduated`] (task unit `D1`) carries one
/// `graduated_to: String`, not a list — and this task unit does not own
/// that struct. This tool records every id the caller passes, joined with
/// `", "`, in that one field. See the top-level report: this is a real
/// mismatch between the tool signature and the journal payload it must
/// use, not a design choice made freely.
#[derive(Debug)]
pub struct FogGraduate {
    session: Arc<CartographSession>,
}

impl FogGraduate {
    /// Creates the tool, sharing `session` with the other mutating
    /// ticket tools in this harness session.
    #[must_use]
    pub fn new(session: Arc<CartographSession>) -> Self {
        Self { session }
    }
}

impl Tool for FogGraduate {
    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "fog_graduate".to_string(),
            description: "Marks a fog patch graduated into one or more tickets, and clears \
                the patch."
                .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "fog_id": {
                        "type": "string",
                        "description": "The fog patch that graduated.",
                    },
                    "ticket_ids": {
                        "type": "array",
                        "items": { "type": "string" },
                        "minItems": 1,
                        "description": "The ticket or tickets the fog patch became.",
                    },
                },
                "required": ["fog_id", "ticket_ids"],
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
                serde_json::from_value(args).map_err(|err| invalid_args("fog_graduate", err))?;
            if args.ticket_ids.is_empty() {
                return Err(Error::new(
                    ErrCode::ToolInvalidArgs,
                    "fog_graduate: ticket_ids must name at least one ticket",
                ));
            }

            let mut store = open_store(ctx)?;
            let fog = load_fog(&store, &args.fog_id)?;
            if let Some(graduated_to) = &fog.graduated_to {
                return Err(Error::new(
                    ErrCode::ToolInvalidArgs,
                    format!(
                        "fog patch {} already graduated to {graduated_to}",
                        args.fog_id
                    ),
                ));
            }

            let graduated_to = args.ticket_ids.join(", ");
            let event = JournalEvent::FogGraduated(FogGraduated {
                id: args.fog_id.clone(),
                graduated_to: graduated_to.clone(),
            });
            journal_then_apply(&mut store, &self.session.maps_root, &fog.map_id, &event)?;

            Ok(ToolResult::ok(format!(
                "fog {} graduated to {graduated_to}",
                args.fog_id
            )))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::journal::{FogAdded, MapCreated, MapStatus};
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
            .apply(&JournalEvent::FogAdded(FogAdded {
                id: "F1".to_owned(),
                map_id: "M1".to_owned(),
                patch: "How packs are distributed.".to_owned(),
                axis: None,
                created_at: 1_700_000_000_000,
            }))
            .unwrap();
    }

    #[tokio::test]
    async fn graduates_a_fog_patch_into_one_ticket() {
        let dir = TempDir::new().unwrap();
        let mut store = Store::open(dir.path()).unwrap();
        seed(&mut store);
        drop(store);
        let session = Arc::new(CartographSession::new(dir.path().join("maps"), "s"));
        let tool = FogGraduate::new(session);

        let result = tool
            .invoke(
                json!({"fog_id": "F1", "ticket_ids": ["T1"]}),
                &ctx(dir.path()),
            )
            .await
            .unwrap();
        assert!(!result.is_error);

        let store = Store::open(dir.path()).unwrap();
        let graduated_to: String = store
            .connection()
            .query_row("SELECT graduated_to FROM fog WHERE id = 'F1'", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(graduated_to, "T1");
    }

    #[tokio::test]
    async fn graduating_into_several_tickets_records_every_id() {
        let dir = TempDir::new().unwrap();
        let mut store = Store::open(dir.path()).unwrap();
        seed(&mut store);
        drop(store);
        let session = Arc::new(CartographSession::new(dir.path().join("maps"), "s"));
        let tool = FogGraduate::new(session);

        tool.invoke(
            json!({"fog_id": "F1", "ticket_ids": ["T1", "T2"]}),
            &ctx(dir.path()),
        )
        .await
        .unwrap();

        let store = Store::open(dir.path()).unwrap();
        let graduated_to: String = store
            .connection()
            .query_row("SELECT graduated_to FROM fog WHERE id = 'F1'", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(graduated_to, "T1, T2");
    }

    #[tokio::test]
    async fn a_graduated_patch_leaves_the_digests_fog_section() {
        let dir = TempDir::new().unwrap();
        let mut store = Store::open(dir.path()).unwrap();
        seed(&mut store);
        drop(store);
        let session = Arc::new(CartographSession::new(dir.path().join("maps"), "s"));
        FogGraduate::new(session)
            .invoke(
                json!({"fog_id": "F1", "ticket_ids": ["T1"]}),
                &ctx(dir.path()),
            )
            .await
            .unwrap();

        let store = Store::open(dir.path()).unwrap();
        let text = crate::digest::render(&store, "M1", crate::digest::Tier::Full)
            .unwrap()
            .unwrap();
        assert!(!text.contains("NOT YET SPECIFIED"));
    }

    #[tokio::test]
    async fn an_empty_ticket_ids_list_is_invalid_args() {
        let dir = TempDir::new().unwrap();
        let mut store = Store::open(dir.path()).unwrap();
        seed(&mut store);
        drop(store);
        let session = Arc::new(CartographSession::new(dir.path().join("maps"), "s"));
        let tool = FogGraduate::new(session);

        let err = tool
            .invoke(json!({"fog_id": "F1", "ticket_ids": []}), &ctx(dir.path()))
            .await
            .unwrap_err();
        assert_eq!(err.code, dark_contract::ErrCode::ToolInvalidArgs);
    }

    #[tokio::test]
    async fn graduating_an_already_graduated_patch_is_refused() {
        let dir = TempDir::new().unwrap();
        let mut store = Store::open(dir.path()).unwrap();
        seed(&mut store);
        drop(store);
        let session = Arc::new(CartographSession::new(dir.path().join("maps"), "s"));
        FogGraduate::new(session.clone())
            .invoke(
                json!({"fog_id": "F1", "ticket_ids": ["T1"]}),
                &ctx(dir.path()),
            )
            .await
            .unwrap();

        let err = FogGraduate::new(session)
            .invoke(
                json!({"fog_id": "F1", "ticket_ids": ["T2"]}),
                &ctx(dir.path()),
            )
            .await
            .unwrap_err();
        assert_eq!(err.code, dark_contract::ErrCode::ToolInvalidArgs);
    }

    #[tokio::test]
    async fn an_unknown_fog_patch_is_map_not_found() {
        let dir = TempDir::new().unwrap();
        Store::open(dir.path()).unwrap();
        let session = Arc::new(CartographSession::new(dir.path().join("maps"), "s"));
        let tool = FogGraduate::new(session);

        let err = tool
            .invoke(
                json!({"fog_id": "no-such-fog", "ticket_ids": ["T1"]}),
                &ctx(dir.path()),
            )
            .await
            .unwrap_err();
        assert_eq!(err.code, dark_contract::ErrCode::MapNotFound);
    }
}
