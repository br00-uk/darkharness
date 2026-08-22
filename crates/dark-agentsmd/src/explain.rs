//! Renders the resolved instruction chain, and warns about known quality
//! problems in it. See task unit K3.
//!
//! [`render`] is the text behind `dark agents explain`; a later change
//! wires it into `dark-cli` (this crate never touches that crate). It is a
//! pure function of a [`ResolvedChain`] plus the repository root and an
//! optional `README.md` body: no clock, and no absolute path that would
//! differ from one machine to the next, so its output is fit for a golden
//! file (Rule 29 style determinism, applied here for the same reason: a
//! stable, reviewable rendering).

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::path::Path;

use crate::chain::{ChainEntry, ChainRole, FileKind, ResolvedChain};

/// The word-shingle size used to measure duplication between the root
/// `AGENTS.md` and `README.md` (step 2). Five words is long enough that a
/// generic phrase rarely collides by chance, and short enough that a
/// duplicated paragraph is still caught in a short file.
const SHINGLE_SIZE: usize = 5;

/// The shingle-overlap fraction above which [`quality_warnings`] warns
/// about duplication between the root `AGENTS.md` and `README.md`.
///
/// Measured as the Jaccard similarity of the two files' shingle sets:
/// `|shingles(AGENTS.md) ∩ shingles(README.md)| / |shingles(AGENTS.md) ∪
/// shingles(README.md)|`. This is the standard shingle-overlap measure for
/// near-duplicate text, and it is symmetric: which file is "the" source
/// does not change the score.
const OVERLAP_WARNING_THRESHOLD: f64 = 0.40;

/// The line count above which [`quality_warnings`] suggests splitting the
/// root instruction file into nested files (step 3).
const ROOT_LINE_WARNING_THRESHOLD: usize = 150;

/// Renders `chain` for `dark agents explain`.
///
/// Lists every entry in resolution order, with its token count and what it
/// overrode, then a total token count, then every warning: the budget
/// warnings [`ResolvedChain::warnings`] already carries, followed by the
/// quality warnings from [`quality_warnings`]. A warning never removes an
/// entry or changes a count — explain only ever adds text (task unit K3:
/// "Do not block on a warning").
///
/// `repo_root` is used only to shorten each entry's path for display: a
/// path under it renders relative to it, and the one global entry (see
/// [`ChainRole::Global`]) renders as `~/.darkharness/<file name>` rather
/// than the real home directory, which is not the same on every machine.
/// Neither `repo_root` nor any other input here can make two calls with the
/// same `chain` render different text.
///
/// `readme` is the `README.md` body, when the caller has one to compare
/// against the root `AGENTS.md` file. Pass `None` when the repository has
/// no `README.md`; the duplication warning is simply skipped.
#[must_use]
pub fn render(chain: &ResolvedChain, repo_root: &Path, readme: Option<&str>) -> String {
    let entries = chain.entries();
    let mut out = String::new();

    if entries.is_empty() {
        out.push_str(
            "AGENTS.md chain: empty. No AGENTS.md, AGENTS.override.md, CLAUDE.md, or GEMINI.md \
             file governs this turn.\n",
        );
        return out;
    }

    let _ = writeln!(
        out,
        "AGENTS.md chain: {} file{}, {} token{} total.",
        entries.len(),
        plural(entries.len()),
        chain.total_tokens(),
        plural(chain.total_tokens()),
    );

    for (index, entry) in entries.iter().enumerate() {
        out.push('\n');
        let _ = writeln!(
            out,
            "{}. {} ({} / {}) — {} token{}{}",
            index + 1,
            display_path(entry, repo_root),
            entry.source.role,
            entry.source.kind,
            entry.tokens,
            plural(entry.tokens),
            if entry.truncated { ", truncated" } else { "" },
        );
        let _ = writeln!(out, "   overrides: {}", overrides_text(index, entry));
    }

    let quality = quality_warnings(chain, readme);
    let budget_warnings = chain.warnings();
    if !budget_warnings.is_empty() || !quality.is_empty() {
        out.push('\n');
        out.push_str("Warnings:\n");
        for warning in budget_warnings {
            let _ = writeln!(out, "- {warning}");
        }
        for warning in &quality {
            let _ = writeln!(out, "- {warning}");
        }
    }

    out
}

/// Returns the plural suffix for an English count: empty for `1`, `"s"`
/// otherwise.
fn plural(count: usize) -> &'static str {
    if count == 1 { "" } else { "s" }
}

/// Renders one entry's path for display.
///
/// The global entry never shows the real home directory: it shows the
/// well-known location, `~/.darkharness/<file name>`, so the rendering does
/// not vary with where a particular machine's home directory sits. Every
/// other entry renders relative to `repo_root` when it sits under it, and
/// falls back to the full path only when it does not (which should not
/// happen for a chain that [`crate::resolve::resolve`] produced).
fn display_path(entry: &ChainEntry, repo_root: &Path) -> String {
    if entry.source.role == ChainRole::Global {
        let name = entry
            .path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        return format!("~/.darkharness/{name}");
    }

    match entry.path.strip_prefix(repo_root) {
        Ok(relative) => relative.to_string_lossy().into_owned(),
        Err(_) => entry.path.to_string_lossy().into_owned(),
    }
}

/// Describes what one entry overrode, for the "overrides:" line.
fn overrides_text(index: usize, entry: &ChainEntry) -> String {
    if entry.source.kind == FileKind::Override {
        "every entry resolution found before it in this directory's ancestry; \
         AGENTS.override.md replaces the chain so far, it does not extend it"
            .to_owned()
    } else if index == 0 {
        "nothing; it is first in the chain".to_owned()
    } else {
        "the entries above it, where this file disagrees with them".to_owned()
    }
}

/// Runs the quality checks from task unit K3, steps 2 and 3, and returns
/// one warning string per problem found. Returns an empty vector when
/// neither check fires. Never blocks: this is advice, not a [`Result`].
///
/// [`Result`]: dark_contract::Result
#[must_use]
pub fn quality_warnings(chain: &ResolvedChain, readme: Option<&str>) -> Vec<String> {
    let mut warnings = Vec::new();

    let Some(root) = root_entry(chain) else {
        return warnings;
    };

    if let (Some(readme_text), FileKind::Agents) = (readme, &root.source.kind) {
        let overlap = shingle_overlap(&root.content, readme_text);
        if overlap > OVERLAP_WARNING_THRESHOLD {
            warnings.push(format!(
                "AGENTS.md and README.md overlap by {:.0}% (5-word shingles), over the {:.0}% \
                 threshold; move the shared material into one file and point the other at it.",
                overlap * 100.0,
                OVERLAP_WARNING_THRESHOLD * 100.0,
            ));
        }
    }

    let line_count = root.content.lines().count();
    if line_count > ROOT_LINE_WARNING_THRESHOLD {
        warnings.push(format!(
            "the root instruction file is {line_count} lines, over the \
             {ROOT_LINE_WARNING_THRESHOLD}-line guideline; move effort-specific rules into a \
             nested AGENTS.md file closer to the code they govern."
        ));
    }

    warnings
}

/// Returns the chain's repository-root entry: the one entry, when any,
/// whose role is [`ChainRole::Directory`] at depth `0`.
fn root_entry(chain: &ResolvedChain) -> Option<&ChainEntry> {
    chain
        .entries()
        .iter()
        .find(|entry| entry.source.role == ChainRole::Directory { depth: 0 })
}

/// Returns the Jaccard similarity of `a` and `b`'s five-word shingle sets:
/// `0.0` when they share nothing (or either is too short to shingle),
/// `1.0` when their shingle sets are identical.
fn shingle_overlap(a: &str, b: &str) -> f64 {
    let shingles_a = word_shingles(a);
    let shingles_b = word_shingles(b);
    if shingles_a.is_empty() || shingles_b.is_empty() {
        return 0.0;
    }

    let intersection = shingles_a.intersection(&shingles_b).count();
    let union = shingles_a.union(&shingles_b).count();
    #[allow(clippy::cast_precision_loss)]
    let ratio = intersection as f64 / union as f64;
    ratio
}

/// Splits `text` into lowercase words and returns the set of contiguous
/// `SHINGLE_SIZE`-word windows. A text shorter than `SHINGLE_SIZE` words
/// becomes its own single shingle, so two very short, identical files still
/// compare as a full overlap rather than an empty one.
fn word_shingles(text: &str) -> BTreeSet<String> {
    let words: Vec<String> = text.split_whitespace().map(str::to_lowercase).collect();
    if words.is_empty() {
        return BTreeSet::new();
    }
    if words.len() < SHINGLE_SIZE {
        return BTreeSet::from([words.join(" ")]);
    }
    words
        .windows(SHINGLE_SIZE)
        .map(|window| window.join(" "))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chain::ChainSource;
    use crate::config::AgentsMdConfig;
    use crate::resolve::resolve;
    use crate::working_set::WorkingSet;
    use std::fs;
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn count_words(text: &str) -> usize {
        text.split_whitespace().count()
    }

    fn entry(role: ChainRole, kind: FileKind, path: &str, content: &str) -> ChainEntry {
        ChainEntry {
            path: PathBuf::from(path),
            source: ChainSource { role, kind },
            directory: PathBuf::from(path).parent().unwrap().to_path_buf(),
            content: content.to_owned(),
            tokens: content.split_whitespace().count(),
            truncated: false,
        }
    }

    #[test]
    fn render_reports_an_empty_chain() {
        let chain = ResolvedChain::empty();
        let text = render(&chain, Path::new("/repo"), None);
        assert!(text.contains("empty"));
    }

    #[test]
    fn render_is_a_pure_function_of_the_chain() {
        let chain = ResolvedChain::new(
            vec![entry(
                ChainRole::Directory { depth: 0 },
                FileKind::Agents,
                "/repo/AGENTS.md",
                "be terse",
            )],
            Vec::new(),
            BTreeSet::new(),
        );
        let first = render(&chain, Path::new("/repo"), None);
        let second = render(&chain, Path::new("/repo"), None);
        assert_eq!(first, second);
    }

    #[test]
    fn render_shows_each_entry_path_relative_to_the_repository_root() {
        let chain = ResolvedChain::new(
            vec![entry(
                ChainRole::Directory { depth: 2 },
                FileKind::Agents,
                "/repo/crates/dark-tools/AGENTS.md",
                "tool rule",
            )],
            Vec::new(),
            BTreeSet::new(),
        );
        let text = render(&chain, Path::new("/repo"), None);
        assert!(text.contains("crates/dark-tools/AGENTS.md"));
        assert!(!text.contains("/repo/crates"));
    }

    #[test]
    fn render_never_shows_the_real_home_directory_for_the_global_entry() {
        let chain = ResolvedChain::new(
            vec![entry(
                ChainRole::Global,
                FileKind::Agents,
                "/home/whoever-runs-this-machine/.darkharness/AGENTS.md",
                "global rule",
            )],
            Vec::new(),
            BTreeSet::new(),
        );
        let text = render(&chain, Path::new("/repo"), None);
        assert!(text.contains("~/.darkharness/AGENTS.md"));
        assert!(!text.contains("whoever-runs-this-machine"));
    }

    #[test]
    fn render_marks_the_first_entry_as_overriding_nothing() {
        let chain = ResolvedChain::new(
            vec![entry(
                ChainRole::Directory { depth: 0 },
                FileKind::Agents,
                "/repo/AGENTS.md",
                "root rule",
            )],
            Vec::new(),
            BTreeSet::new(),
        );
        let text = render(&chain, Path::new("/repo"), None);
        assert!(text.contains("overrides: nothing"));
    }

    #[test]
    fn render_marks_a_later_entry_as_overriding_the_ones_above_it() {
        let chain = ResolvedChain::new(
            vec![
                entry(
                    ChainRole::Global,
                    FileKind::Agents,
                    "/home/x/.darkharness/AGENTS.md",
                    "global rule",
                ),
                entry(
                    ChainRole::Directory { depth: 0 },
                    FileKind::Agents,
                    "/repo/AGENTS.md",
                    "root rule",
                ),
            ],
            Vec::new(),
            BTreeSet::new(),
        );
        let text = render(&chain, Path::new("/repo"), None);
        assert!(text.contains("overrides: the entries above it"));
    }

    #[test]
    fn render_marks_an_override_entry_as_replacing_rather_than_extending() {
        let chain = ResolvedChain::new(
            vec![entry(
                ChainRole::Directory { depth: 1 },
                FileKind::Override,
                "/repo/crates/AGENTS.override.md",
                "override rule",
            )],
            Vec::new(),
            BTreeSet::new(),
        );
        let text = render(&chain, Path::new("/repo"), None);
        assert!(text.contains("replaces the chain so far, it does not extend it"));
    }

    #[test]
    fn render_includes_the_total_token_count() {
        let chain = ResolvedChain::new(
            vec![
                entry(
                    ChainRole::Global,
                    FileKind::Agents,
                    "/home/x/.darkharness/AGENTS.md",
                    "two words",
                ),
                entry(
                    ChainRole::Directory { depth: 0 },
                    FileKind::Agents,
                    "/repo/AGENTS.md",
                    "three more words",
                ),
            ],
            Vec::new(),
            BTreeSet::new(),
        );
        let text = render(&chain, Path::new("/repo"), None);
        assert!(text.contains("5 tokens total"));
    }

    #[test]
    fn render_carries_forward_the_budget_warnings_the_chain_already_has() {
        let chain = ResolvedChain::new(
            vec![entry(
                ChainRole::Directory { depth: 0 },
                FileKind::Agents,
                "/repo/AGENTS.md",
                "root rule",
            )],
            vec!["dropped /repo/far/AGENTS.md (9 tokens): over budget".to_owned()],
            BTreeSet::new(),
        );
        let text = render(&chain, Path::new("/repo"), None);
        assert!(text.contains("Warnings:"));
        assert!(text.contains("dropped /repo/far/AGENTS.md"));
    }

    #[test]
    fn quality_warnings_is_empty_for_a_short_agents_md_and_no_readme() {
        let chain = ResolvedChain::new(
            vec![entry(
                ChainRole::Directory { depth: 0 },
                FileKind::Agents,
                "/repo/AGENTS.md",
                "be terse. use active voice.",
            )],
            Vec::new(),
            BTreeSet::new(),
        );
        assert!(quality_warnings(&chain, None).is_empty());
    }

    #[test]
    fn quality_warnings_fires_the_line_count_warning_over_150_lines() {
        let long_content = "# Heading\n".to_owned() + &"one short line of text\n".repeat(151);
        let chain = ResolvedChain::new(
            vec![entry(
                ChainRole::Directory { depth: 0 },
                FileKind::Agents,
                "/repo/AGENTS.md",
                &long_content,
            )],
            Vec::new(),
            BTreeSet::new(),
        );
        let warnings = quality_warnings(&chain, None);
        assert!(warnings.iter().any(|w| w.contains("150-line")));
    }

    #[test]
    fn quality_warnings_does_not_fire_the_line_count_warning_at_exactly_150_lines() {
        let content = "one short line of text\n".repeat(150);
        let chain = ResolvedChain::new(
            vec![entry(
                ChainRole::Directory { depth: 0 },
                FileKind::Agents,
                "/repo/AGENTS.md",
                &content,
            )],
            Vec::new(),
            BTreeSet::new(),
        );
        let warnings = quality_warnings(&chain, None);
        assert!(!warnings.iter().any(|w| w.contains("150-line")));
    }

    #[test]
    fn quality_warnings_fires_the_overlap_warning_when_readme_repeats_agents_md() {
        let shared = "shared paragraph one two three four five six seven eight nine ten \
                       eleven twelve"
            .to_owned();
        let chain = ResolvedChain::new(
            vec![entry(
                ChainRole::Directory { depth: 0 },
                FileKind::Agents,
                "/repo/AGENTS.md",
                &shared,
            )],
            Vec::new(),
            BTreeSet::new(),
        );
        let warnings = quality_warnings(&chain, Some(&shared));
        assert!(warnings.iter().any(|w| w.contains("overlap")));
    }

    #[test]
    fn quality_warnings_does_not_fire_the_overlap_warning_for_unrelated_text() {
        let agents = "be terse. use active voice. one instruction per sentence always.";
        let readme = "darkharness is a local coding harness with its own inference engine.";
        let chain = ResolvedChain::new(
            vec![entry(
                ChainRole::Directory { depth: 0 },
                FileKind::Agents,
                "/repo/AGENTS.md",
                agents,
            )],
            Vec::new(),
            BTreeSet::new(),
        );
        let warnings = quality_warnings(&chain, Some(readme));
        assert!(!warnings.iter().any(|w| w.contains("overlap")));
    }

    #[test]
    fn quality_warnings_skips_the_overlap_check_when_the_root_file_is_not_agents_md() {
        // The root entry is a CLAUDE.md fallback, not AGENTS.md, so the
        // duplication check (which names AGENTS.md specifically) does not
        // apply even though the content is identical to the README.
        let shared = "identical content in both files for this fixture case only".to_owned();
        let chain = ResolvedChain::new(
            vec![entry(
                ChainRole::Directory { depth: 0 },
                FileKind::Fallback {
                    name: "CLAUDE.md".to_owned(),
                },
                "/repo/CLAUDE.md",
                &shared,
            )],
            Vec::new(),
            BTreeSet::new(),
        );
        let warnings = quality_warnings(&chain, Some(&shared));
        assert!(!warnings.iter().any(|w| w.contains("overlap")));
    }

    #[test]
    fn quality_warnings_returns_nothing_when_the_chain_has_no_root_entry() {
        let chain = ResolvedChain::new(
            vec![entry(
                ChainRole::Global,
                FileKind::Agents,
                "/home/x/.darkharness/AGENTS.md",
                "global rule",
            )],
            Vec::new(),
            BTreeSet::new(),
        );
        assert!(quality_warnings(&chain, Some("anything")).is_empty());
    }

    #[test]
    fn shingle_overlap_of_identical_text_is_one() {
        let text = "one two three four five six seven eight";
        assert!((shingle_overlap(text, text) - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn shingle_overlap_of_disjoint_text_is_zero() {
        let a = "alpha bravo charlie delta echo foxtrot";
        let b = "golf hotel india juliet kilo lima";
        assert!(shingle_overlap(a, b).abs() < f64::EPSILON);
    }

    /// End-to-end fixture: resolves a real chain from files on disk (a
    /// repository-root `AGENTS.md` that is both over 150 lines and heavily
    /// duplicates `README.md`), renders it, and compares the result byte
    /// for byte against a checked-in golden file. The fixture directory
    /// lives under a fresh [`TempDir`] on every run, but [`render`] shows
    /// paths relative to `repo_root`, so the golden file names no
    /// machine-specific path.
    #[test]
    fn render_matches_the_golden_file_and_fires_both_warnings() {
        let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/explain");
        let agents_md = fs::read_to_string(fixtures.join("root_agents.md")).unwrap();
        let readme = fs::read_to_string(fixtures.join("readme.md")).unwrap();
        let golden = fs::read_to_string(fixtures.join("golden.txt")).unwrap();

        let tmp = TempDir::new().unwrap();
        let home = tmp.path().join("home");
        let repo_root = tmp.path().join("repo");
        fs::create_dir_all(&home).unwrap();
        fs::create_dir_all(&repo_root).unwrap();
        fs::write(repo_root.join("AGENTS.md"), &agents_md).unwrap();

        // A generous budget: this fixture exercises K3's quality warnings,
        // not K1's overflow handling, so nothing here should truncate.
        let config = AgentsMdConfig {
            budget_tokens: 10_000,
            ..AgentsMdConfig::default()
        };
        let chain = resolve(&home, &repo_root, &WorkingSet::new(), &config, &count_words).unwrap();

        let text = render(&chain, &repo_root, Some(&readme));
        assert_eq!(text, golden, "explain output must match the golden file");

        let warnings = quality_warnings(&chain, Some(&readme));
        assert!(
            warnings.iter().any(|w| w.contains("overlap")),
            "the overlap warning must fire on this fixture"
        );
        assert!(
            warnings.iter().any(|w| w.contains("150-line")),
            "the line-count warning must fire on this fixture"
        );
    }
}
