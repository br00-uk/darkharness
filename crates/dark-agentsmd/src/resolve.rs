//! Resolves the `AGENTS.md` instruction chain.
//!
//! [`resolve`] runs once, at the start of a turn, and produces a
//! [`ResolvedChain`] whose [`prefix_text`](ResolvedChain::prefix_text)
//! goes into the context prefix (Rule 22). A file that the harness finds
//! later, during the same turn, never touches that already-built chain;
//! [`discover_for_tail`] finds it instead and hands back tail content plus
//! a notice (Rule 23).

use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};

use dark_contract::{ErrCode, Error, Result};

use crate::chain::{ChainEntry, ChainRole, ChainSource, FileKind, ResolvedChain, TailAddition};
use crate::config::{AgentsMdConfig, OnOverflow};
use crate::working_set::WorkingSet;

/// A function that counts the tokens in a string, for the model that will
/// read the resolved chain.
///
/// In production this closure should wrap [`dark_contract::Engine::tokenize`]
/// for whichever role class assembles the turn's prefix; taking a plain
/// closure here, instead of `&dyn Engine`, keeps this crate free of any
/// choice about which role class that is. Never estimate by character
/// count — measure the real tokenizer.
pub type TokenCounter<'a> = &'a dyn Fn(&str) -> usize;

/// Resolves the instruction chain for one turn.
///
/// Reads, in this order, with a later entry winning a conflict:
/// 1. `<home>/.darkharness/AGENTS.md`.
/// 2. `<repo_root>/AGENTS.md`.
/// 3. Each directory between `repo_root` and every directory in
///    `working_set`.
///
/// An `AGENTS.override.md` file in a directory replaces every entry that
/// resolution produced before it; it does not extend them. When no
/// `AGENTS.md` or override exists in a directory, this function falls back
/// to `config.fallback_names`, tried in order, read-only.
///
/// Applies `config.budget_tokens` afterwards: on overflow, it drops the
/// nested entry furthest from the working set, then truncates the
/// repository-root entry at a heading boundary, and records a warning for
/// each change.
///
/// `home` is a parameter, not read from the environment, so a caller can
/// resolve against a fixture directory instead of the real home directory.
///
/// # Errors
///
/// Returns an error when a file that a directory scan found cannot be
/// read (for example, a permission error, or the file vanishing between
/// the scan and the read).
pub fn resolve(
    home: &Path,
    repo_root: &Path,
    working_set: &WorkingSet,
    config: &AgentsMdConfig,
    count_tokens: TokenCounter<'_>,
) -> Result<ResolvedChain> {
    if !config.enabled {
        return Ok(ResolvedChain::empty());
    }

    let root_norm = lexical_normalize(repo_root);
    let global_dir = lexical_normalize(&home.join(".darkharness"));

    let leaf_dirs: Vec<PathBuf> = working_set
        .all_paths()
        .iter()
        .map(|p| lexical_normalize(p))
        .collect();

    let mut nested_dirs: BTreeSet<PathBuf> = BTreeSet::new();
    for dir in &leaf_dirs {
        for d in directory_chain(&root_norm, dir) {
            nested_dirs.insert(d);
        }
    }

    let mut entries: Vec<ChainEntry> = Vec::new();
    let mut known_directories: BTreeSet<PathBuf> = BTreeSet::new();

    if let Some(entry) = build_entry(&global_dir, ChainRole::Global, config, count_tokens)? {
        apply(&mut entries, entry);
    }
    known_directories.insert(global_dir);

    if let Some(entry) = build_entry(
        &root_norm,
        ChainRole::Directory { depth: 0 },
        config,
        count_tokens,
    )? {
        apply(&mut entries, entry);
    }
    known_directories.insert(root_norm.clone());

    let root_depth = root_norm.components().count();
    for dir in &nested_dirs {
        let depth = dir.components().count().saturating_sub(root_depth);
        if let Some(entry) = build_entry(dir, ChainRole::Directory { depth }, config, count_tokens)?
        {
            apply(&mut entries, entry);
        }
        known_directories.insert(dir.clone());
    }

    let mut warnings = Vec::new();
    enforce_budget(
        &mut entries,
        &leaf_dirs,
        config,
        count_tokens,
        &mut warnings,
    );

    Ok(ResolvedChain::new(entries, warnings, known_directories))
}

/// Tracks which directories a turn has already placed in the tail, so a
/// path touched on a later round-trip does not re-notice a directory that
/// an earlier round-trip already added. Build a fresh tracker at the start
/// of each turn.
#[derive(Debug, Clone, Default)]
pub struct TailTracker {
    seen: BTreeSet<PathBuf>,
}

impl TailTracker {
    /// Creates an empty tracker.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

/// Looks for a nested instruction file that governs `touched_path`, that
/// `chain` (the prefix resolved at the start of this turn) does not
/// already cover, and that `tracker` has not already placed in the tail.
///
/// Never modifies `chain`. The prefix stays exactly what [`resolve`] built
/// at the start of the turn, whatever this function finds. See Rule 23.
///
/// # Errors
///
/// Returns an error when a discovered file cannot be read.
pub fn discover_for_tail(
    chain: &ResolvedChain,
    tracker: &mut TailTracker,
    repo_root: &Path,
    touched_path: &Path,
    config: &AgentsMdConfig,
    count_tokens: TokenCounter<'_>,
) -> Result<Vec<TailAddition>> {
    if !config.enabled {
        return Ok(Vec::new());
    }

    let root_norm = lexical_normalize(repo_root);
    let dir_norm = lexical_normalize(touched_path);
    let root_depth = root_norm.components().count();

    let mut additions = Vec::new();
    for dir in directory_chain(&root_norm, &dir_norm) {
        if chain.known_directories().contains(&dir) || tracker.seen.contains(&dir) {
            continue;
        }
        tracker.seen.insert(dir.clone());

        let depth = dir.components().count().saturating_sub(root_depth);
        if let Some(entry) =
            build_entry(&dir, ChainRole::Directory { depth }, config, count_tokens)?
        {
            let notice = format!(
                "found {} for {} during the turn; it joins the tail, not the prefix, so the \
                 prefix stays stable (Rule 23)",
                entry.path.display(),
                dir.display()
            );
            additions.push(TailAddition { entry, notice });
        }
    }
    Ok(additions)
}

/// Pushes `entry`. An override entry first clears every entry that
/// resolution produced before it — it replaces the chain so far, rather
/// than extending it.
fn apply(entries: &mut Vec<ChainEntry>, entry: ChainEntry) {
    if entry.source.kind == FileKind::Override {
        entries.clear();
    }
    entries.push(entry);
}

/// Which file, if any, `find_in_dir` matched.
enum FoundFile {
    Override(PathBuf),
    Agents(PathBuf),
    Fallback(PathBuf, String),
}

/// Looks for an instruction file directly inside `dir`: an override first
/// (when honoured), then `AGENTS.md`, then each fallback name in order.
fn find_in_dir(dir: &Path, config: &AgentsMdConfig) -> Option<FoundFile> {
    if config.honour_overrides {
        let path = dir.join("AGENTS.override.md");
        if path.is_file() {
            return Some(FoundFile::Override(path));
        }
    }

    let path = dir.join("AGENTS.md");
    if path.is_file() {
        return Some(FoundFile::Agents(path));
    }

    for name in &config.fallback_names {
        let path = dir.join(name);
        if path.is_file() {
            return Some(FoundFile::Fallback(path, name.clone()));
        }
    }

    None
}

/// Builds one chain entry for `dir`, or returns `Ok(None)` when `dir` has
/// no instruction file at all.
fn build_entry(
    dir: &Path,
    role: ChainRole,
    config: &AgentsMdConfig,
    count_tokens: TokenCounter<'_>,
) -> Result<Option<ChainEntry>> {
    let Some(found) = find_in_dir(dir, config) else {
        return Ok(None);
    };
    let (path, kind) = match found {
        FoundFile::Override(path) => (path, FileKind::Override),
        FoundFile::Agents(path) => (path, FileKind::Agents),
        FoundFile::Fallback(path, name) => (path, FileKind::Fallback { name }),
    };

    let content = read_file(&path)?;
    let tokens = count_tokens(&content);
    Ok(Some(ChainEntry {
        path,
        source: ChainSource { role, kind },
        directory: dir.to_path_buf(),
        content,
        tokens,
        truncated: false,
    }))
}

/// Reads an instruction file. This crate reads `AGENTS.md`,
/// `AGENTS.override.md`, and every fallback name; it never writes to any
/// of them.
fn read_file(path: &Path) -> Result<String> {
    std::fs::read_to_string(path).map_err(|err| {
        let code = if err.kind() == std::io::ErrorKind::NotFound {
            ErrCode::ToolNotFound
        } else {
            ErrCode::ToolFailed
        };
        Error::new(code, format!("cannot read {}: {err}", path.display()))
    })
}

/// Returns the directories strictly between `root` and `dir`, plus `dir`
/// itself, in top-down order. Returns an empty vector when `dir` equals
/// `root` or does not lie under it.
fn directory_chain(root: &Path, dir: &Path) -> Vec<PathBuf> {
    let Ok(rel) = dir.strip_prefix(root) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut cur = root.to_path_buf();
    for component in rel.components() {
        cur.push(component.as_os_str());
        out.push(cur.clone());
    }
    out
}

/// Normalises `.` and `..` components without touching the filesystem.
///
/// This is lexical, not a substitute for canonicalisation: it does not
/// resolve symlinks and does not require the path to exist, which matters
/// for a path in the person's message that names a file the turn has not
/// created yet.
fn lexical_normalize(path: &Path) -> PathBuf {
    let mut kept: Vec<Component<'_>> = Vec::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if matches!(kept.last(), Some(Component::Normal(_))) {
                    kept.pop();
                } else {
                    kept.push(component);
                }
            }
            other => kept.push(other),
        }
    }
    kept.into_iter().collect()
}

/// The distance, in path components, between two directories: the number
/// of steps up from `a` to their common ancestor, plus the number of steps
/// down from there to `b`.
fn tree_distance(a: &Path, b: &Path) -> usize {
    let ac: Vec<_> = a.components().collect();
    let bc: Vec<_> = b.components().collect();
    let common = ac.iter().zip(bc.iter()).take_while(|(x, y)| x == y).count();
    (ac.len() - common) + (bc.len() - common)
}

/// Finds the nested entry (`depth > 0`) whose directory is furthest from
/// every leaf directory in the working set. Ties break toward the entry
/// whose directory sorts last, since `nested_dirs` iterates in ascending
/// byte order — that keeps the choice deterministic across identical
/// inputs.
fn furthest_nested_index(entries: &[ChainEntry], leaf_dirs: &[PathBuf]) -> Option<usize> {
    entries
        .iter()
        .enumerate()
        .filter(
            |(_, entry)| matches!(entry.source.role, ChainRole::Directory { depth } if depth > 0),
        )
        .max_by_key(|(_, entry)| {
            leaf_dirs
                .iter()
                .map(|leaf| tree_distance(&entry.directory, leaf))
                .min()
                .unwrap_or(usize::MAX)
        })
        .map(|(idx, _)| idx)
}

/// Applies `config.on_overflow` when the chain exceeds `config.budget_tokens`.
fn enforce_budget(
    entries: &mut Vec<ChainEntry>,
    leaf_dirs: &[PathBuf],
    config: &AgentsMdConfig,
    count_tokens: TokenCounter<'_>,
    warnings: &mut Vec<String>,
) {
    // `TruncateWarn` is the only policy today; matching keeps this call
    // site correct if a future policy is added.
    match config.on_overflow {
        OnOverflow::TruncateWarn => {}
    }

    let budget = config.budget_tokens;
    let mut total: usize = entries.iter().map(|entry| entry.tokens).sum();
    if total <= budget {
        return;
    }

    // Step 1: drop the nested file furthest from the working set, one at a
    // time, until the chain fits or no nested file remains.
    while total > budget {
        let Some(idx) = furthest_nested_index(entries, leaf_dirs) else {
            break;
        };
        let dropped = entries.remove(idx);
        total -= dropped.tokens;
        warnings.push(format!(
            "dropped {} ({} tokens): the AGENTS.md chain exceeded its {budget}-token budget",
            dropped.path.display(),
            dropped.tokens
        ));
    }

    // Step 2: truncate the repository-root file at a heading boundary.
    if total > budget
        && let Some(root_idx) = entries
            .iter()
            .position(|entry| matches!(entry.source.role, ChainRole::Directory { depth: 0 }))
    {
        let others = total - entries[root_idx].tokens;
        let target = budget.saturating_sub(others);
        let before = entries[root_idx].tokens;
        let (new_content, did_truncate) =
            truncate_at_boundary(&entries[root_idx].content, target, count_tokens);
        if did_truncate {
            entries[root_idx].content = new_content;
            entries[root_idx].tokens = count_tokens(&entries[root_idx].content);
            entries[root_idx].truncated = true;
            total = others + entries[root_idx].tokens;
            warnings.push(format!(
                "truncated {} at a heading boundary: {before} tokens to {} tokens",
                entries[root_idx].path.display(),
                entries[root_idx].tokens
            ));
        }
    }

    // Step 3: warn, naming every file and its token count.
    if total > budget {
        let sizes: Vec<String> = entries
            .iter()
            .map(|entry| format!("{} ({} tokens)", entry.path.display(), entry.tokens))
            .collect();
        warnings.push(format!(
            "the AGENTS.md chain still uses {total} tokens after dropping nested files and \
             truncating the root file; the {budget}-token budget stands. Remaining files: {}",
            sizes.join(", ")
        ));
    }
}

/// Truncates `content` to at most `max_tokens`, preferring the last
/// markdown heading boundary that fits. Falls back to a paragraph
/// boundary, then a sentence boundary. Every candidate boundary sits
/// outside an open code fence, so the result never ends mid-sentence and
/// never leaves a fence unclosed (task unit K1, step 9).
fn truncate_at_boundary(
    content: &str,
    max_tokens: usize,
    count_tokens: TokenCounter<'_>,
) -> (String, bool) {
    if count_tokens(content) <= max_tokens {
        return (content.to_owned(), false);
    }

    let boundaries = safe_boundaries(content);
    for candidates in [
        &boundaries.headings,
        &boundaries.paragraphs,
        &boundaries.sentences,
    ] {
        let fit = candidates.iter().rev().find(|&&cut| {
            cut > 0 && cut < content.len() && count_tokens(&content[..cut]) <= max_tokens
        });
        if let Some(&cut) = fit {
            let mut truncated = content[..cut].trim_end().to_owned();
            truncated.push_str("\n\n<!-- truncated: over the agents_md token budget -->\n");
            return (truncated, true);
        }
    }

    // No safe boundary fits the budget at all. Dropping the content
    // entirely is still safer than an unsafe cut.
    (String::new(), true)
}

/// Byte offsets, ascending, where truncating `content` is safe: right
/// before a heading line, right before a paragraph, and right after a
/// line that ends a sentence. Every offset sits outside an open code
/// fence.
struct Boundaries {
    headings: Vec<usize>,
    paragraphs: Vec<usize>,
    sentences: Vec<usize>,
}

fn safe_boundaries(content: &str) -> Boundaries {
    let mut headings = Vec::new();
    let mut paragraphs = Vec::new();
    let mut sentences = Vec::new();

    let mut in_fence = false;
    let mut fence_marker = "";
    let mut prev_line_blank = false;
    let mut offset = 0usize;

    for line in content.split_inclusive('\n') {
        let bare = line.trim_end_matches('\n');
        let trimmed = bare.trim_start();

        let is_fence_line = trimmed.starts_with("```") || trimmed.starts_with("~~~");
        if is_fence_line {
            let marker = &trimmed[..3];
            if in_fence {
                if trimmed.starts_with(fence_marker) {
                    in_fence = false;
                }
            } else {
                in_fence = true;
                fence_marker = marker;
            }
        } else if !in_fence {
            if is_atx_heading(trimmed) {
                headings.push(offset);
            }
            if prev_line_blank && !trimmed.is_empty() {
                paragraphs.push(offset);
            }
            if bare
                .trim_end()
                .chars()
                .last()
                .is_some_and(|c| matches!(c, '.' | '!' | '?'))
            {
                sentences.push(offset + line.len());
            }
        }

        prev_line_blank = trimmed.is_empty();
        offset += line.len();
    }

    Boundaries {
        headings,
        paragraphs,
        sentences,
    }
}

/// Returns `true` when `trimmed` (already left-trimmed) opens an ATX
/// heading: one to six `#` characters followed by a space.
fn is_atx_heading(trimmed: &str) -> bool {
    let hashes = trimmed.chars().take_while(|&c| c == '#').count();
    (1..=6).contains(&hashes) && trimmed.as_bytes().get(hashes) == Some(&b' ')
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    /// A test-only counter. Never estimate tokens by character count in
    /// production code (Rule from task unit A3) — this whitespace count
    /// stands in for a real tokenizer in these fixtures only.
    fn count_words(text: &str) -> usize {
        text.split_whitespace().count()
    }

    struct Fixture {
        _tmp: TempDir,
        home: PathBuf,
        repo_root: PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            let tmp = TempDir::new().expect("tempdir");
            let home = tmp.path().join("home");
            let repo_root = tmp.path().join("repo");
            fs::create_dir_all(&home).unwrap();
            fs::create_dir_all(&repo_root).unwrap();
            Self {
                _tmp: tmp,
                home,
                repo_root,
            }
        }

        fn write(&self, rel: &str, content: &str) {
            let path = self.repo_root.join(rel);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, content).unwrap();
        }

        fn write_home(&self, rel: &str, content: &str) {
            let path = self.home.join(rel);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, content).unwrap();
        }
    }

    #[test]
    fn resolves_global_then_root_in_order() {
        let fx = Fixture::new();
        fx.write_home(".darkharness/AGENTS.md", "global rule");
        fx.write("AGENTS.md", "root rule");

        let chain = resolve(
            &fx.home,
            &fx.repo_root,
            &WorkingSet::new(),
            &AgentsMdConfig::default(),
            &count_words,
        )
        .unwrap();

        let sources: Vec<_> = chain.entries().iter().map(|e| e.source.role).collect();
        assert_eq!(
            sources,
            vec![ChainRole::Global, ChainRole::Directory { depth: 0 }]
        );
    }

    #[test]
    fn nested_directories_join_the_chain_for_a_working_set_path() {
        let fx = Fixture::new();
        fx.write("AGENTS.md", "root rule");
        fx.write("crates/dark-tools/AGENTS.md", "tool rule");

        let mut ws = WorkingSet::new();
        ws.ticket_scope.push(fx.repo_root.join("crates/dark-tools"));

        let chain = resolve(
            &fx.home,
            &fx.repo_root,
            &ws,
            &AgentsMdConfig::default(),
            &count_words,
        )
        .unwrap();

        // Two files exist, so the chain holds two entries: the repository
        // root and crates/dark-tools. `crates/` lies between them and joins
        // the walk, but it holds no AGENTS.md, so it contributes nothing.
        assert_eq!(chain.entries().len(), 2);
        let last = chain.entries().last().unwrap();
        assert!(last.content.contains("tool rule"));
        assert_eq!(last.source.role, ChainRole::Directory { depth: 2 });
    }

    #[test]
    fn a_missing_chain_produces_no_entries() {
        let fx = Fixture::new();
        let chain = resolve(
            &fx.home,
            &fx.repo_root,
            &WorkingSet::new(),
            &AgentsMdConfig::default(),
            &count_words,
        )
        .unwrap();
        assert!(chain.entries().is_empty());
        assert_eq!(chain.prefix_text(), "");
    }

    #[test]
    fn disabled_resolver_returns_an_empty_chain() {
        let fx = Fixture::new();
        fx.write("AGENTS.md", "root rule");
        let config = AgentsMdConfig {
            enabled: false,
            ..AgentsMdConfig::default()
        };

        let chain = resolve(
            &fx.home,
            &fx.repo_root,
            &WorkingSet::new(),
            &config,
            &count_words,
        )
        .unwrap();
        assert!(chain.entries().is_empty());
    }

    #[test]
    fn override_replaces_everything_above_it_rather_than_extending_it() {
        let fx = Fixture::new();
        fx.write_home(".darkharness/AGENTS.md", "global rule");
        fx.write("AGENTS.md", "root rule");
        fx.write("crates/dark-tools/AGENTS.override.md", "override rule only");

        let mut ws = WorkingSet::new();
        ws.ticket_scope.push(fx.repo_root.join("crates/dark-tools"));

        let chain = resolve(
            &fx.home,
            &fx.repo_root,
            &ws,
            &AgentsMdConfig::default(),
            &count_words,
        )
        .unwrap();

        // The override is the only entry: it replaced global and root, it
        // did not extend them.
        assert_eq!(chain.entries().len(), 1);
        assert_eq!(chain.entries()[0].content, "override rule only");
        assert_eq!(chain.entries()[0].source.kind, FileKind::Override);
    }

    #[test]
    fn override_is_ignored_when_honour_overrides_is_off() {
        let fx = Fixture::new();
        fx.write("AGENTS.md", "root rule");
        fx.write("AGENTS.override.md", "should not apply");
        let config = AgentsMdConfig {
            honour_overrides: false,
            ..AgentsMdConfig::default()
        };

        let chain = resolve(
            &fx.home,
            &fx.repo_root,
            &WorkingSet::new(),
            &config,
            &count_words,
        )
        .unwrap();

        assert_eq!(chain.entries().len(), 1);
        assert_eq!(chain.entries()[0].content, "root rule");
    }

    #[test]
    fn falls_back_to_claude_md_then_gemini_md() {
        let fx = Fixture::new();
        fx.write("CLAUDE.md", "claude rule");
        fx.write("crates/x/GEMINI.md", "gemini rule");

        let mut ws = WorkingSet::new();
        ws.ticket_scope.push(fx.repo_root.join("crates/x"));

        let chain = resolve(
            &fx.home,
            &fx.repo_root,
            &ws,
            &AgentsMdConfig::default(),
            &count_words,
        )
        .unwrap();

        assert_eq!(chain.entries().len(), 2);
        assert_eq!(chain.entries()[0].content, "claude rule");
        assert_eq!(
            chain.entries()[0].source.kind,
            FileKind::Fallback {
                name: "CLAUDE.md".to_owned()
            }
        );
        assert_eq!(chain.entries()[1].content, "gemini rule");
    }

    #[test]
    fn agents_md_is_preferred_over_the_fallback_names() {
        let fx = Fixture::new();
        fx.write("AGENTS.md", "agents rule");
        fx.write("CLAUDE.md", "claude rule");

        let chain = resolve(
            &fx.home,
            &fx.repo_root,
            &WorkingSet::new(),
            &AgentsMdConfig::default(),
            &count_words,
        )
        .unwrap();

        assert_eq!(chain.entries().len(), 1);
        assert_eq!(chain.entries()[0].content, "agents rule");
    }

    #[test]
    fn overflow_drops_the_nested_file_furthest_from_the_working_set_and_warns() {
        let fx = Fixture::new();
        fx.write("AGENTS.md", "root");
        fx.write("near/AGENTS.md", "near content near content");
        fx.write(
            "near/deep/far/AGENTS.md",
            "far content far content far content far",
        );

        let mut ws = WorkingSet::new();
        // The working set centres on `near`, so `near/deep/far` is the
        // more distant nested file.
        ws.ticket_scope.push(fx.repo_root.join("near"));
        ws.ticket_scope.push(fx.repo_root.join("near/deep/far"));

        let config = AgentsMdConfig {
            budget_tokens: 5,
            ..AgentsMdConfig::default()
        }; // "root" (1) + "near..." (4) = 5, no room for "far..."

        let chain = resolve(&fx.home, &fx.repo_root, &ws, &config, &count_words).unwrap();

        let paths: Vec<_> = chain.entries().iter().map(|e| e.path.clone()).collect();
        assert!(
            !paths.iter().any(|p| p.ends_with("far/AGENTS.md")),
            "far file should be dropped"
        );
        assert!(
            paths.iter().any(|p| p.ends_with("near/AGENTS.md")),
            "near file should survive"
        );
        assert!(
            chain
                .warnings()
                .iter()
                .any(|w| w.contains("far") && w.contains("dropped")),
            "warning should name the dropped file: {:?}",
            chain.warnings()
        );
    }

    #[test]
    fn overflow_truncates_the_root_file_at_a_heading_boundary_when_nested_files_are_gone() {
        let fx = Fixture::new();
        let root_content = "# Intro\nshort intro text here\n\n# Rules\nmany many many many many many many many words in this section that push it well over budget\n";
        fx.write("AGENTS.md", root_content);

        // The only heading boundary sits before "# Rules", and everything
        // above it is six words: "#", "Intro", "short", "intro", "text",
        // "here". The budget has to admit those six for a cut there to be
        // possible at all; below that the whole file is over budget and
        // there is nothing safe to keep.
        let config = AgentsMdConfig {
            budget_tokens: 6,
            ..AgentsMdConfig::default()
        };

        let chain = resolve(
            &fx.home,
            &fx.repo_root,
            &WorkingSet::new(),
            &config,
            &count_words,
        )
        .unwrap();

        assert_eq!(chain.entries().len(), 1);
        let entry = &chain.entries()[0];
        assert!(entry.truncated);
        assert!(entry.content.contains("# Intro"));
        assert!(
            !entry.content.contains("# Rules"),
            "must cut before the next heading"
        );
        assert!(
            chain.warnings().iter().any(|w| w.contains("truncated")),
            "warning should mention truncation: {:?}",
            chain.warnings()
        );
    }

    #[test]
    fn truncation_never_lands_inside_a_code_fence() {
        let fx = Fixture::new();
        let root_content = "# Title\nintro words here padding padding padding\n\n```\nfence line one\nfence line two\nfence line three\n```\n\nafter fence text that would only be reached if we cut past the fence safely.\n";
        fx.write("AGENTS.md", root_content);

        let config = AgentsMdConfig {
            budget_tokens: 6,
            ..AgentsMdConfig::default()
        }; // small enough to force truncation before the fence content fits

        let chain = resolve(
            &fx.home,
            &fx.repo_root,
            &WorkingSet::new(),
            &config,
            &count_words,
        )
        .unwrap();

        let content = &chain.entries()[0].content;
        // Any code fence marker that appears must appear an even number of
        // times: never left open.
        let fence_count = content.matches("```").count();
        assert_eq!(
            fence_count % 2,
            0,
            "content must not end with an unclosed fence: {content:?}"
        );
    }

    #[test]
    fn truncation_never_lands_mid_sentence() {
        let fx = Fixture::new();
        let root_content = "This is one full sentence that ends cleanly. This is another full sentence that also ends cleanly and is quite a bit longer than the first one so that a naive cut would land inside it.";
        fx.write("AGENTS.md", root_content);

        let config = AgentsMdConfig {
            budget_tokens: 9,
            ..AgentsMdConfig::default()
        };

        let chain = resolve(
            &fx.home,
            &fx.repo_root,
            &WorkingSet::new(),
            &config,
            &count_words,
        )
        .unwrap();

        let content = chain.entries()[0].content.trim_end();
        let before_marker = content.split("<!-- truncated").next().unwrap().trim_end();
        assert!(
            before_marker.is_empty() || before_marker.ends_with('.'),
            "truncated content must end at a sentence boundary: {before_marker:?}"
        );
    }

    #[test]
    fn discover_for_tail_ignores_a_directory_the_prefix_already_covers() {
        let fx = Fixture::new();
        fx.write("crates/x/AGENTS.md", "x rule");

        let mut ws = WorkingSet::new();
        ws.ticket_scope.push(fx.repo_root.join("crates/x"));
        let chain = resolve(
            &fx.home,
            &fx.repo_root,
            &ws,
            &AgentsMdConfig::default(),
            &count_words,
        )
        .unwrap();

        let mut tracker = TailTracker::new();
        let additions = discover_for_tail(
            &chain,
            &mut tracker,
            &fx.repo_root,
            &fx.repo_root.join("crates/x"),
            &AgentsMdConfig::default(),
            &count_words,
        )
        .unwrap();
        assert!(
            additions.is_empty(),
            "a directory already in the prefix must not re-appear in the tail"
        );
    }

    #[test]
    fn discover_for_tail_finds_a_directory_the_prefix_never_saw() {
        let fx = Fixture::new();
        fx.write("crates/y/AGENTS.md", "y rule");
        let chain = resolve(
            &fx.home,
            &fx.repo_root,
            &WorkingSet::new(),
            &AgentsMdConfig::default(),
            &count_words,
        )
        .unwrap();

        let mut tracker = TailTracker::new();
        let additions = discover_for_tail(
            &chain,
            &mut tracker,
            &fx.repo_root,
            &fx.repo_root.join("crates/y"),
            &AgentsMdConfig::default(),
            &count_words,
        )
        .unwrap();

        assert_eq!(additions.len(), 1);
        assert!(additions[0].entry.content.contains("y rule"));
        assert!(additions[0].notice.contains("tail"));
    }

    #[test]
    fn discover_for_tail_does_not_repeat_a_notice_across_round_trips() {
        let fx = Fixture::new();
        fx.write("crates/y/AGENTS.md", "y rule");
        let chain = resolve(
            &fx.home,
            &fx.repo_root,
            &WorkingSet::new(),
            &AgentsMdConfig::default(),
            &count_words,
        )
        .unwrap();
        let mut tracker = TailTracker::new();

        let first = discover_for_tail(
            &chain,
            &mut tracker,
            &fx.repo_root,
            &fx.repo_root.join("crates/y"),
            &AgentsMdConfig::default(),
            &count_words,
        )
        .unwrap();
        let second = discover_for_tail(
            &chain,
            &mut tracker,
            &fx.repo_root,
            &fx.repo_root.join("crates/y"),
            &AgentsMdConfig::default(),
            &count_words,
        )
        .unwrap();

        assert_eq!(first.len(), 1);
        assert!(
            second.is_empty(),
            "the second round-trip must not repeat the first's notice"
        );
    }
}
