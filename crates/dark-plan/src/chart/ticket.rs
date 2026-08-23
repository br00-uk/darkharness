//! Ticket-shaped output types that the charting pipeline produces.
//!
//! `dark-plan` does not depend on `dark-cartograph`: that dependency would
//! fail `cargo xtask check-deps`, and more fundamentally the map store is
//! downstream of charting, not upstream of it (`dark-core` depends on both
//! `dark-plan` and `dark-cartograph`; neither depends on the other — see the
//! architecture diagram in `CLAUDE.md`). So the types in this module are
//! charting's own, and they line up **by field name and by shape** with
//! `dark_cartograph::journal::event::TicketCreated`, `EdgeAdded`,
//! `FogAdded`, and `ScopeExclusionAdded` (see
//! `crates/dark-cartograph/src/journal/event.rs`), so that the caller who
//! does hold both crates can build one `JournalEvent` per item here with a
//! field-by-field copy, no translation table needed.
//!
//! One field does not line up, and no dependency-free change fixes it: see
//! the note on [`ChartedTicket::axis`].

use serde::{Deserialize, Serialize};

/// The kind of ticket. Mirrors
/// `dark_cartograph::journal::event::TicketType`, including its
/// `snake_case` wire form, so that a caller can round-trip a value through
/// `serde_json` into the cartograph type without a manual match.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TicketKind {
    /// The ticket needs an answer, not a change to the repository.
    Research,
    /// The ticket needs a small, throwaway implementation to test an idea.
    Prototype,
    /// The ticket needs a person to decide something.
    Grilling,
    /// The ticket needs ordinary implementation work.
    Task,
}

impl TicketKind {
    /// Returns the stable text form, matching
    /// `dark_cartograph::journal::event::TicketType::as_str`.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Research => "research",
            Self::Prototype => "prototype",
            Self::Grilling => "grilling",
            Self::Task => "task",
        }
    }

    /// Returns whether a ticket of this kind needs a human in the loop by
    /// default, following the routing table in task unit `E7`, Do step 5:
    ///
    /// | Type | Human present |
    /// | --- | --- |
    /// | `research` | No |
    /// | `prototype` | Yes |
    /// | `grilling` | Yes |
    /// | `task` | Either |
    ///
    /// `task` reads "Either" in that table, not "Yes": the build
    /// specification leaves the actual choice to the ticket, not the type.
    /// Charting has no further signal to make that choice with, so this
    /// method defaults a `task` ticket to `false` (no human required),
    /// matching Rule 21, which lets `/plan --headless` create tickets
    /// without a human present. A later stage — sizing or wiring, or a
    /// person editing the ticket — may still set `hitl` to `true` on a
    /// `task` ticket; [`ChartedTicket::hitl`] stays a field of its own for
    /// exactly that reason, distinct from the kind.
    #[must_use]
    pub fn default_hitl(self) -> bool {
        matches!(self, Self::Prototype | Self::Grilling)
    }
}

/// A ticket that the charting pipeline drafted.
///
/// Mirrors `dark_cartograph::journal::event::TicketCreated`, minus the
/// fields that only exist once a map store assigns them (`map_id`,
/// `status`, `created_at`). [`ChartedTicket::id`] and
/// [`ChartedTicket::ordinal`] charting fills in itself, at the point Do
/// step 7 of task unit `E1` names: "A ticket needs an identifier before
/// another ticket can reference it," so the identifier exists before stage
/// 7 (wire) resolves a blocker name into an edge.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChartedTicket {
    /// The ticket's identifier. Charting generates this as a ULID, the same
    /// generator the map store's caller uses for a `TicketCreated.id`.
    pub id: String,
    /// The ticket's short name. Stage 4 (extract) produces this; stage 6
    /// (size) may replace one ticket with several, each carrying its own
    /// name.
    pub name: String,
    /// The question that the ticket answers.
    pub question: String,
    /// The kind of ticket.
    pub ticket_type: TicketKind,
    /// `true` when the ticket needs a person. See [`TicketKind::default_hitl`].
    pub hitl: bool,
    /// The ticket's position among its siblings. Lower sorts first. Charting
    /// assigns this in the order it creates tickets, after stage 6 (size),
    /// so a split ticket's parts sort next to each other.
    pub ordinal: i64,
    /// The axis that produced this ticket, when the map has axes.
    ///
    /// **Field name mismatch.** `dark_cartograph`'s `tickets.axis` column
    /// (and `TicketCreated.axis`) holds one axis name as `TEXT`. Task unit
    /// `E5` (`size.rs`, not owned here) can split one ticket into several
    /// during stage 6, and a split ticket may inherit more than one parent
    /// axis when two candidates on different axes merge before splitting —
    /// the build specification does not rule this out, and `E5` is not
    /// built yet to say whether it happens. This field is therefore
    /// `Vec<String>`, not `Option<String>`, so charting never has to decide
    /// how to collapse multiple axes into one before the ticket exists. A
    /// caller writing a `TicketCreated` event must collapse this list to
    /// `Option<String>` itself — join it, or take its first element — since
    /// the store column takes one value. This is the one shape mismatch the
    /// module documentation promises; every other field here is a direct
    /// copy. Flagged in the task report rather than resolved silently,
    /// because resolving it means picking a collapsing rule that belongs
    /// with `E5`'s splitting logic, not with charting's output type.
    pub axis: Vec<String>,
}

/// A blocking edge between two drafted tickets.
///
/// Mirrors `dark_cartograph::journal::event::EdgeAdded` exactly: `blocker`
/// and `blocked` are both [`ChartedTicket::id`] values, resolved from the
/// names stage 7 (wire) answered with, once every ticket has an
/// identifier.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChartedEdge {
    /// The ticket that must resolve first.
    pub blocker: String,
    /// The ticket that cannot start until `blocker` resolves.
    pub blocked: String,
}

/// A patch of fog: a question the map has not yet sharpened into a ticket.
///
/// Mirrors `dark_cartograph::journal::event::FogAdded`, minus `id` and
/// `map_id`, which only exist once a map store assigns them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FogPatch {
    /// The text of the unanswered question.
    pub patch: String,
    /// The axis that this fog patch sits on, when the map has axes.
    pub axis: Option<String>,
}

/// One thing the map excludes from its scope.
///
/// Mirrors `dark_cartograph::journal::event::ScopeExclusionAdded`, minus
/// `id` and `map_id`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScopeExclusion {
    /// A short summary of the excluded thing.
    pub gist: String,
    /// Why the map excludes it.
    pub reason: String,
    /// The ticket that raised this exclusion, when one did. Charting itself
    /// never sets this: Do step 8 of task unit `E1` scopes charting to
    /// producing decisions, never to resolving one, and a scope exclusion
    /// tied to a ticket only exists once a ticket resolution raises it
    /// (task unit `E7`, Do step 8).
    pub ticket_id: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ticket_kind_text_forms_match_the_cartograph_check_constraint_spelling() {
        assert_eq!(TicketKind::Research.as_str(), "research");
        assert_eq!(TicketKind::Prototype.as_str(), "prototype");
        assert_eq!(TicketKind::Grilling.as_str(), "grilling");
        assert_eq!(TicketKind::Task.as_str(), "task");
    }

    #[test]
    fn ticket_kind_serialises_to_the_same_snake_case_the_cartograph_type_uses() {
        let json = serde_json::to_string(&TicketKind::Grilling).unwrap();
        assert_eq!(json, "\"grilling\"");
    }

    #[test]
    fn default_hitl_follows_the_e7_routing_table() {
        assert!(!TicketKind::Research.default_hitl());
        assert!(TicketKind::Prototype.default_hitl());
        assert!(TicketKind::Grilling.default_hitl());
        assert!(!TicketKind::Task.default_hitl());
    }

    #[test]
    fn a_charted_ticket_round_trips_through_json() {
        let ticket = ChartedTicket {
            id: "01T".to_owned(),
            name: "pack staleness policy".to_owned(),
            question: "How does a pack declare its staleness policy?".to_owned(),
            ticket_type: TicketKind::Grilling,
            hitl: true,
            ordinal: 3,
            axis: vec!["lifecycle, migration and backfill".to_owned()],
        };
        let json = serde_json::to_string(&ticket).unwrap();
        let back: ChartedTicket = serde_json::from_str(&json).unwrap();
        assert_eq!(back, ticket);
    }
}
