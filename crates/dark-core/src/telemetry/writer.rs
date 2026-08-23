//! Appends [`TelemetryRecord`] lines to `$DARK_HOME/telemetry.jsonl`
//! (section 5.3 of the build specification), and reads them back for
//! `dark stats`.
//!
//! This module opens exactly one file and never anything else. It has no
//! knowledge of a network, and it never reads the `DARK_HOME` environment
//! variable itself: every function here takes `dark_home` as a parameter,
//! the same discipline [`crate::session::transcript`] uses. The composition
//! root (`dark-cli`) resolves the environment variable and passes the path
//! in.

use std::path::{Path, PathBuf};

use dark_contract::{ErrCode, Error, Result};
use tokio::io::{AsyncWriteExt, BufWriter};

use super::record::TelemetryRecord;

/// The file name telemetry appends to, directly under `$DARK_HOME`
/// (section 5.3). Unlike a transcript, telemetry has no session
/// subdirectory: one file covers every session on the machine.
const FILE_NAME: &str = "telemetry.jsonl";

/// Returns the telemetry path under `dark_home`.
#[must_use]
pub fn telemetry_path(dark_home: &Path) -> PathBuf {
    dark_home.join(FILE_NAME)
}

/// Appends one JSON line per [`TelemetryRecord`] to `telemetry.jsonl`.
///
/// Every call to [`TelemetryWriter::record`] flushes immediately.
/// [`crate::session::transcript::TranscriptWriter`] buffers a turn's token
/// deltas because a turn can stream a thousand of them; telemetry writes
/// once per turn, so there is nothing to gain by batching, and a person
/// tailing the file should see each turn land as it ends.
#[derive(Debug)]
pub struct TelemetryWriter {
    path: PathBuf,
    file: BufWriter<tokio::fs::File>,
}

impl TelemetryWriter {
    /// Opens `telemetry.jsonl` under `dark_home`, creating the directory
    /// and the file when they do not exist, and appending to the file when
    /// it already does.
    ///
    /// # Errors
    ///
    /// Returns an error when `dark_home` cannot be created, or when the
    /// file cannot be opened.
    pub async fn open(dark_home: &Path) -> Result<Self> {
        tokio::fs::create_dir_all(dark_home)
            .await
            .map_err(|err| io_error(dark_home, &err))?;

        let path = telemetry_path(dark_home);
        let file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .await
            .map_err(|err| io_error(&path, &err))?;

        Ok(Self {
            path,
            file: BufWriter::new(file),
        })
    }

    /// Returns the path this writer appends to.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Appends `record` as one line and flushes it to disk.
    ///
    /// # Errors
    ///
    /// Returns an error when the record cannot be serialised, or when the
    /// write or the flush fails.
    pub async fn record(&mut self, record: &TelemetryRecord) -> Result<()> {
        let mut line = serde_json::to_string(record).map_err(|err| {
            Error::new(
                ErrCode::ToolFailed,
                format!(
                    "cannot serialise a telemetry record for {}: {err}",
                    self.path.display()
                ),
            )
        })?;
        line.push('\n');

        self.file
            .write_all(line.as_bytes())
            .await
            .map_err(|err| io_error(&self.path, &err))?;
        self.flush().await
    }

    /// Flushes buffered writes to disk.
    ///
    /// # Errors
    ///
    /// Returns an error when the flush fails.
    pub async fn flush(&mut self) -> Result<()> {
        self.file
            .flush()
            .await
            .map_err(|err| io_error(&self.path, &err))
    }
}

/// Reads every [`TelemetryRecord`] in `$DARK_HOME/telemetry.jsonl`.
///
/// Tolerates a truncated final line the same way
/// [`crate::session::transcript::read_events`] does: a crash mid-write can
/// leave one incomplete JSON object at the end of the file, and this
/// function drops that line rather than failing the whole read. Returns an
/// empty list when the file does not exist yet — a person who has not run
/// a turn is not an error condition for `dark stats`.
///
/// # Errors
///
/// Returns an error when the file exists but cannot be read, or when a
/// line other than the last one fails to parse.
pub async fn read_records(dark_home: &Path) -> Result<Vec<TelemetryRecord>> {
    let path = telemetry_path(dark_home);
    let content = match tokio::fs::read_to_string(&path).await {
        Ok(content) => content,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(io_error(&path, &err)),
    };
    crate::jsonl::parse_lines(&path, &content)
}

/// Maps an I/O failure to the harness error taxonomy.
///
/// The taxonomy has no telemetry-specific domain, so this uses
/// [`ErrCode::ToolFailed`], the general-purpose code for a failure that no
/// domain-specific code covers — the same choice
/// [`crate::session::transcript`] makes for a write or a read it cannot
/// otherwise classify.
fn io_error(path: &Path, err: &std::io::Error) -> Error {
    Error::new(
        ErrCode::ToolFailed,
        format!("cannot access {}: {err}", path.display()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn record(turn_ms: u64) -> TelemetryRecord {
        TelemetryRecord {
            turn_ms,
            prompt_tokens: 100,
            completion_tokens: 20,
            cached_tokens: 80,
            model_loads: 0,
            model_load_ms: 0,
            tool_calls: 0,
            tool_failures: 0,
            frame_overruns: 0,
        }
    }

    #[test]
    fn telemetry_path_sits_directly_under_dark_home() {
        let home = Path::new("/home/person/.darkharness");
        assert_eq!(
            telemetry_path(home),
            PathBuf::from("/home/person/.darkharness/telemetry.jsonl")
        );
    }

    #[tokio::test]
    async fn open_creates_dark_home_and_the_file() {
        let tmp = TempDir::new().unwrap();
        let dark_home = tmp.path().join("nested").join("home");
        let writer = TelemetryWriter::open(&dark_home).await.unwrap();
        assert_eq!(writer.path(), telemetry_path(&dark_home));
        assert!(dark_home.is_dir());
    }

    #[tokio::test]
    async fn record_appends_one_json_line_and_reads_back_the_same_value() {
        let tmp = TempDir::new().unwrap();
        let mut writer = TelemetryWriter::open(tmp.path()).await.unwrap();

        writer.record(&record(1000)).await.unwrap();
        writer.record(&record(2000)).await.unwrap();

        let records = read_records(tmp.path()).await.unwrap();
        assert_eq!(records, vec![record(1000), record(2000)]);
    }

    #[tokio::test]
    async fn record_flushes_immediately() {
        let tmp = TempDir::new().unwrap();
        let mut writer = TelemetryWriter::open(tmp.path()).await.unwrap();
        let path = writer.path().to_path_buf();

        writer.record(&record(500)).await.unwrap();

        // An independent handle sees the write without an explicit flush
        // from the test: `record` must have flushed on its own.
        let on_disk = tokio::fs::read_to_string(&path).await.unwrap();
        assert_eq!(on_disk.lines().count(), 1);
    }

    #[tokio::test]
    async fn read_records_returns_empty_when_the_file_does_not_exist() {
        let tmp = TempDir::new().unwrap();
        let records = read_records(tmp.path()).await.unwrap();
        assert!(records.is_empty());
    }

    #[tokio::test]
    async fn read_records_skips_a_truncated_final_line() {
        let tmp = TempDir::new().unwrap();
        let mut writer = TelemetryWriter::open(tmp.path()).await.unwrap();
        writer.record(&record(1000)).await.unwrap();
        let path = writer.path().to_path_buf();
        drop(writer);

        let mut raw = tokio::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .await
            .unwrap();
        raw.write_all(br#"{"turn_ms":50,"prompt_"#).await.unwrap();
        raw.flush().await.unwrap();
        drop(raw);

        let records = read_records(tmp.path()).await.unwrap();
        assert_eq!(records, vec![record(1000)]);
    }

    #[tokio::test]
    async fn read_records_errors_on_corruption_before_the_final_line() {
        let tmp = TempDir::new().unwrap();
        let path = telemetry_path(tmp.path());
        let good_line = serde_json::to_string(&record(1)).unwrap();
        tokio::fs::write(&path, format!("not json at all\n{good_line}\n"))
            .await
            .unwrap();

        let err = read_records(tmp.path()).await.unwrap_err();
        assert_eq!(err.code, ErrCode::ToolFailed);
    }
}
