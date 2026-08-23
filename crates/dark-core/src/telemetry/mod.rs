//! Telemetry: local measurements of harness performance, and nothing else.
//!
//! This module writes one line per turn to `$DARK_HOME/telemetry.jsonl`
//! (section 5.3 of the build specification), and reads that file back so
//! `dark stats` (task unit `J6`) can render it.
//!
//! # This file never leaves the machine
//!
//! Nothing in this module constructs a network connection. Only
//! `dark-airlock` may construct an HTTP client anywhere in this workspace
//! (Rule 13), and `cargo deny` fails the build the moment another crate's
//! dependency tree gains one — but that check exists to catch a mistake,
//! not to be the reason this module stays local. The reason is the primary
//! requirement of the whole harness: a person disconnects the network
//! after `dark setup` completes and keeps working, and telemetry that
//! phoned anywhere would break that promise the moment it ran. There is no
//! remote sink, no upload, no opt-out setting a person has to find and
//! flip, and no code path here that reads `telemetry.jsonl` for any
//! purpose but printing it back to the person who owns the machine it
//! lives on.
//!
//! # What a record can and cannot hold
//!
//! [`TelemetryRecord`] carries counts and durations: turn duration, tokens
//! in and out, the generation rate, model load count and duration, the
//! tool failure rate, the prefix cache hit rate, and frame-budget
//! overruns. It never carries a prompt, a reply, a tool result, a file
//! path, or a file's content — see [`TelemetryRecorder::on_event`] for
//! exactly which event fields it reads, and which it deliberately never
//! touches.
//!
//! # This module never reads `DARK_HOME`
//!
//! Every function that touches storage takes `dark_home` as a parameter,
//! the same discipline [`crate::session::transcript`] uses. The
//! composition root (`dark-cli`'s `dark_home` function) resolves the
//! environment variable; this crate never does.

mod record;
mod recorder;
mod writer;

pub use record::TelemetryRecord;
pub use recorder::TelemetryRecorder;
pub use writer::{TelemetryWriter, read_records, telemetry_path};
