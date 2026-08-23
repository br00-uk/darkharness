//! The map digest: the whole map, rendered small enough to sit in the
//! context prefix.
//!
//! `crates/dark-core/src/context/prefix.rs` places the digest at prefix
//! part 4, budgeted at 1200 tokens out of a 32k grant (`PRD.md`, task
//! unit `A3`). [`render`] is a pure function of the map's stored state —
//! no clock, no absolute path, no iteration order that depends on
//! anything but the data itself — because the prefix must not change
//! mid-turn (Rule 5, `CLAUDE.md`): a digest that renders two different
//! ways for the same map forces a full prefill, costing 15 to 30 seconds
//! on a 32B model.
//!
//! Three tiers control how much of the map appears (task unit `D3`, step
//! 4): [`Tier::Full`] for ticket resolution turns, [`Tier::FrontierOnly`]
//! for charting stages that only need orientation, and [`Tier::None`]
//! for stages that need no map context at all. [`Tier::Full`] compresses
//! itself, in the fixed sequence task unit `D3` step 2 names, whenever
//! the rendered text runs over an estimated budget — see
//! `estimate::ESTIMATED_BUDGET` for why that budget is an estimate and
//! not the real tokenizer's own count.

mod estimate;
mod query;
mod render;

use dark_contract::Result;

use crate::store::Store;

pub use render::Tier;

/// Renders the digest for `map_id` at `tier`.
///
/// Returns `Ok(None)` for [`Tier::None`] without reading the database:
/// a caller that asked for nothing gets nothing, at no query cost.
/// Returns `Ok(Some(text))` for [`Tier::Full`] and [`Tier::FrontierOnly`].
///
/// # Errors
///
/// Returns [`dark_contract::ErrCode::MapNotFound`] when no map has this
/// identifier. Returns an error when the underlying database read fails.
pub fn render(store: &Store, map_id: &str, tier: Tier) -> Result<Option<String>> {
    if matches!(tier, Tier::None) {
        return Ok(None);
    }
    let snapshot = query::load(store, map_id)?;
    Ok(Some(render::render(&snapshot, tier)))
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
                destination: "A frozen, versioned pack format that ships vendor docs offline."
                    .to_owned(),
                notes: Some("Domain: Rust, content-addressed storage.".to_owned()),
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
                gist: Some("blake3 of canonical manifest".to_owned()),
                resolved_at: Some(1_700_000_005_000),
                ..TicketUpdated::default()
            }))
            .unwrap();

        store
            .apply(&JournalEvent::TicketCreated(TicketCreated {
                id: "T-018".to_owned(),
                map_id: "M1".to_owned(),
                name: "How does a pack declare its staleness policy?".to_owned(),
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
    fn tier_none_returns_none_without_touching_the_database() {
        // A bogus map id would fail any real query; None must never run one.
        let (_tmp, store) = built_store();
        let result = render(&store, "no-such-map", Tier::None).unwrap();
        assert_eq!(result, None);
    }

    #[test]
    fn tier_full_reports_map_not_found_for_an_unknown_map() {
        let (_tmp, store) = built_store();
        let err = render(&store, "no-such-map", Tier::Full).unwrap_err();
        assert_eq!(err.code, dark_contract::ErrCode::MapNotFound);
    }

    #[test]
    fn tier_full_covers_every_section() {
        let (_tmp, store) = built_store();
        let text = render(&store, "M1", Tier::Full).unwrap().unwrap();

        assert!(text.starts_with("MAP: Offline pack format"));
        assert!(text.contains("DESTINATION"));
        assert!(text.contains("NOTES"));
        assert!(text.contains("DECISIONS SO FAR (1)"));
        assert!(text.contains("T-004"));
        assert!(text.contains("FRONTIER (1 takeable now)"));
        assert!(text.contains("T-018"));
        assert!(text.contains("BLOCKED (1)"));
        assert!(text.contains("T-020"));
        assert!(text.contains("NOT YET SPECIFIED (fog)"));
        assert!(text.contains("OUT OF SCOPE (1)"));
    }

    #[test]
    fn tier_frontier_only_is_much_smaller_than_full() {
        let (_tmp, store) = built_store();
        let full = render(&store, "M1", Tier::Full).unwrap().unwrap();
        let frontier_only = render(&store, "M1", Tier::FrontierOnly).unwrap().unwrap();

        assert!(frontier_only.len() < full.len());
        assert!(frontier_only.contains("FRONTIER"));
        assert!(!frontier_only.contains("DECISIONS"));
        assert!(!frontier_only.contains("BLOCKED"));
    }

    #[test]
    fn rendering_the_same_map_twice_produces_byte_identical_output() {
        // Pins the property the module documentation promises: the
        // digest must never change between two round trips of one turn.
        let (_tmp, store) = built_store();
        let first = render(&store, "M1", Tier::Full).unwrap().unwrap();
        let second = render(&store, "M1", Tier::Full).unwrap().unwrap();
        assert_eq!(first, second);
        assert_eq!(first.as_bytes(), second.as_bytes());
    }
}
