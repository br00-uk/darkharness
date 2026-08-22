//! The append-only event log.
//!
//! The journal lives at `<maps_root>/<map-id>/journal.jsonl`, one JSON
//! object per line, and it is the source of truth for a map: the `SQLite`
//! database in [`crate::store`] holds nothing that a journal replay could
//! not rebuild. Commit the journal to Git. Two sessions that both append
//! to the same journal merge cleanly, because `.gitattributes` marks the
//! file `merge=union`: Git keeps every line from both sides instead of
//! asking a person to resolve a text conflict.
//!
//! `maps_root` is always a parameter here, never `$DARK_HOME` read from
//! the environment — a caller resolves that path once, outside this
//! crate, the same way `dark-agentsmd` takes `home` as a parameter rather
//! than reading it itself.

mod event;
mod log;

pub use event::{
    AssetAdded, EdgeAdded, EdgeRemoved, FogAdded, FogGraduated, JournalEvent, MapCreated,
    MapStatus, MapUpdated, ScopeExclusionAdded, TicketCreated, TicketStatus, TicketType,
    TicketUpdated, Timestamp,
};
pub use log::{append, journal_path, read_events};
