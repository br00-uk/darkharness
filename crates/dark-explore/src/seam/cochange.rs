//! Co-change coupling, read from the repository's own history.
//!
//! Two files that always change together are not really separate, whatever
//! the import graph says about them. This is the term that catches a
//! boundary which looks clean structurally and is not one in practice. See
//! Do step 6 of task unit `F3`.
//!
//! ```text
//! C(a, b) = commits that touch both / commits that touch either
//! ```
//!
//! # Why the window is part of the configuration hash
//!
//! Coupling read over 500 commits and coupling read over 5000 are different
//! numbers for the same repository, so the window changes the output. Rule
//! 29 requires identical bytes for the same commit *and configuration*, so
//! the window belongs in the configuration hash. [`Window::commits`] is
//! what a caller feeds into it.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;

use dark_contract::{ErrCode, Error, Result};

/// How many commits back to read. Do step 6 of task unit `F3`.
pub const DEFAULT_WINDOW: usize = 500;

/// The history window that one coupling reading covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Window {
    /// How many commits back the reading covers.
    pub commits: usize,
}

impl Default for Window {
    fn default() -> Self {
        Self {
            commits: DEFAULT_WINDOW,
        }
    }
}

/// How often each pair of files changed together.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CoChange {
    /// How many commits in the window touched each file.
    touched: BTreeMap<PathBuf, u32>,
    /// How many commits touched both files of a pair. The pair is always
    /// stored with the lower path first, so a lookup finds it whichever way
    /// round the caller asks.
    together: BTreeMap<(PathBuf, PathBuf), u32>,
    /// The window this reading covers.
    pub window: Window,
    /// How many commits the reading actually saw. A repository younger than
    /// the window has fewer.
    pub commits_read: usize,
}

/// Orders a pair so a lookup finds it whichever way round it is asked for.
fn key(a: &Path, b: &Path) -> (PathBuf, PathBuf) {
    if a <= b {
        (a.to_path_buf(), b.to_path_buf())
    } else {
        (b.to_path_buf(), a.to_path_buf())
    }
}

impl CoChange {
    /// The coupling between two files, from 0 to 1.
    ///
    /// Returns 0 when neither file appears in the window: two files that
    /// never changed have no evidence of coupling, and treating an absence
    /// of evidence as coupling would penalise a new file for being new.
    #[must_use]
    pub fn coupling(&self, a: &Path, b: &Path) -> f64 {
        let both = self.together.get(&key(a, b)).copied().unwrap_or(0);
        let a_count = self.touched.get(a).copied().unwrap_or(0);
        let b_count = self.touched.get(b).copied().unwrap_or(0);

        // Commits touching either, by inclusion and exclusion.
        let either = a_count + b_count - both;
        if either == 0 {
            return 0.0;
        }
        f64::from(both) / f64::from(either)
    }

    /// Builds a reading from already-parsed commits.
    ///
    /// Each entry is the set of paths one commit touched. This is the seam
    /// between parsing and counting, so a test can drive the counting
    /// without a repository.
    #[must_use]
    pub fn from_commits(commits: &[BTreeSet<PathBuf>], window: Window) -> Self {
        let mut touched: BTreeMap<PathBuf, u32> = BTreeMap::new();
        let mut together: BTreeMap<(PathBuf, PathBuf), u32> = BTreeMap::new();

        for paths in commits {
            let ordered: Vec<&PathBuf> = paths.iter().collect();
            for (index, path) in ordered.iter().enumerate() {
                *touched.entry((*path).clone()).or_insert(0) += 1;
                for other in ordered.iter().skip(index + 1) {
                    *together.entry(key(path, other)).or_insert(0) += 1;
                }
            }
        }

        Self {
            touched,
            together,
            window,
            commits_read: commits.len(),
        }
    }

    /// Reads the coupling from a repository's git history.
    ///
    /// # Errors
    ///
    /// Returns [`ErrCode::ExploreParse`] when git cannot be run or reports a
    /// failure. A repository with no commits yet is not a failure: it reads
    /// as an empty window.
    pub fn read(repo_root: &Path, window: Window) -> Result<Self> {
        let output = Command::new("git")
            .arg("-C")
            .arg(repo_root)
            .arg("log")
            .arg("--numstat")
            .arg("--format=%H")
            .arg("-n")
            .arg(window.commits.to_string())
            .output()
            .map_err(|source| {
                Error::new(
                    ErrCode::ExploreParse,
                    format!("cannot run git in {}: {source}", repo_root.display()),
                )
                .with_remedy("Check that git is installed and the path is a repository.")
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            // A repository with no commits reports a failure, and that is
            // not one: a new repository simply has no history to read.
            if stderr.contains("does not have any commits")
                || stderr.contains("bad default revision")
            {
                return Ok(Self::from_commits(&[], window));
            }
            return Err(Error::new(
                ErrCode::ExploreParse,
                format!(
                    "git log failed in {}: {}",
                    repo_root.display(),
                    stderr.trim()
                ),
            )
            .with_remedy("Check that the path is a git repository with a checked-out branch."));
        }

        let text = String::from_utf8_lossy(&output.stdout);
        Ok(Self::from_commits(&parse_numstat(&text), window))
    }
}

/// Parses `git log --numstat --format=%H` output into one path set per
/// commit.
///
/// A numstat line is `added<TAB>deleted<TAB>path`. A binary file reports
/// `-` for both counts and still counts as touched. A rename reports
/// `old => new`; the new path is the one that matters, because that is what
/// the graph knows about.
fn parse_numstat(text: &str) -> Vec<BTreeSet<PathBuf>> {
    let mut commits: Vec<BTreeSet<PathBuf>> = Vec::new();
    let mut current: Option<BTreeSet<PathBuf>> = None;

    for line in text.lines() {
        if line.is_empty() {
            continue;
        }

        // A commit hash: 40 hexadecimal characters and nothing else.
        let is_hash = line.len() == 40 && line.bytes().all(|b| b.is_ascii_hexdigit());
        if is_hash {
            if let Some(paths) = current.take() {
                commits.push(paths);
            }
            current = Some(BTreeSet::new());
            continue;
        }

        let mut fields = line.split('\t');
        let (Some(_added), Some(_deleted), Some(path)) =
            (fields.next(), fields.next(), fields.next())
        else {
            continue;
        };

        if let Some(paths) = current.as_mut() {
            paths.insert(PathBuf::from(rename_target(path)));
        }
    }

    if let Some(paths) = current.take() {
        commits.push(paths);
    }
    commits
}

/// Returns the path a numstat field names, following a rename to its new
/// name.
///
/// Git writes a rename either as `old => new` or, when the two share a
/// prefix, as `src/{old => new}.rs`. Both forms name the file after the
/// rename on the right of the arrow.
fn rename_target(field: &str) -> String {
    let Some(arrow) = field.find(" => ") else {
        return field.to_owned();
    };

    let (before, after) = field.split_at(arrow);
    let after = &after[" => ".len()..];

    // The braced form: keep the text either side of the braces.
    if let Some(open) = before.rfind('{') {
        if let Some(close) = after.find('}') {
            let prefix = &before[..open];
            let middle = &after[..close];
            let suffix = &after[close + 1..];
            return format!("{prefix}{middle}{suffix}");
        }
    }
    after.to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn commit(paths: &[&str]) -> BTreeSet<PathBuf> {
        paths.iter().map(PathBuf::from).collect()
    }

    #[test]
    fn two_files_that_always_change_together_couple_fully() {
        let commits = vec![commit(&["a.rs", "b.rs"]), commit(&["a.rs", "b.rs"])];
        let found = CoChange::from_commits(&commits, Window::default());
        let coupling = found.coupling(Path::new("a.rs"), Path::new("b.rs"));
        assert!((coupling - 1.0).abs() < f64::EPSILON, "got {coupling}");
    }

    #[test]
    fn two_files_that_never_change_together_do_not_couple() {
        let commits = vec![commit(&["a.rs"]), commit(&["b.rs"])];
        let found = CoChange::from_commits(&commits, Window::default());
        assert!(found.coupling(Path::new("a.rs"), Path::new("b.rs")).abs() < f64::EPSILON);
    }

    #[test]
    fn half_shared_commits_couple_by_a_third() {
        // a in 2 commits, b in 2, both in 1: 1 / (2 + 2 - 1) = 1/3.
        let commits = vec![
            commit(&["a.rs", "b.rs"]),
            commit(&["a.rs"]),
            commit(&["b.rs"]),
        ];
        let found = CoChange::from_commits(&commits, Window::default());
        let coupling = found.coupling(Path::new("a.rs"), Path::new("b.rs"));
        assert!((coupling - 1.0 / 3.0).abs() < 1e-9, "got {coupling}");
    }

    #[test]
    fn the_lookup_finds_a_pair_whichever_way_round_it_is_asked() {
        let commits = vec![commit(&["z.rs", "a.rs"])];
        let found = CoChange::from_commits(&commits, Window::default());
        let forward = found.coupling(Path::new("a.rs"), Path::new("z.rs"));
        let backward = found.coupling(Path::new("z.rs"), Path::new("a.rs"));
        assert!((forward - backward).abs() < f64::EPSILON);
        assert!((forward - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn a_file_the_window_never_saw_scores_zero_rather_than_dividing_by_zero() {
        let found = CoChange::from_commits(&[], Window::default());
        let coupling = found.coupling(Path::new("new.rs"), Path::new("other.rs"));
        assert!(coupling.is_finite(), "must not be NaN");
        assert!(coupling.abs() < f64::EPSILON);
    }

    #[test]
    fn numstat_parses_one_commit_per_hash() {
        let text = "\
aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
1\t2\tsrc/lib.rs
3\t0\tsrc/main.rs
bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb
5\t5\tsrc/lib.rs
";
        let commits = parse_numstat(text);
        assert_eq!(commits.len(), 2);
        assert_eq!(commits[0].len(), 2);
        assert_eq!(commits[1].len(), 1);
    }

    #[test]
    fn a_binary_file_reports_dashes_and_still_counts_as_touched() {
        let text = "\
aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
-\t-\tassets/logo.png
";
        let commits = parse_numstat(text);
        assert!(commits[0].contains(&PathBuf::from("assets/logo.png")));
    }

    #[test]
    fn a_rename_counts_under_its_new_name() {
        assert_eq!(rename_target("old.rs => new.rs"), "new.rs");
        assert_eq!(rename_target("src/{old => new}.rs"), "src/new.rs");
        assert_eq!(rename_target("plain.rs"), "plain.rs");
    }

    #[test]
    fn reading_a_real_repository_sees_its_own_commits() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();

        let git = |args: &[&str]| {
            Command::new("git")
                .arg("-C")
                .arg(root)
                .args(args)
                .output()
                .expect("git runs")
        };
        git(&["init", "-q"]);
        git(&["config", "user.email", "test@example.invalid"]);
        git(&["config", "user.name", "Test"]);

        std::fs::write(root.join("a.rs"), "fn a() {}").expect("write");
        std::fs::write(root.join("b.rs"), "fn b() {}").expect("write");
        git(&["add", "."]);
        git(&["commit", "-qm", "both"]);

        std::fs::write(root.join("a.rs"), "fn a() { }").expect("write");
        git(&["add", "."]);
        git(&["commit", "-qm", "only a"]);

        let found = CoChange::read(root, Window::default()).expect("the history reads");
        assert_eq!(found.commits_read, 2);

        // a in 2, b in 1, both in 1: 1 / (2 + 1 - 1) = 0.5.
        let coupling = found.coupling(Path::new("a.rs"), Path::new("b.rs"));
        assert!((coupling - 0.5).abs() < 1e-9, "got {coupling}");
    }

    #[test]
    fn a_repository_with_no_commits_reads_as_an_empty_window() {
        let dir = tempfile::tempdir().expect("tempdir");
        Command::new("git")
            .arg("-C")
            .arg(dir.path())
            .args(["init", "-q"])
            .output()
            .expect("git runs");

        let found = CoChange::read(dir.path(), Window::default())
            .expect("a repository with no history is not a failure");
        assert_eq!(found.commits_read, 0);
    }
}
