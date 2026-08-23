//! Checkpointing, so a charting run killed mid-way can resume without
//! starting over.
//!
//! Task unit `E1`, Do step 4 and step 5: write a checkpoint to the journal
//! after each stage, and support `dark map chart --resume <map-id>
//! --from-stage <n>`. "One bad generation in twelve is normal on a local
//! model. A full restart makes the feature unusable."
//!
//! `dark-plan` does not depend on `dark-cartograph`, so "the journal" here
//! is not `dark_cartograph::journal` — it is [`CheckpointStore`], a small
//! trait this module owns. [`FileCheckpointStore`] is the reference
//! implementation: one JSON-lines file per map, appended to, the same
//! append-only shape `D1` gives the real map journal. A caller wiring
//! charting into the harness is free to implement [`CheckpointStore`] over
//! the cartograph journal instead — nothing here requires the file-backed
//! version.

use std::fs::OpenOptions;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use dark_contract::{ErrCode, Error, Result};
use serde::{Deserialize, Serialize};

/// One of the seven charting stages, in run order.
///
/// Matches the stage table in task unit `E1`, Do step 1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    /// Stage 1: settle what the map is charting a way towards.
    Destination,
    /// Stage 2: read the repository. Uses no model.
    Seed,
    /// Stage 3: enumerate the axes, one turn each.
    AxisSweep,
    /// Stage 4: turn the axis answers into named candidates.
    Extract,
    /// Stage 5: test each candidate for fog.
    Sharpen,
    /// Stage 6: split a candidate that does not fit one session.
    Size,
    /// Stage 7: wire the blocking edges.
    Wire,
}

impl Stage {
    /// The stages, in run order. Resuming "from stage `n`" resumes from the
    /// `n`th entry, one-indexed, matching the Do step 1 table.
    pub const ORDER: [Stage; 7] = [
        Stage::Destination,
        Stage::Seed,
        Stage::AxisSweep,
        Stage::Extract,
        Stage::Sharpen,
        Stage::Size,
        Stage::Wire,
    ];

    /// Returns the one-indexed stage number the build specification's table
    /// uses, for example `5` for [`Stage::Sharpen`].
    #[must_use]
    pub fn number(self) -> u8 {
        #[expect(
            clippy::cast_possible_truncation,
            reason = "seven stages never approach u8::MAX"
        )]
        let position = Self::ORDER
            .iter()
            .position(|&stage| stage == self)
            .unwrap_or(0) as u8;
        position + 1
    }

    /// Looks up a stage by its one-indexed number.
    #[must_use]
    pub fn from_number(number: u8) -> Option<Self> {
        number
            .checked_sub(1)
            .and_then(|index| Self::ORDER.get(index as usize).copied())
    }

    /// Returns the stage's lowercase name, matching its `serde` wire form.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Destination => "destination",
            Self::Seed => "seed",
            Self::AxisSweep => "axis_sweep",
            Self::Extract => "extract",
            Self::Sharpen => "sharpen",
            Self::Size => "size",
            Self::Wire => "wire",
        }
    }
}

/// One recorded checkpoint.
///
/// `payload` holds the stage's output, serialised with `serde_json`, so
/// [`CheckpointStore::load`] can hand it back to a resumed run without
/// re-running the model for a stage that already finished.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Checkpoint {
    /// The map this checkpoint belongs to.
    pub map_id: String,
    /// The stage that finished.
    pub stage: Stage,
    /// When the stage finished, in milliseconds since the Unix epoch.
    pub recorded_at: i64,
    /// The stage's output, opaque to the checkpoint store itself.
    pub payload: serde_json::Value,
}

/// Where the charting pipeline writes and reads checkpoints.
///
/// See the module documentation for why this is a `dark-plan`-owned trait
/// rather than `dark_cartograph::journal`.
pub trait CheckpointStore: Send + Sync {
    /// Records that `checkpoint`'s stage finished.
    ///
    /// # Errors
    ///
    /// Returns an error when the checkpoint cannot be recorded.
    fn record(&self, checkpoint: &Checkpoint) -> Result<()>;

    /// Returns every checkpoint recorded for `map_id`, in the order they
    /// were recorded.
    ///
    /// # Errors
    ///
    /// Returns an error when the store cannot be read.
    fn load(&self, map_id: &str) -> Result<Vec<Checkpoint>>;
}

/// Builds the [`ErrCode::EngineGenerate`] error a checkpoint I/O failure
/// reports.
///
/// A checkpoint failure is not a model failure, but the taxonomy (see
/// `crates/dark-contract/src/error.rs`) has no `E_MAP_` code that fits a
/// storage failure that is not a cycle or an empty frontier, and `dark-plan`
/// does not own `dark-contract`'s error module to add one. `EngineGenerate`
/// is the closest existing code for "the charting pipeline could not
/// continue for a reason outside the model," and the message names the
/// real cause, so a person is not misled about where to look.
fn store_failed(message: impl Into<String>) -> Error {
    Error::new(ErrCode::EngineGenerate, message.into())
        .with_remedy("Check that the checkpoint file's directory exists and is writable.")
}

/// A [`CheckpointStore`] backed by one JSON-lines file per map.
///
/// Appends one line per checkpoint, mirroring the append-only shape of the
/// real map journal (`D1`). A lock guards concurrent writers within one
/// process; two processes writing the same file rely on the operating
/// system's append semantics, the same assumption `D1`'s `journal.jsonl`
/// makes.
#[derive(Debug)]
pub struct FileCheckpointStore {
    path: PathBuf,
    write_lock: Mutex<()>,
}

impl FileCheckpointStore {
    /// Opens a checkpoint store at `path`. The file need not exist yet:
    /// [`CheckpointStore::record`] creates it on first use.
    #[must_use]
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            write_lock: Mutex::new(()),
        }
    }

    /// Returns the file this store reads and writes.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl CheckpointStore for FileCheckpointStore {
    fn record(&self, checkpoint: &Checkpoint) -> Result<()> {
        let line = serde_json::to_string(checkpoint)
            .map_err(|err| store_failed(format!("cannot serialise a checkpoint: {err}")))?;

        let _guard = self
            .write_lock
            .lock()
            .map_err(|_| store_failed("the checkpoint write lock is poisoned"))?;

        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .map_err(|err| {
                store_failed(format!(
                    "cannot open {} for append: {err}",
                    self.path.display()
                ))
            })?;

        writeln!(file, "{line}")
            .map_err(|err| store_failed(format!("cannot append to {}: {err}", self.path.display())))
    }

    fn load(&self, map_id: &str) -> Result<Vec<Checkpoint>> {
        let Ok(file) = std::fs::File::open(&self.path) else {
            // No file yet means no checkpoints yet, not an error: a first
            // charting run always starts with an empty store.
            return Ok(Vec::new());
        };

        let mut checkpoints = Vec::new();
        for line in BufReader::new(file).lines() {
            let line = line.map_err(|err| {
                store_failed(format!("cannot read {}: {err}", self.path.display()))
            })?;
            if line.trim().is_empty() {
                continue;
            }
            let checkpoint: Checkpoint = serde_json::from_str(&line).map_err(|err| {
                store_failed(format!(
                    "cannot parse a checkpoint line in {}: {err}",
                    self.path.display()
                ))
            })?;
            if checkpoint.map_id == map_id {
                checkpoints.push(checkpoint);
            }
        }
        Ok(checkpoints)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stage_numbers_match_the_do_step_1_table() {
        assert_eq!(Stage::Destination.number(), 1);
        assert_eq!(Stage::Seed.number(), 2);
        assert_eq!(Stage::AxisSweep.number(), 3);
        assert_eq!(Stage::Extract.number(), 4);
        assert_eq!(Stage::Sharpen.number(), 5);
        assert_eq!(Stage::Size.number(), 6);
        assert_eq!(Stage::Wire.number(), 7);
    }

    #[test]
    fn from_number_round_trips_with_number() {
        for stage in Stage::ORDER {
            assert_eq!(Stage::from_number(stage.number()), Some(stage));
        }
        assert_eq!(Stage::from_number(0), None);
        assert_eq!(Stage::from_number(8), None);
    }

    #[test]
    fn a_fresh_store_has_no_checkpoints() {
        let dir = tempfile::tempdir().unwrap();
        let store = FileCheckpointStore::new(dir.path().join("checkpoints.jsonl"));
        assert!(store.load("map-1").unwrap().is_empty());
    }

    #[test]
    fn record_then_load_returns_the_checkpoint_in_order() {
        let dir = tempfile::tempdir().unwrap();
        let store = FileCheckpointStore::new(dir.path().join("checkpoints.jsonl"));

        store
            .record(&Checkpoint {
                map_id: "map-1".to_owned(),
                stage: Stage::Destination,
                recorded_at: 1,
                payload: serde_json::json!({"destination": "a"}),
            })
            .unwrap();
        store
            .record(&Checkpoint {
                map_id: "map-1".to_owned(),
                stage: Stage::Seed,
                recorded_at: 2,
                payload: serde_json::json!({}),
            })
            .unwrap();

        let loaded = store.load("map-1").unwrap();
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].stage, Stage::Destination);
        assert_eq!(loaded[1].stage, Stage::Seed);
    }

    #[test]
    fn load_filters_by_map_id() {
        let dir = tempfile::tempdir().unwrap();
        let store = FileCheckpointStore::new(dir.path().join("checkpoints.jsonl"));

        store
            .record(&Checkpoint {
                map_id: "map-1".to_owned(),
                stage: Stage::Destination,
                recorded_at: 1,
                payload: serde_json::Value::Null,
            })
            .unwrap();
        store
            .record(&Checkpoint {
                map_id: "map-2".to_owned(),
                stage: Stage::Destination,
                recorded_at: 2,
                payload: serde_json::Value::Null,
            })
            .unwrap();

        assert_eq!(store.load("map-1").unwrap().len(), 1);
        assert_eq!(store.load("map-2").unwrap().len(), 1);
        assert!(store.load("map-3").unwrap().is_empty());
    }

    #[test]
    fn a_blank_line_is_skipped_rather_than_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("checkpoints.jsonl");
        std::fs::write(&path, "\n").unwrap();
        let store = FileCheckpointStore::new(path);
        assert!(store.load("map-1").unwrap().is_empty());
    }
}
