//! The resolved instruction chain and its rendering into prefix text.

use std::collections::BTreeSet;
use std::path::PathBuf;

/// The fixed header that [`ResolvedChain::prefix_text`] puts ahead of every
/// non-empty chain. It states the precedence a reader needs: this chain is
/// repository and effort policy; a narrower source overrides it. See task
/// unit K1, steps 10 and 11.
const PREAMBLE: &str = "<!-- AGENTS.md chain: resolved once at the start of this turn and held \
fixed for every round-trip in it. Entries below appear in resolution order; when two entries \
disagree, the entry listed later wins. A wayfinder map note narrows this chain, and the \
person's current message overrides both. -->";

/// The structural role that an entry plays in the resolution order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ChainRole {
    /// `~/.darkharness/AGENTS.md`, read before every repository-specific
    /// file.
    Global,
    /// A directory between the repository root and a working-set
    /// directory. `depth` is `0` for the repository root itself and grows
    /// by one per nested directory below it.
    Directory {
        /// Path components between the repository root and this
        /// directory.
        depth: usize,
    },
}

impl std::fmt::Display for ChainRole {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Global => write!(f, "global"),
            Self::Directory { depth: 0 } => write!(f, "repository root"),
            Self::Directory { depth } => write!(f, "nested, depth {depth}"),
        }
    }
}

/// Which file matched in a directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileKind {
    /// `AGENTS.md`.
    Agents,
    /// `AGENTS.override.md`. Replaces every entry that resolution produced
    /// before it; it does not extend them. This file name is a
    /// darkharness-only convention: mark it non-portable when you
    /// document it, since another AGENTS.md reader will not recognise it.
    Override,
    /// A fallback file, read because the directory has neither
    /// `AGENTS.md` nor `AGENTS.override.md`.
    Fallback {
        /// The file name that matched, for example `CLAUDE.md`.
        name: String,
    },
}

impl std::fmt::Display for FileKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Agents => write!(f, "AGENTS.md"),
            Self::Override => write!(f, "AGENTS.override.md"),
            Self::Fallback { name } => write!(f, "fallback {name}"),
        }
    }
}

/// Where one entry in the resolved chain came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChainSource {
    /// The entry's place in the resolution order.
    pub role: ChainRole,
    /// The file that matched.
    pub kind: FileKind,
}

/// One file in the resolved chain.
#[derive(Debug, Clone, PartialEq)]
pub struct ChainEntry {
    /// The file's location on disk.
    pub path: PathBuf,
    /// Where this entry sits in the resolution order.
    pub source: ChainSource,
    /// The directory that this entry governs. Recorded separately from
    /// `path` so overflow scoring does not need to call `Path::parent`.
    pub directory: PathBuf,
    /// The file content, after any truncation.
    pub content: String,
    /// The token count of `content`, from the caller's counter.
    pub tokens: usize,
    /// `true` when overflow handling shortened this entry.
    pub truncated: bool,
}

/// One nested file that the harness found during a turn, after the prefix
/// was already built. It joins the tail, never the prefix. See Rule 23.
#[derive(Debug, Clone, PartialEq)]
pub struct TailAddition {
    /// The discovered file.
    pub entry: ChainEntry,
    /// The notice to show, naming the file and the subtree it governs.
    pub notice: String,
}

/// The instruction chain, resolved once at the start of a turn.
///
/// Call [`prefix_text`](Self::prefix_text) to render it for the prefix.
/// Rendering is a pure function of the entries, so the same
/// `ResolvedChain` renders identical bytes on every call — that is what
/// keeps the prefix stable across a turn's round-trips. See Rule 22.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedChain {
    entries: Vec<ChainEntry>,
    warnings: Vec<String>,
    known_directories: BTreeSet<PathBuf>,
}

impl ResolvedChain {
    /// Builds a resolved chain from its parts.
    pub(crate) fn new(
        entries: Vec<ChainEntry>,
        warnings: Vec<String>,
        known_directories: BTreeSet<PathBuf>,
    ) -> Self {
        Self {
            entries,
            warnings,
            known_directories,
        }
    }

    /// Returns the chain that a disabled resolver, or a resolver that
    /// found no instruction file anywhere, produces.
    #[must_use]
    pub fn empty() -> Self {
        Self {
            entries: Vec::new(),
            warnings: Vec::new(),
            known_directories: BTreeSet::new(),
        }
    }

    /// Returns the entries, in resolution order.
    #[must_use]
    pub fn entries(&self) -> &[ChainEntry] {
        &self.entries
    }

    /// Returns the warnings that overflow handling produced, in the order
    /// it produced them.
    #[must_use]
    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }

    /// Returns the total token count across every entry.
    #[must_use]
    pub fn total_tokens(&self) -> usize {
        self.entries.iter().map(|entry| entry.tokens).sum()
    }

    /// Returns the directories that this resolution already looked at,
    /// whether or not it found a file there. A directory in this set never
    /// produces a tail notice — the prefix already accounts for it.
    pub(crate) fn known_directories(&self) -> &BTreeSet<PathBuf> {
        &self.known_directories
    }

    /// Renders the chain for the context prefix.
    ///
    /// Returns an empty string when the chain has no entries, so a
    /// disabled resolver, or a repository with no instruction file
    /// anywhere, adds nothing to the prefix.
    #[must_use]
    pub fn prefix_text(&self) -> String {
        if self.entries.is_empty() {
            return String::new();
        }
        let mut out = String::from(PREAMBLE);
        for entry in &self.entries {
            out.push_str("\n\n<!-- agentsmd: ");
            out.push_str(&entry.source.role.to_string());
            out.push_str(" / ");
            out.push_str(&entry.source.kind.to_string());
            out.push_str(" / ");
            out.push_str(&entry.path.to_string_lossy());
            out.push_str(" -->\n");
            out.push_str(&entry.content);
        }
        out
    }
}

/// Renders a set of tail additions for the turn tail.
///
/// Unlike [`ResolvedChain::prefix_text`], this is not required to stay
/// stable across round-trips: the tail is exactly the part of the context
/// that grows during a turn. See Rule 8.
#[must_use]
pub fn tail_text(additions: &[TailAddition]) -> String {
    let mut out = String::new();
    for addition in additions {
        if !out.is_empty() {
            out.push_str("\n\n");
        }
        out.push_str("<!-- agentsmd notice: ");
        out.push_str(&addition.notice);
        out.push_str(" -->\n");
        out.push_str(&addition.entry.content);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(role: ChainRole, kind: FileKind, content: &str) -> ChainEntry {
        ChainEntry {
            path: PathBuf::from("/repo/AGENTS.md"),
            source: ChainSource { role, kind },
            directory: PathBuf::from("/repo"),
            content: content.to_owned(),
            tokens: content.split_whitespace().count(),
            truncated: false,
        }
    }

    #[test]
    fn empty_chain_renders_no_prefix_text() {
        assert_eq!(ResolvedChain::empty().prefix_text(), "");
    }

    #[test]
    fn prefix_text_is_stable_across_calls() {
        let chain = ResolvedChain::new(
            vec![entry(
                ChainRole::Directory { depth: 0 },
                FileKind::Agents,
                "be terse",
            )],
            Vec::new(),
            BTreeSet::new(),
        );
        assert_eq!(chain.prefix_text(), chain.prefix_text());
    }

    #[test]
    fn prefix_text_carries_every_entry_in_order() {
        let chain = ResolvedChain::new(
            vec![
                entry(ChainRole::Global, FileKind::Agents, "global rule"),
                entry(
                    ChainRole::Directory { depth: 0 },
                    FileKind::Agents,
                    "root rule",
                ),
            ],
            Vec::new(),
            BTreeSet::new(),
        );
        let text = chain.prefix_text();
        let global_at = text.find("global rule").expect("global rule present");
        let root_at = text.find("root rule").expect("root rule present");
        assert!(
            global_at < root_at,
            "global entry must render before the root entry"
        );
    }
}
