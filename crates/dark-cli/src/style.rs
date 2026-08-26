//! Deriving the house style, which needs the parse `dark-explore` runs.
//!
//! [`dark_explore::style::profile`] works from extraction's output, and
//! [`dark_explore::style::measure_source`] needs the bytes. Both are
//! produced by the same pass, so this runs it once and hands the profiler
//! both — reading every file a second time to count its indentation would
//! double the cost of the slowest thing `dark extend` does.

use std::path::Path;

use anyhow::Result;
use dark_explore::style::{StyleProfile, measure_source, profile};
use dark_explore::syntax::Cache;
use dark_explore::{discover, extract, syntax};

/// Derives the style profile for the repository at `root`.
///
/// # Errors
///
/// Returns an error when discovery or parsing fails.
pub(crate) fn profile_for(root: &Path) -> Result<StyleProfile> {
    let options = discover::DiscoverOptions::default();
    let snapshot = discover::discover(root, &options).map_err(crate::contract_error)?;
    let (parsed, _cache) =
        syntax::parse_snapshot(&Cache::new(), root, &snapshot).map_err(crate::contract_error)?;

    // The parse already read every file, so the bytes are to hand.
    let shape = measure_source(parsed.iter().map(|file| &*file.source));
    let files = extract::extract_repository(&snapshot, &parsed);
    Ok(profile(&files, Some(shape)))
}
