//! Checks that discovery's `.gitignore` handling matches `git check-ignore`.
//!
//! F1 requires this parity, negation patterns included, because a naive
//! gitignore implementation is where most divergence from real Git
//! happens: negation, a nested `.gitignore`, and a directory-only pattern
//! are the three places that trip up a hand-rolled matcher. This test
//! builds one fixture tree that exercises all three, then asks the real
//! `git check-ignore` and `dark_explore::discover::discover` the same
//! question about every file in the tree and asserts they agree.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use dark_explore::discover::{self, DiscoverOptions};

/// Builds the fixture tree under `root`.
///
/// - `.gitignore` at the root exercises a wildcard pattern, a negation
///   (`!important.log`), and a directory-only pattern (`dist/`).
/// - `src/.gitignore` is a nested ignore file with its own negation, and it
///   must apply to `src/nested/` too, not only to direct children of `src/`.
/// - `keep/dist` is a plain *file* named `dist`, which the directory-only
///   pattern `dist/` must not match — only a directory named `dist` matches
///   a directory-only pattern.
fn build_fixture(root: &Path) {
    let write = |rel: &str, content: &str| {
        let path = root.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    };

    write(".gitignore", "*.log\n!important.log\ndist/\nsecrets/\n");
    write("a.log", "dropped by the wildcard");
    write("important.log", "kept by the negation");
    write("normal.rs", "fn normal() {}");
    write(
        "dist/output.txt",
        "dropped: inside a directory the pattern excludes",
    );
    write("secrets/key.txt", "dropped: inside an excluded directory");
    write("keep/dist", "kept: a file named dist, not a directory");

    write("src/.gitignore", "*.tmp\n!keep.tmp\n");
    write("src/main.rs", "fn main() {}");
    write("src/scratch.tmp", "dropped by the nested rule");
    write("src/keep.tmp", "kept by the nested negation");
    write(
        "src/nested/deep.tmp",
        "dropped: the nested rule applies below src/, too",
    );
    write("src/nested/deep.rs", "fn deep() {}");
}

/// Lists every regular file under `root`, relative to `root`, skipping
/// `.git`. The list is not sorted; each caller sorts however it needs.
fn list_files(root: &Path) -> Vec<PathBuf> {
    fn walk(dir: &Path, root: &Path, out: &mut Vec<PathBuf>) {
        for entry in fs::read_dir(dir).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            if path.file_name().and_then(|n| n.to_str()) == Some(".git") {
                continue;
            }
            if path.is_dir() {
                walk(&path, root, out);
            } else {
                out.push(path.strip_prefix(root).unwrap().to_path_buf());
            }
        }
    }
    let mut out = Vec::new();
    walk(root, root, &mut out);
    out
}

/// Asks the real `git check-ignore` whether `relative_path` is ignored.
///
/// Returns `Some(true)` when Git ignores the path, `Some(false)` when it
/// does not, and `None` when the `git` binary is unavailable, so the test
/// can skip gracefully in an environment without Git rather than reporting
/// a false failure.
fn git_ignores(repo: &Path, relative_path: &Path) -> Option<bool> {
    let output = Command::new("git")
        .current_dir(repo)
        .args(["check-ignore", "-q", "--"])
        .arg(relative_path)
        .output()
        .ok()?;
    match output.status.code() {
        Some(0) => Some(true),
        Some(1) => Some(false),
        other => panic!(
            "git check-ignore exited with an unexpected code {other:?} for {}",
            relative_path.display()
        ),
    }
}

#[test]
fn matches_git_check_ignore_including_negation_and_directory_only_patterns() {
    if Command::new("git").arg("--version").output().is_err() {
        eprintln!("git is not on PATH; skipping the conformance check");
        return;
    }

    let repo = tempfile::tempdir().unwrap();
    let status = Command::new("git")
        .args(["init", "-q"])
        .arg(repo.path())
        .status()
        .expect("failed to run git init");
    assert!(status.success(), "git init failed");

    build_fixture(repo.path());

    let snapshot = discover::discover(repo.path(), &DiscoverOptions::default()).unwrap();
    let mut ours: Vec<PathBuf> = snapshot.files.iter().map(|f| f.path.clone()).collect();
    ours.sort();

    let mut expected_kept = Vec::new();
    let mut checked_at_least_one_ignored = false;
    let mut checked_at_least_one_kept = false;
    for path in list_files(repo.path()) {
        let Some(ignored) = git_ignores(repo.path(), &path) else {
            eprintln!("git is not on PATH; skipping the conformance check");
            return;
        };
        if ignored {
            checked_at_least_one_ignored = true;
        } else {
            checked_at_least_one_kept = true;
            expected_kept.push(path);
        }
    }
    expected_kept.sort();

    assert!(
        checked_at_least_one_ignored,
        "the fixture should exercise at least one ignore rule"
    );
    assert!(
        checked_at_least_one_kept,
        "the fixture should keep at least one file"
    );
    assert_eq!(
        ours, expected_kept,
        "discover() must keep exactly the files that `git check-ignore` does not ignore"
    );

    // Spell out the cases the brief calls out by name, so a regression in
    // one of them fails on its own line instead of only on the set diff
    // above.
    let kept = |rel: &str| ours.iter().any(|p| p == Path::new(rel));
    assert!(
        kept("important.log"),
        "negation (!important.log) must keep the file"
    );
    assert!(!kept("a.log"), "the wildcard *.log must still drop a.log");
    assert!(
        kept("keep/dist"),
        "a plain file named dist must survive the directory-only pattern dist/"
    );
    assert!(
        !kept("dist/output.txt"),
        "the directory-only pattern dist/ must drop its contents"
    );
    assert!(
        kept("src/keep.tmp"),
        "the nested .gitignore's negation must keep src/keep.tmp"
    );
    assert!(
        !kept("src/scratch.tmp"),
        "the nested .gitignore must drop src/scratch.tmp"
    );
    assert!(
        !kept("src/nested/deep.tmp"),
        "the nested .gitignore's rule must reach src/nested/, not only direct children of src/"
    );
}
