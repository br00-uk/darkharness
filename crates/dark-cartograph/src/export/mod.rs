//! Exports a map to a shared tracker. Task unit `D5`.
//!
//! [`export`] is a pure function of the map's stored state: no clock, no
//! absolute path, and every list this module reads is already ordered
//! deterministically by [`query::load`] (ordinal, then identifier, the
//! same tie-break the rest of this crate uses — see
//! `crate::digest::query::load_decisions` for the precedent). Rendering
//! the same map twice therefore produces byte-identical output; the
//! integration test `tests/export_purity.rs` pins this for all three
//! formats, the same way `tests/digest_budget.rs` pins it for the
//! digest.
//!
//! **Export is one way in version 1** (task unit `D5`, step 3). Nothing
//! in this crate reads a shared tracker's state back into a map: a
//! ticket a person edits on GitHub after export, or resolves there, does
//! not reach the journal. Running an export again does not update the
//! first one — it produces an independent second set of issues. A
//! future version that closes this loop needs its own task unit.
//!
//! [`Format::Github`] never contacts GitHub — see the module
//! documentation on [`github`] for why that would break Rule 13
//! (`CLAUDE.md`) — so it renders [`github::Plan`] as JSON instead, for a
//! layer with real network access to execute.

mod github;
mod markdown;
mod mermaid;
mod query;

use dark_contract::{ErrCode, Error, Result};

use crate::store::Store;

pub use github::{IssueDraft, MAP_LABEL, Plan, TICKET_LABEL, TicketIssue};

/// The format `dark map export --format=` accepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// A [`Plan`], as JSON, for a layer with GitHub access to execute.
    Github,
    /// A single Markdown document: destination, notes, a ticket
    /// checklist with blockers, fog, and scope exclusions.
    Markdown,
    /// A Mermaid `flowchart TD`: one node per ticket, one arrow per
    /// blocking edge, styled by status.
    Mermaid,
}

/// Exports `map_id` in `format`.
///
/// # Errors
///
/// Returns [`ErrCode::MapNotFound`] when no map has this identifier.
/// Returns an error when the underlying database read fails, or — for
/// [`Format::Github`] only — when rendering the plan to JSON fails.
pub fn export(store: &Store, map_id: &str, format: Format) -> Result<String> {
    let snapshot = query::load(store, map_id)?;
    match format {
        Format::Github => github::render(&github::build(&snapshot)),
        Format::Markdown => Ok(markdown::render(&snapshot)),
        Format::Mermaid => Ok(mermaid::render(&snapshot)),
    }
}

/// Builds the GitHub export plan for `map_id`, as structured data rather
/// than the JSON text [`export`] returns for [`Format::Github`].
///
/// A layer that holds a `dark-airlock` client and executes this plan in
/// the same process wants [`Plan`] directly, not a round trip through
/// JSON; [`export`] exists for a caller that only wants text in one of
/// the three formats, uniformly.
///
/// # Errors
///
/// Returns [`ErrCode::MapNotFound`] when no map has this identifier.
/// Returns an error when the underlying database read fails.
pub fn github_plan(store: &Store, map_id: &str) -> Result<Plan> {
    let snapshot = query::load(store, map_id)?;
    Ok(github::build(&snapshot))
}

/// Parses a `--format` value into a [`Format`].
///
/// # Errors
///
/// Returns [`ErrCode::ToolInvalidArgs`] for anything other than
/// `github`, `markdown`, or `mermaid`.
pub fn parse_format(value: &str) -> Result<Format> {
    match value {
        "github" => Ok(Format::Github),
        "markdown" => Ok(Format::Markdown),
        "mermaid" => Ok(Format::Mermaid),
        other => Err(Error::new(
            ErrCode::ToolInvalidArgs,
            format!("export format must be github, markdown, or mermaid, got {other:?}"),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::journal::{
        EdgeAdded, FogAdded, JournalEvent, MapCreated, MapStatus, ScopeExclusionAdded,
        TicketCreated, TicketStatus, TicketType, TicketUpdated,
    };
    use tempfile::TempDir;

    fn built_store() -> (TempDir, Store) {
        let tmp = TempDir::new().expect("tempdir");
        let mut store = Store::open(tmp.path()).expect("open store");

        store
            .apply(&JournalEvent::MapCreated(MapCreated {
                id: "M1".to_owned(),
                name: "Offline pack format".to_owned(),
                destination: "A frozen, versioned pack format.".to_owned(),
                notes: Some("Domain: Rust.".to_owned()),
                created_at: 1_700_000_000_000,
                status: MapStatus::Active,
            }))
            .unwrap();

        store
            .apply(&JournalEvent::TicketCreated(TicketCreated {
                id: "T-004".to_owned(),
                map_id: "M1".to_owned(),
                name: "Pack identity is content-addressed".to_owned(),
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
                id: "T-004".to_owned(),
                status: Some(TicketStatus::Resolved),
                resolution: Some("Content-addressed by blake3.".to_owned()),
                gist: Some("blake3 of canonical manifest".to_owned()),
                resolved_at: Some(1_700_000_005_000),
                ..TicketUpdated::default()
            }))
            .unwrap();

        store
            .apply(&JournalEvent::TicketCreated(TicketCreated {
                id: "T-018".to_owned(),
                map_id: "M1".to_owned(),
                name: "Staleness policy".to_owned(),
                question: "How does a pack declare its staleness policy?".to_owned(),
                ticket_type: TicketType::Grilling,
                hitl: true,
                status: TicketStatus::Open,
                created_at: 1_700_000_001_000,
                ordinal: 1,
                axis: None,
                tokens_used: None,
            }))
            .unwrap();
        store
            .apply(&JournalEvent::TicketCreated(TicketCreated {
                id: "T-020".to_owned(),
                map_id: "M1".to_owned(),
                name: "Follow-on staleness work".to_owned(),
                question: "What follows from the staleness policy?".to_owned(),
                ticket_type: TicketType::Task,
                hitl: false,
                status: TicketStatus::Open,
                created_at: 1_700_000_002_000,
                ordinal: 2,
                axis: None,
                tokens_used: None,
            }))
            .unwrap();
        store
            .apply(&JournalEvent::EdgeAdded(EdgeAdded {
                blocker: "T-018".to_owned(),
                blocked: "T-020".to_owned(),
            }))
            .unwrap();

        store
            .apply(&JournalEvent::FogAdded(FogAdded {
                id: "F1".to_owned(),
                map_id: "M1".to_owned(),
                patch: "Whether reranking needs its own pack metadata.".to_owned(),
                axis: None,
                created_at: 1_700_000_003_000,
            }))
            .unwrap();

        store
            .apply(&JournalEvent::ScopeExclusionAdded(ScopeExclusionAdded {
                id: "S1".to_owned(),
                map_id: "M1".to_owned(),
                gist: "Pack signing and trust chain".to_owned(),
                reason: "separate effort".to_owned(),
                ticket_id: Some("T-009".to_owned()),
            }))
            .unwrap();

        (tmp, store)
    }

    #[test]
    fn parse_format_accepts_the_three_named_formats() {
        assert_eq!(parse_format("github").unwrap(), Format::Github);
        assert_eq!(parse_format("markdown").unwrap(), Format::Markdown);
        assert_eq!(parse_format("mermaid").unwrap(), Format::Mermaid);
    }

    #[test]
    fn parse_format_rejects_anything_else() {
        let err = parse_format("csv").unwrap_err();
        assert_eq!(err.code, ErrCode::ToolInvalidArgs);
    }

    #[test]
    fn export_reports_map_not_found_for_an_unknown_map() {
        let (_tmp, store) = built_store();
        let err = export(&store, "no-such-map", Format::Markdown).unwrap_err();
        assert_eq!(err.code, ErrCode::MapNotFound);
    }

    #[test]
    fn markdown_export_covers_every_section() {
        let (_tmp, store) = built_store();
        let text = export(&store, "M1", Format::Markdown).unwrap();

        assert!(text.starts_with("# Offline pack format"));
        assert!(text.contains("## Destination"));
        assert!(text.contains("## Notes"));
        assert!(text.contains("## Tickets"));
        assert!(text.contains("[x]"), "a resolved ticket must be checked");
        assert!(text.contains("T-020"));
        assert!(text.contains("Blocked by: T-018"));
        assert!(text.contains("## Fog"));
        assert!(text.contains("## Out of scope"));
        assert!(text.contains("Pack signing and trust chain"));
    }

    #[test]
    fn mermaid_export_names_every_ticket_and_edge() {
        let (_tmp, store) = built_store();
        let text = export(&store, "M1", Format::Mermaid).unwrap();

        assert!(text.starts_with("flowchart TD"));
        assert!(text.contains("T-004"));
        assert!(text.contains("T-018"));
        assert!(text.contains("T-020"));
        assert!(text.contains("-->"));
        assert!(text.contains("classDef resolved"));
    }

    #[test]
    fn github_export_produces_valid_json_with_the_map_and_ticket_labels() {
        let (_tmp, store) = built_store();
        let text = export(&store, "M1", Format::Github).unwrap();

        // Parsed as plain JSON, not deserialised back into `Plan`: the
        // module documentation on `github` says export is one way, so
        // this crate offers no `Deserialize` impl for its own output —
        // a round trip through `serde_json::Value` is enough to confirm
        // the text is valid, well-shaped JSON.
        let value: serde_json::Value = serde_json::from_str(&text).expect("the plan is valid JSON");
        let parent_labels = value["parent"]["labels"].as_array().unwrap();
        assert!(parent_labels.iter().any(|l| l == MAP_LABEL));

        let children = value["children"].as_array().unwrap();
        assert_eq!(children.len(), 3);
        for child in children {
            let labels = child["issue"]["labels"].as_array().unwrap();
            assert!(labels.iter().any(|l| l == TICKET_LABEL));
        }

        let staleness = children.iter().find(|c| c["ticket_id"] == "T-020").unwrap();
        assert_eq!(
            staleness["blocked_by"].as_array().unwrap(),
            &[serde_json::Value::String("T-018".to_owned())]
        );

        let identity = children.iter().find(|c| c["ticket_id"] == "T-004").unwrap();
        assert_eq!(
            identity["close"], true,
            "a resolved ticket's issue should close"
        );
    }

    #[test]
    fn every_format_renders_the_same_map_twice_byte_identically() {
        let (_tmp, store) = built_store();
        for format in [Format::Github, Format::Markdown, Format::Mermaid] {
            let first = export(&store, "M1", format).unwrap();
            let second = export(&store, "M1", format).unwrap();
            assert_eq!(first, second, "{format:?} export must be pure");
        }
    }
}
