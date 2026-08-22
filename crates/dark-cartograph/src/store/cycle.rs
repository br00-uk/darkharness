//! Cycle detection over the `edges` table.
//!
//! A blocking edge points from the ticket that must resolve first (the
//! blocker) to the ticket that waits (the blocked ticket), so the edges
//! already in the database describe a directed graph of "must happen
//! before" relationships. Adding one more edge from `blocker` to
//! `blocked` is only safe when no path already runs from `blocked` back
//! to `blocker` — such a path, plus the new edge, would close a loop that
//! can never resolve, because every ticket on it waits, transitively, on
//! itself.

use std::collections::{HashMap, HashSet, VecDeque};

use dark_contract::Result;
use rusqlite::Connection;

use super::sql_failed;

/// Returns the cycle that adding an edge from `blocker` to `blocked`
/// would close, or `None` when the edge is safe to add.
///
/// The returned path lists every ticket on the cycle, in the order that
/// work would have to happen, starting and ending at `blocker` — for
/// example `[blocker, a, b, blocker]` for a four-ticket cycle. Reading it
/// left to right names exactly why the edge is unsafe: `blocker` waits on
/// `a`, which waits on `b`, which waits on `blocker` again.
///
/// # Errors
///
/// Returns an error when the `edges` table cannot be read.
// `blocker` and `blocked` are the schema's own column names (PRD.md,
// section D1, step 5): renaming one to satisfy `similar_names` would make
// the code harder to match against the schema it implements, not easier
// to read.
#[allow(clippy::similar_names)]
pub(super) fn find_cycle(
    conn: &Connection,
    blocker: &str,
    blocked: &str,
) -> Result<Option<Vec<String>>> {
    if blocker == blocked {
        return Ok(Some(vec![blocker.to_owned(), blocked.to_owned()]));
    }

    let adjacency = load_adjacency(conn)?;

    // Breadth-first search from `blocked`, looking for a way back to
    // `blocker`. Breadth-first finds the shortest such path, which keeps
    // the reported cycle as small as possible.
    let mut queue: VecDeque<String> = VecDeque::new();
    let mut visited: HashSet<String> = HashSet::new();
    let mut came_from: HashMap<String, String> = HashMap::new();

    queue.push_back(blocked.to_owned());
    visited.insert(blocked.to_owned());

    while let Some(node) = queue.pop_front() {
        if node == blocker {
            let mut path = vec![node.clone()];
            let mut cursor = node;
            while let Some(prev) = came_from.get(&cursor) {
                path.push(prev.clone());
                cursor = prev.clone();
            }
            path.reverse(); // now reads blocked -> ... -> blocker

            let mut full_cycle = vec![blocker.to_owned()];
            full_cycle.extend(path); // blocker -> blocked -> ... -> blocker
            return Ok(Some(full_cycle));
        }

        if let Some(next_nodes) = adjacency.get(&node) {
            for next in next_nodes {
                if visited.insert(next.clone()) {
                    came_from.insert(next.clone(), node.clone());
                    queue.push_back(next.clone());
                }
            }
        }
    }

    Ok(None)
}

/// Loads the `edges` table into an adjacency list keyed by `blocker`.
fn load_adjacency(conn: &Connection) -> Result<HashMap<String, Vec<String>>> {
    let mut stmt = conn
        .prepare("SELECT blocker, blocked FROM edges")
        .map_err(|err| sql_failed(format!("cannot read the edges table: {err}")))?;
    let rows = stmt
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(|err| sql_failed(format!("cannot read the edges table: {err}")))?;

    let mut adjacency: HashMap<String, Vec<String>> = HashMap::new();
    for row in rows {
        let (from, to) =
            row.map_err(|err| sql_failed(format!("cannot read an edge row: {err}")))?;
        adjacency.entry(from).or_default().push(to);
    }
    Ok(adjacency)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conn_with_edges(pairs: &[(&str, &str)]) -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE edges (blocker TEXT, blocked TEXT, PRIMARY KEY (blocker, blocked));",
        )
        .unwrap();
        for (blocker, blocked) in pairs {
            conn.execute(
                "INSERT INTO edges (blocker, blocked) VALUES (?1, ?2)",
                rusqlite::params![blocker, blocked],
            )
            .unwrap();
        }
        conn
    }

    #[test]
    fn no_existing_path_means_no_cycle() {
        let conn = conn_with_edges(&[("A", "B")]);
        assert_eq!(find_cycle(&conn, "C", "D").unwrap(), None);
    }

    #[test]
    fn a_direct_reversal_is_a_cycle() {
        let conn = conn_with_edges(&[("A", "B")]);
        let path = find_cycle(&conn, "B", "A").unwrap().unwrap();
        assert_eq!(path, vec!["B", "A", "B"]);
    }

    #[test]
    fn a_self_edge_is_a_cycle() {
        let conn = conn_with_edges(&[]);
        let path = find_cycle(&conn, "A", "A").unwrap().unwrap();
        assert_eq!(path, vec!["A", "A"]);
    }

    #[test]
    fn a_five_node_cycle_is_found_and_reported_in_order() {
        // T1 -> T2 -> T3 -> T4 -> T5 already exists. Closing T5 -> T1
        // would make a five-ticket cycle.
        let conn = conn_with_edges(&[("T1", "T2"), ("T2", "T3"), ("T3", "T4"), ("T4", "T5")]);
        let path = find_cycle(&conn, "T5", "T1").unwrap().unwrap();
        assert_eq!(path, vec!["T5", "T1", "T2", "T3", "T4", "T5"]);
    }
}
