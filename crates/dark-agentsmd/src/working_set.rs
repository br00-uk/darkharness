//! The working set: which directories this turn cares about.

use std::path::PathBuf;

/// The directories that decide which nested `AGENTS.md` files join the
/// prefix.
///
/// Build this once, at the start of a turn, from the claimed ticket's
/// scope, the paths that the person's message names, and the paths that
/// the previous turn changed. See task unit K1, step 4.
///
/// Every path in a working set names a directory, not a file. Pass a
/// changed file's parent directory, not the file itself — the resolver
/// walks directories, and guessing "is this a file or a directory" from a
/// string is not reliable across platforms.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorkingSet {
    /// The scope of the ticket that this session claimed.
    pub ticket_scope: Vec<PathBuf>,
    /// The directories that the person's input message named.
    pub message_paths: Vec<PathBuf>,
    /// The directories that the previous turn changed.
    pub previous_turn_changed: Vec<PathBuf>,
}

impl WorkingSet {
    /// Creates an empty working set.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns every path in the working set, deduplicated and sorted with
    /// a byte comparator, so the result does not depend on insertion order
    /// or on the platform's locale. See Rule 30.
    #[must_use]
    pub fn all_paths(&self) -> Vec<PathBuf> {
        let mut paths: Vec<PathBuf> = self
            .ticket_scope
            .iter()
            .chain(&self.message_paths)
            .chain(&self.previous_turn_changed)
            .cloned()
            .collect();
        paths.sort_by(|a, b| {
            a.as_os_str()
                .as_encoded_bytes()
                .cmp(b.as_os_str().as_encoded_bytes())
        });
        paths.dedup();
        paths
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_paths_dedups_across_the_three_sources() {
        let mut ws = WorkingSet::new();
        ws.ticket_scope.push(PathBuf::from("/repo/a"));
        ws.message_paths.push(PathBuf::from("/repo/a"));
        ws.previous_turn_changed.push(PathBuf::from("/repo/b"));

        let paths = ws.all_paths();
        assert_eq!(
            paths,
            vec![PathBuf::from("/repo/a"), PathBuf::from("/repo/b")]
        );
    }

    #[test]
    fn all_paths_is_sorted_regardless_of_insertion_order() {
        let mut ws = WorkingSet::new();
        ws.message_paths.push(PathBuf::from("/repo/z"));
        ws.message_paths.push(PathBuf::from("/repo/a"));

        assert_eq!(
            ws.all_paths(),
            vec![PathBuf::from("/repo/a"), PathBuf::from("/repo/z")]
        );
    }
}
