//! Maps, tickets, blocking edges, and fog.
//!
//! The journal is the source of truth and is committed to Git. The `SQLite`
//! database is derived from the journal by replay and is not committed. See
//! task unit `D1`.

pub mod journal;
pub mod store;
