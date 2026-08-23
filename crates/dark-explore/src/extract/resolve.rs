//! Cross-file reference resolution: F2, "Do" items 3 to 5.
//!
//! [`file::extract_file`] already resolved what it could from a file's own
//! lexical scopes (`ResolutionConfidence::Exact`). This pass runs once,
//! over every file's output at once, and tries the two resolutions that
//! need more than one file's view: `ImportScoped`, through the file's own
//! import map, and `NameOnly`, through a repository-wide, unique name
//! match. A reference that neither pass can place stays unresolved,
//! exactly as F2, "Do not" requires: "do not report a guessed reference as
//! resolved."

use std::collections::HashMap;

use super::types::{FileSymbols, ResolutionConfidence, ResolvedSymbol};

/// Resolves every still-unresolved reference across `files`, in place.
///
/// Only an **exported** definition is a candidate for cross-file
/// resolution: an unexported definition in another file was never visible
/// to begin with, so matching a reference to one would not be a resolution
/// — it would be a guess dressed up as one.
pub(crate) fn resolve_cross_file(files: &mut [FileSymbols]) {
    let mut global_index: HashMap<String, Vec<(usize, usize)>> = HashMap::new();
    for (file_index, file) in files.iter().enumerate() {
        for (def_index, def) in file.defs.iter().enumerate() {
            if def.exported {
                global_index
                    .entry(def.name.clone())
                    .or_default()
                    .push((file_index, def_index));
            }
        }
    }

    for file_index in 0..files.len() {
        let updates = plan_updates(files, file_index, &global_index);
        for (ref_index, resolved_to, confidence) in updates {
            files[file_index].refs[ref_index].resolved_to = resolved_to;
            files[file_index].refs[ref_index].confidence = confidence;
        }
    }
}

type Update = (usize, Option<ResolvedSymbol>, Option<ResolutionConfidence>);

fn plan_updates(
    files: &[FileSymbols],
    file_index: usize,
    global_index: &HashMap<String, Vec<(usize, usize)>>,
) -> Vec<Update> {
    let file = &files[file_index];
    let mut updates = Vec::new();

    for (ref_index, reference) in file.refs.iter().enumerate() {
        if reference.resolved_to.is_some() {
            continue;
        }

        let matching_import = file
            .imports
            .iter()
            .find(|imp| imp.imported_names.iter().any(|n| n == &reference.name));

        if let Some(import) = matching_import {
            // The name is explicitly governed by an import statement, even
            // when that import itself did not resolve to a file. Falling
            // through to a repository-wide name-only match here would
            // ignore that evidence and risk matching an unrelated,
            // same-named local item instead of the (unresolvable) import
            // target. Leave it unresolved rather than guess.
            if let Some(target_path) = &import.resolved_to
                && let Some(target_file) = files.iter().find(|f| &f.path == target_path)
                && let Some(def_index) = target_file
                    .defs
                    .iter()
                    .position(|d| d.name == reference.name && d.exported)
            {
                updates.push((
                    ref_index,
                    Some(ResolvedSymbol {
                        file: target_path.clone(),
                        def_index,
                    }),
                    Some(ResolutionConfidence::ImportScoped),
                ));
            }
            continue;
        }

        // Not governed by any import: a repository-wide name match is
        // usable only when it is unique. See F2, "Do not" — "do not
        // report a guessed reference as resolved," and the requirement
        // that a name matching definitions in two files resolves to
        // neither.
        if let Some(candidates) = global_index.get(&reference.name)
            && let [(f_idx, d_idx)] = candidates[..]
        {
            updates.push((
                ref_index,
                Some(ResolvedSymbol {
                    file: files[f_idx].path.clone(),
                    def_index: d_idx,
                }),
                Some(ResolutionConfidence::NameOnly),
            ));
        }
    }

    updates
}
