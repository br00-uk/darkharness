//! The `SQLite` schema, exactly as the build specification gives it
//! (`PRD.md`, section D1, step 5), with `IF NOT EXISTS` added to each
//! statement so that opening an already-initialised database is a no-op
//! rather than an error.

use dark_contract::Result;
use rusqlite::Connection;

use super::sql_failed;

/// The complete schema, as one batch of `CREATE TABLE` statements.
const CREATE_TABLES: &str = "
CREATE TABLE IF NOT EXISTS maps (
  id TEXT PRIMARY KEY, name TEXT NOT NULL, destination TEXT NOT NULL,
  notes TEXT, created_at INTEGER, updated_at INTEGER,
  status TEXT CHECK(status IN ('charting','active','complete','abandoned'))
);

CREATE TABLE IF NOT EXISTS tickets (
  id TEXT PRIMARY KEY, map_id TEXT NOT NULL REFERENCES maps(id),
  name TEXT NOT NULL,
  question TEXT NOT NULL,
  type TEXT CHECK(type IN ('research','prototype','grilling','task')),
  hitl INTEGER NOT NULL,
  status TEXT CHECK
    (status IN ('open','claimed','resolved','out_of_scope','invalidated')),
  claimed_by TEXT, claimed_at INTEGER,
  resolution TEXT, gist TEXT,
  created_at INTEGER, resolved_at INTEGER,
  ordinal INTEGER NOT NULL,
  axis TEXT,
  tokens_used INTEGER
);

CREATE TABLE IF NOT EXISTS edges (
  blocker TEXT NOT NULL REFERENCES tickets(id),
  blocked TEXT NOT NULL REFERENCES tickets(id),
  PRIMARY KEY (blocker, blocked)
);

CREATE TABLE IF NOT EXISTS fog (
  id TEXT PRIMARY KEY, map_id TEXT NOT NULL,
  patch TEXT NOT NULL, axis TEXT,
  created_at INTEGER, graduated_to TEXT
);

CREATE TABLE IF NOT EXISTS scope_exclusions (
  id TEXT PRIMARY KEY, map_id TEXT NOT NULL,
  gist TEXT NOT NULL, reason TEXT NOT NULL, ticket_id TEXT
);

CREATE TABLE IF NOT EXISTS assets (
  id TEXT PRIMARY KEY, ticket_id TEXT NOT NULL,
  kind TEXT, path TEXT, note TEXT
);
";

/// Creates every table that does not already exist.
///
/// # Errors
///
/// Returns an error when `SQLite` rejects the schema.
pub(super) fn create(conn: &Connection) -> Result<()> {
    conn.execute_batch(CREATE_TABLES)
        .map_err(|err| sql_failed(format!("cannot create the cartograph schema: {err}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_is_idempotent() {
        let conn = Connection::open_in_memory().unwrap();
        create(&conn).unwrap();
        create(&conn).unwrap();
    }

    #[test]
    fn a_bad_map_status_is_rejected_by_the_check_constraint() {
        let conn = Connection::open_in_memory().unwrap();
        create(&conn).unwrap();
        let result = conn.execute(
            "INSERT INTO maps (id, name, destination, status) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params!["m1", "name", "dest", "not_a_real_status"],
        );
        assert!(result.is_err(), "the CHECK constraint should reject this");
    }
}
