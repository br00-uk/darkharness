//! The derived `SQLite` database.
//!
//! [`Store`] wraps `<repo_root>/.dark/cartograph.db`. Nothing in this
//! database is ever the only copy of a fact: every row comes from
//! replaying a [`crate::journal::JournalEvent`], so the database is safe
//! to delete and rebuild at any time with [`Store::rebuild`] (`dark map
//! rebuild` calls this). Do not commit this file — `.gitignore` already
//! excludes it.

mod cycle;
mod schema;

use std::path::{Path, PathBuf};

use dark_contract::{ErrCode, Error, Result};
use rusqlite::{Connection, params};

use crate::journal::{self, EdgeAdded, JournalEvent, MapStatus, TicketStatus};

/// An open connection to `<repo_root>/.dark/cartograph.db`, with the
/// schema already created.
pub struct Store {
    /// The underlying `SQLite` connection.
    conn: Connection,
}

impl Store {
    /// Returns the path to the database file for `repo_root`, without
    /// opening it.
    #[must_use]
    pub fn db_path(repo_root: &Path) -> PathBuf {
        repo_root.join(".dark").join("cartograph.db")
    }

    /// Opens the database at `<repo_root>/.dark/cartograph.db`, creating
    /// the parent directory, the file, and the schema when none exists
    /// yet.
    ///
    /// `repo_root` is a parameter, not read from the environment, so a
    /// caller can open a fixture repository in a test.
    ///
    /// # Errors
    ///
    /// Returns an error when the parent directory cannot be created, or
    /// when `SQLite` cannot open the file or create the schema.
    pub fn open(repo_root: &Path) -> Result<Self> {
        let path = Self::db_path(repo_root);
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)
                .map_err(|err| sql_failed(format!("cannot create {}: {err}", dir.display())))?;
        }
        let conn = Connection::open(&path)
            .map_err(|err| sql_failed(format!("cannot open {}: {err}", path.display())))?;
        schema::create(&conn)?;
        Ok(Self { conn })
    }

    /// Rebuilds the database from scratch by replaying the journal of
    /// every map under `maps_root`.
    ///
    /// Deletes any existing database file at `<repo_root>/.dark/cartograph.db`
    /// first, so the result depends only on the journals on disk, never on
    /// whatever the file held before. This is the entry point that `dark
    /// map rebuild` calls.
    ///
    /// # Errors
    ///
    /// Returns an error when the existing file cannot be removed, when
    /// the new database cannot be opened, when `maps_root` cannot be
    /// listed, or when a journal fails to replay (see
    /// [`journal::read_events`]) or apply (see [`Store::apply`]).
    pub fn rebuild(repo_root: &Path, maps_root: &Path) -> Result<Self> {
        let path = Self::db_path(repo_root);
        if path.exists() {
            std::fs::remove_file(&path)
                .map_err(|err| sql_failed(format!("cannot remove {}: {err}", path.display())))?;
        }

        let mut store = Self::open(repo_root)?;
        for map_id in list_map_ids(maps_root)? {
            for event in journal::read_events(maps_root, &map_id)? {
                store.apply(&event)?;
            }
        }
        Ok(store)
    }

    /// Returns the underlying `SQLite` connection, for queries that this
    /// crate's own callers need and this type does not otherwise expose —
    /// for example the frontier query in task unit `D2`.
    #[must_use]
    pub fn connection(&self) -> &Connection {
        &self.conn
    }

    /// Applies one journal event to the database, in place.
    ///
    /// Replay ([`Store::rebuild`]) calls this once per event, in file
    /// order, and that is the only way this database's content is ever
    /// decided. A live write path should call this once, immediately
    /// after [`journal::append`] durably records the same event, so the
    /// database never claims a fact that the journal does not also hold.
    ///
    /// # Errors
    ///
    /// Returns an error when the underlying SQL statement fails, for
    /// example because a row it updates does not exist.
    pub fn apply(&mut self, event: &JournalEvent) -> Result<()> {
        match event {
            JournalEvent::MapCreated(e) => self.conn.execute(
                "INSERT INTO maps (id, name, destination, notes, created_at, updated_at, status)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?5, ?6)",
                params![
                    e.id,
                    e.name,
                    e.destination,
                    e.notes,
                    e.created_at,
                    e.status.as_str()
                ],
            ),
            JournalEvent::MapUpdated(e) => self.conn.execute(
                "UPDATE maps SET
                   name = COALESCE(?2, name),
                   destination = COALESCE(?3, destination),
                   notes = COALESCE(?4, notes),
                   status = COALESCE(?5, status),
                   updated_at = ?6
                 WHERE id = ?1",
                params![
                    e.id,
                    e.name,
                    e.destination,
                    e.notes,
                    e.status.map(MapStatus::as_str),
                    e.updated_at,
                ],
            ),
            JournalEvent::TicketCreated(e) => self.conn.execute(
                "INSERT INTO tickets
                   (id, map_id, name, question, type, hitl, status, created_at, ordinal, axis,
                    tokens_used)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                params![
                    e.id,
                    e.map_id,
                    e.name,
                    e.question,
                    e.ticket_type.as_str(),
                    e.hitl,
                    e.status.as_str(),
                    e.created_at,
                    e.ordinal,
                    e.axis,
                    e.tokens_used,
                ],
            ),
            JournalEvent::TicketUpdated(e) => self.conn.execute(
                "UPDATE tickets SET
                   status = COALESCE(?2, status),
                   claimed_by = COALESCE(?3, claimed_by),
                   claimed_at = COALESCE(?4, claimed_at),
                   resolution = COALESCE(?5, resolution),
                   gist = COALESCE(?6, gist),
                   resolved_at = COALESCE(?7, resolved_at),
                   tokens_used = COALESCE(?8, tokens_used)
                 WHERE id = ?1",
                params![
                    e.id,
                    e.status.map(TicketStatus::as_str),
                    e.claimed_by,
                    e.claimed_at,
                    e.resolution,
                    e.gist,
                    e.resolved_at,
                    e.tokens_used,
                ],
            ),
            JournalEvent::EdgeAdded(e) => self.conn.execute(
                "INSERT OR IGNORE INTO edges (blocker, blocked) VALUES (?1, ?2)",
                params![e.blocker, e.blocked],
            ),
            JournalEvent::EdgeRemoved(e) => self.conn.execute(
                "DELETE FROM edges WHERE blocker = ?1 AND blocked = ?2",
                params![e.blocker, e.blocked],
            ),
            JournalEvent::FogAdded(e) => self.conn.execute(
                "INSERT INTO fog (id, map_id, patch, axis, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![e.id, e.map_id, e.patch, e.axis, e.created_at],
            ),
            JournalEvent::FogGraduated(e) => self.conn.execute(
                "UPDATE fog SET graduated_to = ?2 WHERE id = ?1",
                params![e.id, e.graduated_to],
            ),
            JournalEvent::ScopeExclusionAdded(e) => self.conn.execute(
                "INSERT INTO scope_exclusions (id, map_id, gist, reason, ticket_id)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![e.id, e.map_id, e.gist, e.reason, e.ticket_id],
            ),
            JournalEvent::AssetAdded(e) => self.conn.execute(
                "INSERT INTO assets (id, ticket_id, kind, path, note) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![e.id, e.ticket_id, e.kind, e.path, e.note],
            ),
        }
        .map_err(|err| sql_failed(format!("cannot apply {event:?}: {err}")))?;
        Ok(())
    }

    /// Adds a blocking edge from `blocker` to `blocked`, after checking
    /// that doing so cannot create a cycle.
    ///
    /// Checks reachability first, over the edges already in this
    /// database. When `blocked` can already reach `blocker`, adding this
    /// edge would close a cycle: every ticket on it would then wait,
    /// transitively, on itself, and the frontier could never take any of
    /// them. In that case this function returns [`ErrCode::MapCycle`]
    /// naming the full cycle, in order, and neither the journal nor the
    /// `edges` table changes.
    ///
    /// On success, appends a [`JournalEvent::EdgeAdded`] event to the
    /// journal for `map_id` under `maps_root` — durably, before this
    /// database changes — then applies that same event here.
    ///
    /// # Errors
    ///
    /// Returns [`ErrCode::MapCycle`] when the edge would create a cycle.
    /// Returns an error when the cycle check, the journal append, or the
    /// SQL insert fails.
    // `blocker` and `blocked` are the schema's own column names (PRD.md,
    // section D1, step 5): renaming one to satisfy `similar_names` would
    // make the code harder to match against the schema it implements, not
    // easier to read.
    #[allow(clippy::similar_names)]
    pub fn add_edge(
        &mut self,
        maps_root: &Path,
        map_id: &str,
        blocker: &str,
        blocked: &str,
    ) -> Result<()> {
        if let Some(path) = cycle::find_cycle(&self.conn, blocker, blocked)? {
            let rendered = path.join(" -> ");
            return Err(Error::new(
                ErrCode::MapCycle,
                format!("edge {blocker} -> {blocked} closes a cycle: {rendered}"),
            ));
        }

        let event = JournalEvent::EdgeAdded(EdgeAdded {
            blocker: blocker.to_owned(),
            blocked: blocked.to_owned(),
        });
        journal::append(maps_root, map_id, &event)?;
        self.apply(&event)
    }
}

/// Lists the map identifiers under `maps_root`: the name of every
/// directory directly inside it, sorted by byte value so that a rebuild
/// replays maps in the same order every time.
///
/// Returns an empty vector when `maps_root` does not exist yet: a fresh
/// `$DARK_HOME` has charted no maps.
fn list_map_ids(maps_root: &Path) -> Result<Vec<String>> {
    if !maps_root.is_dir() {
        return Ok(Vec::new());
    }
    let mut ids = Vec::new();
    let entries = std::fs::read_dir(maps_root)
        .map_err(|err| sql_failed(format!("cannot list {}: {err}", maps_root.display())))?;
    for entry in entries {
        let entry = entry
            .map_err(|err| sql_failed(format!("cannot list {}: {err}", maps_root.display())))?;
        let is_dir = entry
            .file_type()
            .map_err(|err| sql_failed(format!("cannot read {}: {err}", entry.path().display())))?
            .is_dir();
        if is_dir {
            if let Some(name) = entry.file_name().to_str() {
                ids.push(name.to_owned());
            }
        }
    }
    ids.sort();
    Ok(ids)
}

/// Builds an [`Error`] for a database failure that no more specific code
/// covers.
fn sql_failed(message: String) -> Error {
    Error::new(ErrCode::ToolFailed, message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::journal::{MapCreated, MapStatus, TicketCreated, TicketStatus, TicketType};
    use tempfile::TempDir;

    fn open_test_store() -> (TempDir, Store) {
        let tmp = TempDir::new().expect("tempdir");
        let store = Store::open(tmp.path()).expect("open store");
        (tmp, store)
    }

    fn create_map(store: &mut Store, id: &str) {
        store
            .apply(&JournalEvent::MapCreated(MapCreated {
                id: id.to_owned(),
                name: "Test map".to_owned(),
                destination: "A tested destination".to_owned(),
                notes: None,
                created_at: 1_700_000_000_000,
                status: MapStatus::Active,
            }))
            .unwrap();
    }

    fn create_ticket(store: &mut Store, map_id: &str, id: &str, ordinal: i64) {
        store
            .apply(&JournalEvent::TicketCreated(TicketCreated {
                id: id.to_owned(),
                map_id: map_id.to_owned(),
                name: id.to_owned(),
                question: format!("What does {id} answer?"),
                ticket_type: TicketType::Task,
                hitl: false,
                status: TicketStatus::Open,
                created_at: 1_700_000_000_000,
                ordinal,
                axis: None,
                tokens_used: None,
            }))
            .unwrap();
    }

    #[test]
    fn open_creates_the_schema() {
        let (_tmp, store) = open_test_store();
        let count: i64 = store
            .connection()
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = 'tickets'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn apply_map_created_inserts_a_row() {
        let (_tmp, mut store) = open_test_store();
        create_map(&mut store, "M1");
        let name: String = store
            .connection()
            .query_row("SELECT name FROM maps WHERE id = 'M1'", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(name, "Test map");
    }

    #[test]
    fn apply_map_updated_changes_only_the_named_fields() {
        let (_tmp, mut store) = open_test_store();
        create_map(&mut store, "M1");
        store
            .apply(&JournalEvent::MapUpdated(crate::journal::MapUpdated {
                id: "M1".to_owned(),
                status: Some(MapStatus::Complete),
                updated_at: 1_700_000_001_000,
                ..crate::journal::MapUpdated::default()
            }))
            .unwrap();

        let (name, status): (String, String) = store
            .connection()
            .query_row("SELECT name, status FROM maps WHERE id = 'M1'", [], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .unwrap();
        assert_eq!(
            name, "Test map",
            "name must survive an update that did not name it"
        );
        assert_eq!(status, "complete");
    }

    #[test]
    fn add_edge_accepts_a_safe_edge_and_records_it_in_the_journal() {
        let tmp = TempDir::new().expect("tempdir");
        let repo_root = tmp.path().join("repo");
        let maps_root = tmp.path().join("maps");
        let mut store = Store::open(&repo_root).unwrap();
        create_map(&mut store, "M1");
        create_ticket(&mut store, "M1", "T1", 0);
        create_ticket(&mut store, "M1", "T2", 1);

        store.add_edge(&maps_root, "M1", "T1", "T2").unwrap();

        let count: i64 = store
            .connection()
            .query_row("SELECT count(*) FROM edges", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 1);

        let events = journal::read_events(&maps_root, "M1").unwrap();
        assert!(
            events
                .iter()
                .any(|e| matches!(e, JournalEvent::EdgeAdded(edge) if edge.blocker == "T1" && edge.blocked == "T2"))
        );
    }

    #[test]
    fn add_edge_rejects_a_five_node_cycle_and_names_the_path() {
        let tmp = TempDir::new().expect("tempdir");
        let repo_root = tmp.path().join("repo");
        let maps_root = tmp.path().join("maps");
        let mut store = Store::open(&repo_root).unwrap();
        create_map(&mut store, "M1");
        for (ordinal, id) in ["T1", "T2", "T3", "T4", "T5"].iter().enumerate() {
            create_ticket(&mut store, "M1", id, i64::try_from(ordinal).unwrap());
        }
        store.add_edge(&maps_root, "M1", "T1", "T2").unwrap();
        store.add_edge(&maps_root, "M1", "T2", "T3").unwrap();
        store.add_edge(&maps_root, "M1", "T3", "T4").unwrap();
        store.add_edge(&maps_root, "M1", "T4", "T5").unwrap();

        // Closing T5 -> T1 completes a five-ticket cycle.
        let err = store.add_edge(&maps_root, "M1", "T5", "T1").unwrap_err();
        assert_eq!(err.code, ErrCode::MapCycle);
        for id in ["T1", "T2", "T3", "T4", "T5"] {
            assert!(
                err.message.contains(id),
                "message should name {id}: {}",
                err.message
            );
        }
        assert_eq!(
            err.remedy.as_deref(),
            Some("Remove one edge on the reported path.")
        );

        // The rejected edge must not reach the database or the journal.
        let count: i64 = store
            .connection()
            .query_row(
                "SELECT count(*) FROM edges WHERE blocker = 'T5' AND blocked = 'T1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 0, "a rejected edge must not be inserted");

        let events = journal::read_events(&maps_root, "M1").unwrap();
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, JournalEvent::EdgeAdded(edge) if edge.blocker == "T5" && edge.blocked == "T1")),
            "a rejected edge must not be journalled"
        );
    }

    #[test]
    fn rebuild_reproduces_the_database_from_the_journal() {
        let tmp = TempDir::new().expect("tempdir");
        let repo_root = tmp.path().join("repo");
        let maps_root = tmp.path().join("maps");

        journal::append(
            &maps_root,
            "M1",
            &JournalEvent::MapCreated(MapCreated {
                id: "M1".to_owned(),
                name: "Rebuilt map".to_owned(),
                destination: "Destination".to_owned(),
                notes: None,
                created_at: 1_700_000_000_000,
                status: MapStatus::Active,
            }),
        )
        .unwrap();
        journal::append(
            &maps_root,
            "M1",
            &JournalEvent::TicketCreated(TicketCreated {
                id: "T1".to_owned(),
                map_id: "M1".to_owned(),
                name: "T1".to_owned(),
                question: "What?".to_owned(),
                ticket_type: TicketType::Research,
                hitl: false,
                status: TicketStatus::Open,
                created_at: 1_700_000_000_000,
                ordinal: 0,
                axis: None,
                tokens_used: None,
            }),
        )
        .unwrap();

        let store = Store::rebuild(&repo_root, &maps_root).unwrap();
        let (map_name, ticket_name): (String, String) = store
            .connection()
            .query_row(
                "SELECT maps.name, tickets.name FROM maps JOIN tickets ON tickets.map_id = maps.id
                 WHERE maps.id = 'M1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(map_name, "Rebuilt map");
        assert_eq!(ticket_name, "T1");
    }

    #[test]
    fn rebuild_deletes_stale_content_that_the_journal_no_longer_supports() {
        let tmp = TempDir::new().expect("tempdir");
        let repo_root = tmp.path().join("repo");
        let maps_root = tmp.path().join("maps");

        // First rebuild sees one map.
        journal::append(
            &maps_root,
            "M1",
            &JournalEvent::MapCreated(MapCreated {
                id: "M1".to_owned(),
                name: "First".to_owned(),
                destination: "Destination".to_owned(),
                notes: None,
                created_at: 1_700_000_000_000,
                status: MapStatus::Active,
            }),
        )
        .unwrap();
        Store::rebuild(&repo_root, &maps_root).unwrap();

        // Overwrite the journal directly (as if history were rewritten)
        // and rebuild again: the stale row must not survive.
        std::fs::write(journal::journal_path(&maps_root, "M1"), "").unwrap();
        let store = Store::rebuild(&repo_root, &maps_root).unwrap();
        let count: i64 = store
            .connection()
            .query_row("SELECT count(*) FROM maps", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 0);
    }
}
