//! `dark map`: rebuilds, checks the health of, and exports a map.
//!
//! Every subcommand here opens `<repo_root>/.dark/cartograph.db`
//! ([`Store`]) and, for `rebuild`, replays the journals under
//! `$DARK_HOME/maps` (Section 5.3 of the build specification) into it.
//! Both paths are local: no subcommand in this module reaches the network.
//!
//! Every function below that does real work takes `repo_root` and
//! `maps_root` as parameters rather than reading `crate::repo_root()` or
//! `crate::dark_home()` itself, the same discipline `dark_cartograph`'s own
//! `journal` module uses. [`run_command`] is the one place that resolves
//! those paths for real; a test drives the parameterised functions
//! directly, against a tempdir, and never touches the real `$DARK_HOME`.
//!
//! `list` names every map with its status and ticket counts; `show`
//! renders one map as Markdown, which is the same rendering
//! `dark map export --format markdown` writes, printed rather than
//! returned.

use std::path::Path;

use dark_cartograph::export;
use dark_cartograph::health::{self, Context};
use dark_cartograph::store::Store;

use crate::MapAction;

/// Lists the map identifiers under `maps_root`: the name of every directory
/// directly inside it, sorted.
///
/// Mirrors `dark_cartograph::store::list_map_ids`, which is private to that
/// crate. Returns an empty vector when `maps_root` does not exist yet.
fn list_map_ids(maps_root: &Path) -> anyhow::Result<Vec<String>> {
    if !maps_root.is_dir() {
        return Ok(Vec::new());
    }
    let mut ids = Vec::new();
    for entry in std::fs::read_dir(maps_root)? {
        let entry = entry?;
        if entry.file_type()?.is_dir()
            && let Some(name) = entry.file_name().to_str()
        {
            ids.push(name.to_owned());
        }
    }
    ids.sort();
    Ok(ids)
}

/// Resolves the map identifier `dark map health` should report on: `map`
/// itself when given, otherwise the sole map identifier under `maps_root`
/// when there is exactly one.
///
/// # Errors
///
/// Returns an error naming every map identifier found when `map` is `None`
/// and `maps_root` holds zero or more than one map — `dark map health`
/// cannot guess which one a person means.
fn resolve_map_id(maps_root: &Path, map: Option<String>) -> anyhow::Result<String> {
    if let Some(map) = map {
        return Ok(map);
    }
    let ids = list_map_ids(maps_root)?;
    match ids.as_slice() {
        [only] => Ok(only.clone()),
        [] => anyhow::bail!(
            "no map found under {}. Chart one with /plan, or pass --map <id>.",
            maps_root.display()
        ),
        many => anyhow::bail!(
            "more than one map exists ({}); pass --map <id> to choose one.",
            many.join(", ")
        ),
    }
}

/// Runs `dark map rebuild`: replays every map's journal under `maps_root`
/// into `<repo_root>/.dark/cartograph.db`, from scratch.
///
/// # Errors
///
/// Returns an error when the rebuild fails (see [`Store::rebuild`]) or the
/// rebuilt database cannot be queried for its own summary counts.
fn run_rebuild(repo_root: &Path, maps_root: &Path) -> anyhow::Result<()> {
    let map_ids = list_map_ids(maps_root)?;

    let store = Store::rebuild(repo_root, maps_root).map_err(crate::contract_error)?;
    let (maps, tickets, edges): (i64, i64, i64) = store
        .connection()
        .query_row(
            "SELECT (SELECT COUNT(*) FROM maps), (SELECT COUNT(*) FROM tickets), \
             (SELECT COUNT(*) FROM edges)",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .map_err(|err| anyhow::anyhow!("cannot count the rebuilt database: {err}"))?;

    println!(
        "rebuilt {} from {} journal(s) under {}: {maps} map(s), {tickets} ticket(s), {edges} \
         edge(s).",
        Store::db_path(repo_root).display(),
        map_ids.len(),
        maps_root.display(),
    );
    Ok(())
}

/// Runs `dark map health`: prints ticket sizing quality for one map.
///
/// `known_axes` and `compacted_tickets` are always the empty defaults —
/// nothing in the crates this command can reach tracks either yet (see
/// [`Context`]'s own documentation for exactly why); this command says so
/// in the printed report rather than guessing at either list.
///
/// # Errors
///
/// Returns an error when `map` is ambiguous or absent (see
/// [`resolve_map_id`]), when the database cannot be opened, or when
/// [`health::compute`] fails — for example,
/// [`dark_contract::ErrCode::MapNotFound`] when the identifier names no
/// map.
fn run_health(repo_root: &Path, maps_root: &Path, map: Option<String>) -> anyhow::Result<()> {
    let map_id = resolve_map_id(maps_root, map)?;

    let store = Store::open(repo_root).map_err(crate::contract_error)?;
    let report =
        health::compute(&store, &map_id, &Context::default()).map_err(crate::contract_error)?;

    println!("MAP {map_id}\n");
    print!("{}", health::render(&report));
    println!(
        "\nknown_axes and compacted_tickets are both empty defaults: nothing this command can \
         reach tracks either yet (dark-plan's axis sweep and E5's compaction flag). SILENT AXES \
         and COMPACTION above are therefore always empty until those land."
    );
    Ok(())
}

/// Runs `dark map export --format <fmt>`: prints one map in one of three
/// formats.
///
/// # Errors
///
/// Returns an error when `format` is not `github`, `markdown`, or
/// `mermaid` (see [`export::parse_format`]), when the database cannot be
/// opened, or when [`export::export`] fails — for example,
/// [`dark_contract::ErrCode::MapNotFound`] when `map` names no map.
fn run_export(repo_root: &Path, map: &str, format: &str) -> anyhow::Result<()> {
    let format = export::parse_format(format).map_err(crate::contract_error)?;

    let store = Store::open(repo_root).map_err(crate::contract_error)?;
    let rendered = export::export(&store, map, format).map_err(crate::contract_error)?;
    print!("{rendered}");
    Ok(())
}

/// Runs `dark map <action>`.
///
/// Resolves `repo_root` (the nearest ancestor `.git`) and `maps_root`
/// (`$DARK_HOME/maps`) once, from the real environment, then delegates to
/// the parameterised functions above.
///
/// # Errors
///
/// See [`run_rebuild`], [`run_health`], and [`run_export`]. `list` and
/// `show` are not wired yet and report so.
pub(crate) fn run_command(action: MapAction) -> anyhow::Result<()> {
    match action {
        MapAction::List => run_list(&crate::repo_root()?, &crate::dark_home().join("maps")),
        MapAction::Show { map } => run_show(&crate::repo_root()?, &map),
        MapAction::Rebuild => run_rebuild(&crate::repo_root()?, &crate::dark_home().join("maps")),
        MapAction::Health { map } => {
            run_health(&crate::repo_root()?, &crate::dark_home().join("maps"), map)
        }
        MapAction::Export { map, format } => run_export(&crate::repo_root()?, &map, &format),
    }
}

/// Runs `dark map list`.
///
/// Rebuilds the database from the journals first, the same as
/// [`run_health`]: the journals are the record, and a database that has
/// drifted from them would list something that is not there.
///
/// # Errors
///
/// Returns an error when the journals cannot be read or replayed.
fn run_list(repo_root: &Path, maps_root: &Path) -> anyhow::Result<()> {
    let ids = list_map_ids(maps_root)?;
    if ids.is_empty() {
        println!(
            "no map found under {}. Chart one with /plan.",
            maps_root.display()
        );
        return Ok(());
    }

    let store = Store::rebuild(repo_root, maps_root).map_err(crate::contract_error)?;
    let mut statement = store
        .connection()
        .prepare(
            "SELECT m.id, m.name, m.status, \
             (SELECT COUNT(*) FROM tickets t WHERE t.map_id = m.id), \
             (SELECT COUNT(*) FROM tickets t WHERE t.map_id = m.id AND t.status = 'resolved') \
             FROM maps m ORDER BY m.id",
        )
        .map_err(|err| anyhow::anyhow!("cannot read the maps: {err}"))?;

    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, i64>(4)?,
            ))
        })
        .map_err(|err| anyhow::anyhow!("cannot read the maps: {err}"))?;

    println!("{:<12} {:<12} {:>9}  name", "id", "status", "tickets");
    for row in rows {
        let (id, name, status, tickets, resolved) =
            row.map_err(|err| anyhow::anyhow!("cannot read a map row: {err}"))?;
        println!("{id:<12} {status:<12} {resolved:>4}/{tickets:<4}  {name}");
    }
    Ok(())
}

/// Runs `dark map show <map>`.
///
/// Prints the Markdown rendering: the destination, the notes, and the
/// ticket checklist with its blockers, fog, and scope exclusions. That is
/// the same rendering `dark map export --format markdown` produces, so a
/// person reading a map on screen and a person reading an exported file
/// see the same thing.
///
/// # Errors
///
/// Returns an error when the database cannot be opened, or when no map
/// carries this identifier.
fn run_show(repo_root: &Path, map_id: &str) -> anyhow::Result<()> {
    let store = Store::open(repo_root).map_err(crate::contract_error)?;
    let rendered =
        export::export(&store, map_id, export::Format::Markdown).map_err(crate::contract_error)?;
    print!("{rendered}");
    Ok(())
}

/// Appends a single journal event, for a test fixture.
#[cfg(test)]
fn seed(maps_root: &Path, map_id: &str, event: &dark_cartograph::journal::JournalEvent) {
    dark_cartograph::journal::append(maps_root, map_id, event).unwrap();
}

#[cfg(test)]
mod tests {
    use super::*;
    use dark_cartograph::journal::{
        JournalEvent, MapCreated, MapStatus, TicketCreated, TicketStatus, TicketType,
    };
    use tempfile::TempDir;

    fn seed_map(maps_root: &Path, map_id: &str) {
        seed(
            maps_root,
            map_id,
            &JournalEvent::MapCreated(MapCreated {
                id: map_id.to_owned(),
                name: "Test map".to_owned(),
                destination: "A tested destination".to_owned(),
                notes: None,
                created_at: 1_700_000_000_000,
                status: MapStatus::Active,
            }),
        );
    }

    #[test]
    fn list_map_ids_is_empty_for_a_missing_directory() {
        let tmp = TempDir::new().unwrap();
        let ids = list_map_ids(&tmp.path().join("absent")).unwrap();
        assert!(ids.is_empty());
    }

    #[test]
    fn list_map_ids_finds_every_directory_sorted() {
        let tmp = TempDir::new().unwrap();
        std::fs::create_dir_all(tmp.path().join("M2")).unwrap();
        std::fs::create_dir_all(tmp.path().join("M1")).unwrap();
        let ids = list_map_ids(tmp.path()).unwrap();
        assert_eq!(ids, vec!["M1".to_owned(), "M2".to_owned()]);
    }

    #[test]
    fn resolve_map_id_returns_the_explicit_value_without_touching_disk() {
        let tmp = TempDir::new().unwrap();
        let id = resolve_map_id(&tmp.path().join("absent"), Some("M9".to_owned())).unwrap();
        assert_eq!(id, "M9");
    }

    #[test]
    fn resolve_map_id_falls_back_to_the_sole_map() {
        let tmp = TempDir::new().unwrap();
        seed_map(tmp.path(), "M1");
        let id = resolve_map_id(tmp.path(), None).unwrap();
        assert_eq!(id, "M1");
    }

    #[test]
    fn resolve_map_id_fails_with_no_maps() {
        let tmp = TempDir::new().unwrap();
        let err = resolve_map_id(&tmp.path().join("absent"), None).unwrap_err();
        assert!(err.to_string().contains("no map found"));
    }

    #[test]
    fn resolve_map_id_fails_when_ambiguous() {
        let tmp = TempDir::new().unwrap();
        seed_map(tmp.path(), "M1");
        seed_map(tmp.path(), "M2");
        let err = resolve_map_id(tmp.path(), None).unwrap_err();
        assert!(err.to_string().contains("more than one map"));
        assert!(err.to_string().contains("M1"));
        assert!(err.to_string().contains("M2"));
    }

    #[test]
    fn list_with_no_maps_says_so_rather_than_failing() {
        let tmp = TempDir::new().unwrap();
        let repo_root = tmp.path().join("repo");
        std::fs::create_dir_all(&repo_root).unwrap();

        // An absent maps directory is the ordinary state before anyone
        // has charted a map, not an error.
        run_list(&repo_root, &tmp.path().join("absent")).unwrap();
    }

    #[test]
    fn list_reads_every_map_from_the_journals() {
        let tmp = TempDir::new().unwrap();
        let repo_root = tmp.path().join("repo");
        let maps_root = tmp.path().join("maps");
        std::fs::create_dir_all(&repo_root).unwrap();
        seed_map(&maps_root, "M1");
        seed_map(&maps_root, "M2");

        run_list(&repo_root, &maps_root).unwrap();
    }

    #[test]
    fn show_renders_a_map_that_exists() {
        let tmp = TempDir::new().unwrap();
        let repo_root = tmp.path().join("repo");
        let maps_root = tmp.path().join("maps");
        std::fs::create_dir_all(&repo_root).unwrap();
        seed_map(&maps_root, "M1");
        // `show` reads the database, so it must be built first — the same
        // order a person follows: chart, then read.
        run_rebuild(&repo_root, &maps_root).unwrap();

        run_show(&repo_root, "M1").unwrap();
    }

    #[test]
    fn show_fails_for_a_map_that_does_not_exist() {
        let tmp = TempDir::new().unwrap();
        let repo_root = tmp.path().join("repo");
        let maps_root = tmp.path().join("maps");
        std::fs::create_dir_all(&repo_root).unwrap();
        seed_map(&maps_root, "M1");
        run_rebuild(&repo_root, &maps_root).unwrap();

        let err = run_show(&repo_root, "M9").unwrap_err();
        assert!(
            err.to_string().contains("M9"),
            "the message names the map that was asked for: {err}"
        );
    }

    #[test]
    fn rebuild_reports_summary_counts_from_the_journal() {
        let tmp = TempDir::new().unwrap();
        let repo_root = tmp.path().join("repo");
        let maps_root = tmp.path().join("maps");
        std::fs::create_dir_all(&repo_root).unwrap();
        seed_map(&maps_root, "M1");

        run_rebuild(&repo_root, &maps_root).unwrap();
        assert!(Store::db_path(&repo_root).is_file());
    }

    #[test]
    fn health_reports_on_the_named_map() {
        let tmp = TempDir::new().unwrap();
        let repo_root = tmp.path().join("repo");
        let maps_root = tmp.path().join("maps");
        std::fs::create_dir_all(&repo_root).unwrap();
        seed_map(&maps_root, "M1");
        seed(
            &maps_root,
            "M1",
            &JournalEvent::TicketCreated(TicketCreated {
                id: "T1".to_owned(),
                map_id: "M1".to_owned(),
                name: "Ticket one".to_owned(),
                question: "What?".to_owned(),
                ticket_type: TicketType::Task,
                hitl: false,
                status: TicketStatus::Open,
                created_at: 1_700_000_000_000,
                ordinal: 0,
                axis: None,
                tokens_used: None,
            }),
        );
        Store::rebuild(&repo_root, &maps_root).unwrap();

        run_health(&repo_root, &maps_root, Some("M1".to_owned())).unwrap();
    }

    #[test]
    fn health_reports_map_not_found_for_an_unknown_map() {
        let tmp = TempDir::new().unwrap();
        let repo_root = tmp.path().join("repo");
        let maps_root = tmp.path().join("maps");
        std::fs::create_dir_all(&repo_root).unwrap();
        Store::open(&repo_root).unwrap();

        let err = run_health(&repo_root, &maps_root, Some("no-such-map".to_owned())).unwrap_err();
        assert!(err.to_string().contains("E_MAP_NOT_FOUND"));
    }

    #[test]
    fn export_prints_the_requested_format() {
        let tmp = TempDir::new().unwrap();
        let repo_root = tmp.path().join("repo");
        let maps_root = tmp.path().join("maps");
        std::fs::create_dir_all(&repo_root).unwrap();
        seed_map(&maps_root, "M1");
        Store::rebuild(&repo_root, &maps_root).unwrap();

        run_export(&repo_root, "M1", "markdown").unwrap();
        run_export(&repo_root, "M1", "mermaid").unwrap();
        run_export(&repo_root, "M1", "github").unwrap();
    }

    #[test]
    fn export_rejects_an_unknown_format() {
        let tmp = TempDir::new().unwrap();
        let repo_root = tmp.path().join("repo");
        std::fs::create_dir_all(&repo_root).unwrap();

        let err = run_export(&repo_root, "M1", "csv").unwrap_err();
        assert!(err.to_string().contains("E_TOOL_INVALID_ARGS"));
    }
}
