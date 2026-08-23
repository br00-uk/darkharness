//! Shared newline-delimited JSON parsing for on-disk logs.
//!
//! [`crate::session::transcript`] and [`crate::telemetry::writer`] both
//! keep an append-only `.jsonl` file and both must tolerate the one failure
//! mode a crash leaves behind: a truncated or partly written final line.
//! [`parse_lines`] is the one place that rule lives.

use std::path::Path;

use dark_contract::{ErrCode, Error, Result};
use serde::de::DeserializeOwned;

/// Parses `content` as newline-delimited JSON records of type `T`,
/// tolerating a truncated or partly written final line.
///
/// A crash mid-write can leave one incomplete JSON object at the end of the
/// file; this function drops that line rather than failing the whole read.
/// A line other than the last one that fails to parse is not a crash
/// artefact — it is corruption — and this function returns an error for it.
///
/// # Errors
///
/// Returns an error when a line other than the last one fails to parse,
/// naming `path` and the line number.
pub(crate) fn parse_lines<T: DeserializeOwned>(path: &Path, content: &str) -> Result<Vec<T>> {
    let mut lines: Vec<&str> = content.split('\n').collect();
    if lines.last() == Some(&"") {
        lines.pop();
    }
    let last_index = lines.len().checked_sub(1);

    let mut records = Vec::with_capacity(lines.len());
    for (index, line) in lines.iter().enumerate() {
        if line.is_empty() {
            continue;
        }
        match serde_json::from_str::<T>(line) {
            Ok(record) => records.push(record),
            Err(err) => {
                if Some(index) == last_index {
                    tracing::warn!(
                        path = %path.display(),
                        error = %err,
                        "dropped a truncated final line"
                    );
                    continue;
                }
                return Err(Error::new(
                    ErrCode::ToolFailed,
                    format!(
                        "line {} of {} is not valid JSON: {err}",
                        index + 1,
                        path.display()
                    ),
                ));
            }
        }
    }
    Ok(records)
}
