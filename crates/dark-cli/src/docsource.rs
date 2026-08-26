//! Which vendor documentation a target language and pattern call for.
//!
//! # The tension this sits in
//!
//! Every other part of `dark refactor` is local. This is not: fetching
//! documentation needs the network, at a moment that is not `dark setup`,
//! which is exactly what the primary requirement exists to avoid.
//!
//! So it is designed to be refusable and visible rather than convenient.
//! The table below **names** sources; it never fetches one. `dark
//! refactor` prints what it would fetch, with the source of each, and
//! fetches only when a person says so. Dark mode refuses the step outright
//! rather than skipping it quietly, because a person who asked for the
//! documentation and did not get it should be told.
//!
//! # Why the table is data
//!
//! Library ecosystems move faster than this binary is rebuilt. The
//! built-in table is a starting point, and
//! `$DARK_HOME/doc-sources.toml` replaces it, so a wrong or stale entry
//! is a text edit rather than a release.

use std::path::Path;

use serde::Deserialize;

use crate::refactor::Pattern;

/// One documentation pack worth fetching.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(crate) struct DocSource {
    /// The library this documents.
    pub(crate) name: String,
    /// Where it comes from, as `dark pack add --source-kind` names it.
    pub(crate) kind: String,
    /// The address to fetch, whatever `kind` means by one.
    pub(crate) source: String,
    /// Why this is being suggested, in one line.
    pub(crate) why: String,
}

/// The whole table, as it is read from disk.
#[derive(Debug, Clone, Default, Deserialize)]
pub(crate) struct DocTable {
    /// Entries, each tagged with the language and patterns it serves.
    #[serde(default)]
    entries: Vec<Entry>,
}

/// One row: a source, and when it applies.
#[derive(Debug, Clone, Deserialize)]
struct Entry {
    /// The target language this serves.
    language: String,
    /// The patterns this serves. Empty means every pattern.
    #[serde(default)]
    patterns: Vec<String>,
    /// The source itself.
    #[serde(flatten)]
    source: DocSource,
}

/// The built-in table.
///
/// Deliberately short. A long list of every library in an ecosystem is a
/// list nobody reads and a fetch nobody wants; these are the ones the
/// pattern actually implies.
const BUILTIN: &str = r#"
[[entries]]
language = "rust"
patterns = ["service split", "event-driven"]
name = "tokio"
kind = "docsrs"
source = "tokio"
why = "the runtime every Rust service is built on"

[[entries]]
language = "rust"
patterns = ["service split"]
name = "axum"
kind = "docsrs"
source = "axum"
why = "the HTTP layer a Rust service usually exposes"

[[entries]]
language = "rust"
patterns = ["service split"]
name = "tonic"
kind = "docsrs"
source = "tonic"
why = "gRPC between services, when HTTP is not the boundary"

[[entries]]
language = "rust"
patterns = ["plugin architecture"]
name = "trait-objects"
kind = "docsrs"
source = "anyhow"
why = "error handling across a plugin boundary"

[[entries]]
language = "go"
patterns = ["service split", "event-driven"]
name = "go-std"
kind = "sitemap"
source = "https://pkg.go.dev/std"
why = "the standard library a Go service leans on"

[[entries]]
language = "typescript"
patterns = ["service split"]
name = "node"
kind = "sitemap"
source = "https://nodejs.org/docs/latest/api/"
why = "the runtime a TypeScript service uses"

[[entries]]
language = "python"
patterns = ["service split", "event-driven"]
name = "python-std"
kind = "sitemap"
source = "https://docs.python.org/3/library/"
why = "the standard library a Python service leans on"
"#;

impl DocTable {
    /// The built-in table.
    ///
    /// # Panics
    ///
    /// Never: the built-in text is a constant this crate's own tests
    /// parse.
    #[must_use]
    pub(crate) fn builtin() -> Self {
        toml::from_str(BUILTIN).unwrap_or_default()
    }

    /// Reads `$DARK_HOME/doc-sources.toml`, falling back to the built-in
    /// table when there is none.
    ///
    /// # Errors
    ///
    /// Returns an error when the file exists and does not parse. A wrong
    /// table is worth reporting: silently using the built-in one instead
    /// would hide the person's edit.
    pub(crate) fn load(dark_home: &Path) -> anyhow::Result<Self> {
        let path = dark_home.join("doc-sources.toml");
        match std::fs::read_to_string(&path) {
            Ok(text) => toml::from_str(&text)
                .map_err(|err| anyhow::anyhow!("cannot parse {}: {err}", path.display())),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(Self::builtin()),
            Err(err) => Err(anyhow::anyhow!("cannot read {}: {err}", path.display())),
        }
    }

    /// The sources for one language and pattern, in table order.
    #[must_use]
    pub(crate) fn for_target(&self, language: &str, pattern: Pattern) -> Vec<&DocSource> {
        self.entries
            .iter()
            .filter(|entry| entry.language.eq_ignore_ascii_case(language))
            .filter(|entry| {
                entry.patterns.is_empty()
                    || entry.patterns.iter().any(|name| name == pattern.name())
            })
            .map(|entry| &entry.source)
            .collect()
    }
}

/// Renders what would be fetched, and the command that does it.
///
/// Nothing is fetched here. The person reads this, and runs the commands
/// they want.
#[must_use]
pub(crate) fn render(sources: &[&DocSource], dark: bool) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();

    if sources.is_empty() {
        return "no documentation packs are suggested for this target.\n".to_owned();
    }

    if dark {
        let _ = writeln!(
            out,
            "dark mode is on, so no documentation can be fetched. These are what this target \
             calls for; run dark golight first if you want them:"
        );
    } else {
        let _ = writeln!(
            out,
            "these documentation packs suit this target. Each needs the network, so none is \
             fetched until you run its command:"
        );
    }
    let _ = writeln!(out);

    for source in sources {
        let _ = writeln!(out, "  {} — {}", source.name, source.why);
        let _ = writeln!(
            out,
            "    dark pack add {} --source-kind {} --name {}",
            source.source, source.kind, source.name
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn the_builtin_table_parses() {
        // `builtin` swallows a parse failure into an empty table, so an
        // unparseable constant would show up as silence rather than a
        // panic. This is what catches it.
        assert!(
            !DocTable::builtin().entries.is_empty(),
            "the built-in table did not parse"
        );
    }

    #[test]
    fn a_target_gets_the_sources_tagged_for_its_pattern() {
        let table = DocTable::builtin();
        let names: Vec<&str> = table
            .for_target("Rust", Pattern::ServiceSplit)
            .iter()
            .map(|source| source.name.as_str())
            .collect();
        assert!(names.contains(&"axum"), "{names:?}");
        assert!(names.contains(&"tonic"), "{names:?}");
    }

    #[test]
    fn a_pattern_that_shares_nothing_gets_a_different_list() {
        let table = DocTable::builtin();
        let split: Vec<&str> = table
            .for_target("Rust", Pattern::ServiceSplit)
            .iter()
            .map(|s| s.name.as_str())
            .collect();
        let plugin: Vec<&str> = table
            .for_target("Rust", Pattern::Plugin)
            .iter()
            .map(|s| s.name.as_str())
            .collect();
        assert_ne!(split, plugin);
    }

    #[test]
    fn a_language_with_no_entries_suggests_nothing() {
        assert!(
            DocTable::builtin()
                .for_target("Ruby", Pattern::ServiceSplit)
                .is_empty()
        );
    }

    #[test]
    fn the_language_match_ignores_case() {
        // The table is written in `Language::name`'s own lower case, but
        // a person editing it should not have to know that.
        assert!(
            !DocTable::builtin()
                .for_target("Rust", Pattern::ServiceSplit)
                .is_empty()
        );
    }

    #[test]
    fn nothing_is_fetched_by_rendering() {
        // The whole design: this prints commands, it does not run them.
        let table = DocTable::builtin();
        let sources = table.for_target("Rust", Pattern::ServiceSplit);
        let text = render(&sources, false);
        assert!(text.contains("dark pack add"), "{text}");
        assert!(text.contains("none is fetched until you run"), "{text}");
    }

    #[test]
    fn dark_mode_says_the_fetch_cannot_happen_rather_than_skipping_it() {
        let table = DocTable::builtin();
        let sources = table.for_target("Rust", Pattern::ServiceSplit);
        let text = render(&sources, true);
        assert!(text.contains("dark mode is on"), "{text}");
        assert!(text.contains("dark golight"), "{text}");
    }

    #[test]
    fn an_empty_suggestion_says_so() {
        assert!(render(&[], false).contains("no documentation packs"));
    }

    #[test]
    fn a_table_on_disk_replaces_the_builtin_one() {
        let dir = TempDir::new().unwrap();
        std::fs::write(
            dir.path().join("doc-sources.toml"),
            r#"
[[entries]]
language = "rust"
patterns = ["service split"]
name = "mine"
kind = "localdir"
source = "/opt/docs"
why = "our own"
"#,
        )
        .unwrap();
        let table = DocTable::load(dir.path()).unwrap();
        let sources = table.for_target("Rust", Pattern::ServiceSplit);
        assert_eq!(sources.len(), 1);
        assert_eq!(sources[0].name, "mine");
    }

    #[test]
    fn a_broken_table_is_reported_not_silently_replaced() {
        // Silently falling back would hide the person's edit and fetch
        // documentation they did not ask for.
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("doc-sources.toml"), "not toml {{{").unwrap();
        assert!(DocTable::load(dir.path()).is_err());
    }

    #[test]
    fn no_table_falls_back_to_the_builtin_one() {
        let dir = TempDir::new().unwrap();
        assert!(!DocTable::load(dir.path()).unwrap().entries.is_empty());
    }
}
