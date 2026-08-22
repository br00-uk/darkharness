//! Reading and appending the journal file.

use std::fs::OpenOptions;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use dark_contract::{ErrCode, Error, Result};

use super::event::JournalEvent;

/// The file name of a map's journal, inside its map directory.
const JOURNAL_FILE_NAME: &str = "journal.jsonl";

/// Returns the path to the journal file for `map_id` under `maps_root`.
///
/// `maps_root` is a parameter, not `$DARK_HOME` read from the
/// environment: a caller resolves that path once, outside this crate, and
/// passes the `maps` directory in (typically `$DARK_HOME/maps`).
#[must_use]
pub fn journal_path(maps_root: &Path, map_id: &str) -> PathBuf {
    maps_root.join(map_id).join(JOURNAL_FILE_NAME)
}

/// Appends `event` to the journal for `map_id`, as one line of JSON.
///
/// Creates the map's directory and the journal file when neither exists
/// yet. Flushes the write to storage before returning, so the journal
/// stays the source of truth even across a crash immediately afterwards:
/// at worst, the next reader sees a truncated final line, which
/// [`read_events`] tolerates.
///
/// # Errors
///
/// Returns an error when the map directory cannot be created, when the
/// event cannot be serialised, when the journal file cannot be opened for
/// append, or when the write or the flush fails.
pub fn append(maps_root: &Path, map_id: &str, event: &JournalEvent) -> Result<()> {
    let path = journal_path(maps_root, map_id);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|err| {
            io_failed(format!(
                "cannot create map directory {}: {err}",
                dir.display()
            ))
        })?;
    }

    let mut line = serde_json::to_string(event)
        .map_err(|err| io_failed(format!("cannot serialise journal event: {err}")))?;
    line.push('\n');

    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|err| io_failed(format!("cannot open journal {}: {err}", path.display())))?;
    file.write_all(line.as_bytes()).map_err(|err| {
        io_failed(format!(
            "cannot append to journal {}: {err}",
            path.display()
        ))
    })?;
    file.sync_all()
        .map_err(|err| io_failed(format!("cannot flush journal {}: {err}", path.display())))?;
    Ok(())
}

/// Reads every event from the journal for `map_id`, in file order.
///
/// Reads the file line by line. A blank line is skipped. When the last
/// non-blank line fails to parse as a [`JournalEvent`], this function
/// treats it as a partial write that a crash interrupted mid-line: it
/// skips that one line, writes a warning through `tracing`, and returns
/// every event read before it, rather than failing the whole replay. A
/// malformed line anywhere else is a different problem — real corruption,
/// not a crash in progress — and this function returns an error instead
/// of guessing which line is right.
///
/// Returns an empty vector when the journal file does not exist: a map
/// with no journal has no events yet.
///
/// # Errors
///
/// Returns an error when the file exists but cannot be opened or read, or
/// when a line other than the last non-blank one fails to parse.
pub fn read_events(maps_root: &Path, map_id: &str) -> Result<Vec<JournalEvent>> {
    let path = journal_path(maps_root, map_id);
    let file = match std::fs::File::open(&path) {
        Ok(file) => file,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => {
            return Err(io_failed(format!(
                "cannot open journal {}: {err}",
                path.display()
            )));
        }
    };

    let lines: Vec<String> = BufReader::new(file)
        .lines()
        .collect::<std::io::Result<_>>()
        .map_err(|err| io_failed(format!("cannot read journal {}: {err}", path.display())))?;

    let last_non_blank = lines.iter().rposition(|line| !line.trim().is_empty());

    let mut events = Vec::new();
    for (idx, line) in lines.iter().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<JournalEvent>(line) {
            Ok(event) => events.push(event),
            Err(err) if Some(idx) == last_non_blank => {
                tracing::warn!(
                    journal = %path.display(),
                    line = idx + 1,
                    error = %err,
                    "skipped a truncated final line in the journal; a crash probably \
                     interrupted the write"
                );
                break;
            }
            Err(err) => {
                return Err(io_failed(format!(
                    "journal {} is corrupt at line {}: {err}",
                    path.display(),
                    idx + 1
                )));
            }
        }
    }
    Ok(events)
}

/// Builds an [`Error`] for a journal read or write failure.
fn io_failed(message: String) -> Error {
    Error::new(ErrCode::ToolFailed, message)
}

#[cfg(test)]
mod tests {
    use super::super::event::{MapCreated, MapStatus};
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn sample_event(id: &str) -> JournalEvent {
        JournalEvent::MapCreated(MapCreated {
            id: id.to_owned(),
            name: "Offline pack format".to_owned(),
            destination: "A frozen pack format".to_owned(),
            notes: None,
            created_at: 1_700_000_000_000,
            status: MapStatus::Charting,
        })
    }

    #[test]
    fn append_then_read_round_trips_in_order() {
        let tmp = TempDir::new().expect("tempdir");
        let maps_root = tmp.path();

        append(maps_root, "map-1", &sample_event("01A")).unwrap();
        append(maps_root, "map-1", &sample_event("01B")).unwrap();

        let events = read_events(maps_root, "map-1").unwrap();
        assert_eq!(events, vec![sample_event("01A"), sample_event("01B")]);
    }

    #[test]
    fn a_missing_journal_reads_as_no_events() {
        let tmp = TempDir::new().expect("tempdir");
        let events = read_events(tmp.path(), "no-such-map").unwrap();
        assert!(events.is_empty());
    }

    #[test]
    fn a_truncated_final_line_is_skipped_not_failed() {
        let tmp = TempDir::new().expect("tempdir");
        let maps_root = tmp.path();
        append(maps_root, "map-1", &sample_event("01A")).unwrap();

        // Simulate a crash mid-write: append a partial JSON object with no
        // closing brace and no trailing newline.
        let path = journal_path(maps_root, "map-1");
        let mut existing = fs::read_to_string(&path).unwrap();
        existing.push_str("{\"event\":\"map_created\",\"id\":\"01B\",\"nam");
        fs::write(&path, existing).unwrap();

        let events = read_events(maps_root, "map-1").unwrap();
        assert_eq!(events, vec![sample_event("01A")]);
    }

    #[test]
    fn a_malformed_line_that_is_not_last_is_an_error() {
        let tmp = TempDir::new().expect("tempdir");
        let maps_root = tmp.path();
        let path = journal_path(maps_root, "map-1");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut line1 = serde_json::to_string(&sample_event("01A")).unwrap();
        line1.push('\n');
        let bad_middle_line = "{not json}\n";
        let mut line3 = serde_json::to_string(&sample_event("01C")).unwrap();
        line3.push('\n');
        fs::write(&path, format!("{line1}{bad_middle_line}{line3}")).unwrap();

        let result = read_events(maps_root, "map-1");
        assert!(result.is_err(), "a mid-file corrupt line must fail replay");
    }
}
