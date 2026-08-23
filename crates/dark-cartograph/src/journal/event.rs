//! The events that a journal line can hold.
//!
//! Each [`JournalEvent`] variant carries exactly one payload struct. The
//! payload fields mirror the `SQLite` schema (see `crate::store::schema`) so
//! that [`crate::store::Store::apply`] can turn a payload into one SQL
//! statement, but the journal itself has no dependency on `SQLite`: it is
//! plain, serialisable data.
//!
//! A `*Created` event fully populates a new row. A `*Updated` event sets
//! only the fields it names — every `None` field leaves the stored value
//! unchanged, so a caller emits one small event per change instead of
//! re-sending a whole row.

use serde::{Deserialize, Serialize};

/// Milliseconds since the Unix epoch.
///
/// The journal stores every timestamp as an integer, never as a
/// formatted date, so replay never depends on a time zone.
pub type Timestamp = i64;

/// One line of `journal.jsonl`.
///
/// Serialises with an `event` tag naming the variant, so a line reads as
/// a self-describing JSON object, for example
/// `{"event":"map_created","id":"01H...", ...}`. The tag is named `event`,
/// not `kind`, because [`AssetAdded`] already has its own field named
/// `kind` (the asset's kind, matching the `assets.kind` column) — reusing
/// `kind` for the tag would collide with it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum JournalEvent {
    /// Creates a map.
    MapCreated(MapCreated),
    /// Updates fields on an existing map.
    MapUpdated(MapUpdated),
    /// Creates a ticket inside a map.
    TicketCreated(TicketCreated),
    /// Updates fields on an existing ticket.
    TicketUpdated(TicketUpdated),
    /// Adds a blocking edge between two tickets.
    EdgeAdded(EdgeAdded),
    /// Removes a blocking edge between two tickets.
    EdgeRemoved(EdgeRemoved),
    /// Records a patch of fog: a question the map has not yet answered.
    FogAdded(FogAdded),
    /// Marks a fog patch as graduated into a ticket.
    FogGraduated(FogGraduated),
    /// Records that the map excludes something from its scope.
    ScopeExclusionAdded(ScopeExclusionAdded),
    /// Records an asset that a ticket produced.
    AssetAdded(AssetAdded),
}

/// The status of a map. Matches the `maps.status` CHECK constraint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MapStatus {
    /// The map is under construction. Charting has not finished.
    Charting,
    /// The map is open for work. Sessions can claim tickets.
    Active,
    /// Every ticket is resolved, invalidated, or out of scope.
    Complete,
    /// The map stopped before it finished.
    Abandoned,
}

impl MapStatus {
    /// Returns the stable text form. This is the exact string that the
    /// `maps.status` CHECK constraint allows.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Charting => "charting",
            Self::Active => "active",
            Self::Complete => "complete",
            Self::Abandoned => "abandoned",
        }
    }
}

/// The kind of a ticket. Matches the `tickets.type` CHECK constraint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TicketType {
    /// The ticket needs an answer, not a change to the repository.
    Research,
    /// The ticket needs a small, throwaway implementation to test an idea.
    Prototype,
    /// The ticket needs a person to decide something (see `hitl`).
    Grilling,
    /// The ticket needs ordinary implementation work.
    Task,
}

impl TicketType {
    /// Returns the stable text form. This is the exact string that the
    /// `tickets.type` CHECK constraint allows.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Research => "research",
            Self::Prototype => "prototype",
            Self::Grilling => "grilling",
            Self::Task => "task",
        }
    }
}

/// The status of a ticket. Matches the `tickets.status` CHECK constraint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TicketStatus {
    /// No session has claimed the ticket.
    Open,
    /// A session holds the ticket under a lease.
    Claimed,
    /// The ticket has an answer.
    Resolved,
    /// The map excludes the ticket from its scope.
    OutOfScope,
    /// A later decision made the ticket void.
    Invalidated,
}

impl TicketStatus {
    /// Returns the stable text form. This is the exact string that the
    /// `tickets.status` CHECK constraint allows.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Claimed => "claimed",
            Self::Resolved => "resolved",
            Self::OutOfScope => "out_of_scope",
            Self::Invalidated => "invalidated",
        }
    }
}

/// Creates a map. See [`JournalEvent::MapCreated`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MapCreated {
    /// The map's identifier. The caller generates this, typically a ULID.
    pub id: String,
    /// The map's short name.
    pub name: String,
    /// The destination: what the map is charting a way towards.
    pub destination: String,
    /// Free-text notes about the map, when the caller has any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    /// When the map was created.
    pub created_at: Timestamp,
    /// The map's starting status.
    pub status: MapStatus,
}

/// Updates fields on an existing map. See [`JournalEvent::MapUpdated`].
///
/// A `None` field leaves the stored value unchanged.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct MapUpdated {
    /// The map to update.
    pub id: String,
    /// The new name, when it changed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The new destination, when it changed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub destination: Option<String>,
    /// The new notes, when they changed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    /// The new status, when it changed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<MapStatus>,
    /// When this update happened.
    pub updated_at: Timestamp,
}

/// Creates a ticket inside a map. See [`JournalEvent::TicketCreated`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TicketCreated {
    /// The ticket's identifier. The caller generates this.
    pub id: String,
    /// The map that this ticket belongs to.
    pub map_id: String,
    /// The ticket's short name.
    pub name: String,
    /// The question that the ticket answers.
    pub question: String,
    /// The kind of ticket.
    pub ticket_type: TicketType,
    /// `true` when the ticket needs a person (a human in the loop).
    pub hitl: bool,
    /// The ticket's starting status.
    pub status: TicketStatus,
    /// When the ticket was created.
    pub created_at: Timestamp,
    /// The ticket's position among its siblings. Lower sorts first.
    pub ordinal: i64,
    /// The axis that this ticket sits on, when the map has axes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub axis: Option<String>,
    /// Tokens spent on this ticket so far, when the caller tracks that.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tokens_used: Option<i64>,
}

/// Updates fields on an existing ticket. See
/// [`JournalEvent::TicketUpdated`].
///
/// A `None` field leaves the stored value unchanged.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct TicketUpdated {
    /// The ticket to update.
    pub id: String,
    /// The new status, when it changed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<TicketStatus>,
    /// The session that claimed the ticket, when a claim just started.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claimed_by: Option<String>,
    /// When the current claim started, when a claim just started.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claimed_at: Option<Timestamp>,
    /// The resolution text, when the ticket just resolved.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolution: Option<String>,
    /// A short summary of the resolution, when the ticket just resolved.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gist: Option<String>,
    /// When the ticket resolved, when it just resolved.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_at: Option<Timestamp>,
    /// Tokens spent on this ticket so far, when the caller tracks that.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tokens_used: Option<i64>,
    /// Clears `claimed_by` and `claimed_at`.
    ///
    /// Every other field here says "change this to that", and `None` means
    /// "leave it alone", so no field can say "make this empty". A lease
    /// that expires needs exactly that: the ticket goes back to open, and
    /// the claimant who abandoned it must not stay recorded against it.
    /// This flag is the one way to clear a value.
    ///
    /// It defaults to `false`, so a journal written before this field
    /// existed still replays. The journal is the source of truth and is
    /// committed to Git, so an older one must keep working.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub release_claim: bool,
}

/// Adds a blocking edge between two tickets. See
/// [`JournalEvent::EdgeAdded`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EdgeAdded {
    /// The ticket that must resolve first.
    pub blocker: String,
    /// The ticket that cannot start until `blocker` resolves.
    pub blocked: String,
}

/// Removes a blocking edge between two tickets. See
/// [`JournalEvent::EdgeRemoved`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EdgeRemoved {
    /// The blocking ticket named on the edge to remove.
    pub blocker: String,
    /// The blocked ticket named on the edge to remove.
    pub blocked: String,
}

/// Records a patch of fog on a map. See [`JournalEvent::FogAdded`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FogAdded {
    /// The fog patch's identifier. The caller generates this.
    pub id: String,
    /// The map that this fog patch belongs to.
    pub map_id: String,
    /// The text of the unanswered question.
    pub patch: String,
    /// The axis that this fog patch sits on, when the map has axes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub axis: Option<String>,
    /// When the fog patch was recorded.
    pub created_at: Timestamp,
}

/// Marks a fog patch as graduated into a ticket. See
/// [`JournalEvent::FogGraduated`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FogGraduated {
    /// The fog patch that graduated.
    pub id: String,
    /// The ticket that the fog patch became.
    pub graduated_to: String,
}

/// Records a scope exclusion on a map. See
/// [`JournalEvent::ScopeExclusionAdded`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScopeExclusionAdded {
    /// The scope exclusion's identifier. The caller generates this.
    pub id: String,
    /// The map that excludes this.
    pub map_id: String,
    /// A short summary of the excluded thing.
    pub gist: String,
    /// Why the map excludes it.
    pub reason: String,
    /// The ticket that raised this exclusion, when one did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ticket_id: Option<String>,
}

/// Records an asset that a ticket produced. See
/// [`JournalEvent::AssetAdded`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AssetAdded {
    /// The asset's identifier. The caller generates this.
    pub id: String,
    /// The ticket that produced this asset.
    pub ticket_id: String,
    /// The kind of asset, when the caller names one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// The path to the asset, when it lives on disk.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// A free-text note about the asset, when the caller has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn map_created_round_trips_through_json() {
        let event = JournalEvent::MapCreated(MapCreated {
            id: "01MAP".to_owned(),
            name: "Offline pack format".to_owned(),
            destination: "A frozen pack format".to_owned(),
            notes: Some("Domain: Rust".to_owned()),
            created_at: 1_700_000_000_000,
            status: MapStatus::Charting,
        });

        let line = serde_json::to_string(&event).expect("serialises");
        assert!(line.contains("\"event\":\"map_created\""));
        assert!(line.contains("\"status\":\"charting\""));

        let back: JournalEvent = serde_json::from_str(&line).expect("deserialises");
        assert_eq!(back, event);
    }

    #[test]
    fn ticket_updated_omits_absent_fields() {
        let event = JournalEvent::TicketUpdated(TicketUpdated {
            id: "T-001".to_owned(),
            status: Some(TicketStatus::Claimed),
            ..TicketUpdated::default()
        });
        let line = serde_json::to_string(&event).expect("serialises");
        // Only the field that changed appears; the rest stay implicit.
        assert!(line.contains("\"status\":\"claimed\""));
        assert!(!line.contains("claimed_by"));
    }

    #[test]
    fn ticket_status_out_of_scope_matches_the_check_constraint_spelling() {
        assert_eq!(TicketStatus::OutOfScope.as_str(), "out_of_scope");
    }
}
