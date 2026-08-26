//! The house style, derived from what extraction already found.
//!
//! # Why this is counted rather than asked
//!
//! Telling an agent "match the existing code style" needs a description of
//! that style. The obvious way to get one is to ask a model to read the
//! repository and write it down. That is slow, costs a context window,
//! changes between runs, and is wrong often enough to be dangerous — an
//! agent told the wrong convention applies it to every file it touches.
//!
//! Almost none of it needs a model. [`crate::extract`] already records
//! every definition's name, kind, export status, and whether it carries a
//! doc comment. A naming convention is a majority vote over those names. A
//! documentation expectation is a proportion. Both are facts about the
//! repository, and both are the same tomorrow.
//!
//! # Determinism
//!
//! Rules 29 to 32 apply. Every count here comes from
//! [`crate::extract::FileSymbols`], which is already sorted; nothing reads
//! a clock, a hash map's iteration order, or the filesystem. The same
//! commit produces the same [`StyleProfile`], byte for byte.
//!
//! # What is deliberately absent
//!
//! Indentation width, line length, and the error-handling idiom are not
//! here. The first two need the source bytes, which extraction does not
//! keep; [`measure_source`] takes them from a caller that still has them.
//! The third cannot be told from names and kinds alone, and a guess an
//! agent then applies across a codebase is worse than no answer.

use std::collections::BTreeMap;

use serde::Serialize;

use crate::extract::{DefKind, FileSymbols};

/// How a name is written.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Casing {
    /// `read_events`
    Snake,
    /// `readEvents`
    Camel,
    /// `ReadEvents`
    Pascal,
    /// `READ_EVENTS`
    ScreamingSnake,
    /// `read-events`
    Kebab,
    /// One word, all lower case: tells `snake_case` and `camelCase` apart
    /// only by accident, so it is counted separately rather than voting
    /// for either.
    Flat,
    /// Nothing above fits.
    Other,
}

/// Classifies one name.
///
/// A single lower-case word is [`Casing::Flat`] rather than a vote for
/// snake or camel: `new`, `run`, and `id` are written the same way under
/// both conventions, and counting them for one would decide the vote on
/// names that carry no evidence.
#[must_use]
pub fn casing_of(name: &str) -> Casing {
    let core = name.trim_matches('_');
    if core.is_empty() || !core.chars().next().is_some_and(char::is_alphabetic) {
        return Casing::Other;
    }
    if !core
        .chars()
        .all(|c| c.is_alphanumeric() || c == '_' || c == '-')
    {
        return Casing::Other;
    }

    let has_upper = core.chars().any(char::is_uppercase);
    let has_lower = core.chars().any(char::is_lowercase);
    let leads_upper = core.chars().next().is_some_and(char::is_uppercase);

    if core.contains('-') {
        return Casing::Kebab;
    }
    if core.contains('_') {
        return if has_lower {
            Casing::Snake
        } else {
            Casing::ScreamingSnake
        };
    }
    if !has_lower && has_upper {
        return Casing::ScreamingSnake;
    }
    if leads_upper {
        return Casing::Pascal;
    }
    if has_upper {
        return Casing::Camel;
    }
    Casing::Flat
}

/// The convention one kind of definition follows, and how strongly.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Convention {
    /// The definition kind this describes, as [`DefKind`] names it.
    pub kind: String,
    /// The casing most of those definitions use.
    pub casing: Casing,
    /// How many carry that casing.
    pub agreeing: u32,
    /// How many were counted in total.
    pub total: u32,
}

impl Convention {
    /// The share of definitions of this kind following the convention,
    /// `0.0` to `1.0`.
    ///
    /// A caller reports the convention as the house rule only when this is
    /// high: a 55% majority is a coin toss dressed as a rule.
    #[must_use]
    pub fn agreement(&self) -> f64 {
        if self.total == 0 {
            return 0.0;
        }
        f64::from(self.agreeing) / f64::from(self.total)
    }
}

/// Where a repository keeps its tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TestLayout {
    /// Beside the code they test, in the same file or directory.
    Alongside,
    /// In a directory of their own.
    Separate,
    /// Both, in numbers close enough that neither is the rule.
    Mixed,
    /// No test file was found.
    None,
}

/// What the source bytes say, which extraction does not keep.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct SourceShape {
    /// The indentation unit: `None` when the files disagree, or when no
    /// indented line was found.
    pub indent: Option<Indent>,
    /// The 95th percentile line width, in characters.
    pub line_width_p95: u32,
    /// The median file length, in lines.
    pub file_lines_median: u32,
}

/// One level of indentation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Indent {
    /// A tab character.
    Tab,
    /// This many spaces.
    Spaces(u8),
}

/// The house style, as the repository itself reveals it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct StyleProfile {
    /// One entry per definition kind that appears at least twice, ordered
    /// by kind name.
    pub conventions: Vec<Convention>,
    /// How many exported definitions carry a doc comment.
    pub documented_exports: u32,
    /// How many exported definitions there are.
    pub exports: u32,
    /// Where the tests live.
    pub test_layout: TestLayout,
    /// Whether modules are named by a directory file (`mod.rs`,
    /// `index.ts`, `__init__.py`) or by their own name.
    pub module_files: ModuleLayout,
    /// What the source bytes say, when a caller measured them.
    pub source: Option<SourceShape>,
}

/// How a repository names the file that stands for a directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ModuleLayout {
    /// A file inside the directory: `mod.rs`, `index.ts`, `__init__.py`.
    DirectoryFile,
    /// A file beside the directory, named after it.
    NamedFile,
    /// Both, or neither clearly.
    Mixed,
}

impl StyleProfile {
    /// The share of exported definitions carrying a doc comment.
    #[must_use]
    pub fn doc_density(&self) -> f64 {
        if self.exports == 0 {
            return 0.0;
        }
        f64::from(self.documented_exports) / f64::from(self.exports)
    }

    /// The convention for `kind`, when one was counted.
    #[must_use]
    pub fn convention_for(&self, kind: &str) -> Option<&Convention> {
        self.conventions.iter().find(|c| c.kind == kind)
    }
}

/// How many definitions of a kind must exist before its casing is called a
/// convention.
///
/// Two definitions agreeing is a coincidence. Below this, the kind is left
/// out rather than reported as a rule an agent should follow.
const MIN_FOR_CONVENTION: u32 = 3;

/// The file names that stand for a directory, across the languages the
/// syntax stage parses.
const DIRECTORY_FILES: [&str; 5] = ["mod.rs", "index.ts", "index.js", "__init__.py", "index.tsx"];

/// Derives the style profile from extraction's output.
///
/// `source` carries what only the bytes can say; pass `None` when they are
/// not to hand, and the profile reports the rest.
#[must_use]
pub fn profile(files: &[FileSymbols], source: Option<SourceShape>) -> StyleProfile {
    let mut by_kind: BTreeMap<&'static str, BTreeMap<Casing, u32>> = BTreeMap::new();
    let mut exports = 0u32;
    let mut documented = 0u32;

    for file in files {
        for def in &file.defs {
            if def.exported {
                exports = exports.saturating_add(1);
                if def.doc_present {
                    documented = documented.saturating_add(1);
                }
            }
            let counts = by_kind.entry(kind_name(def.kind)).or_default();
            *counts.entry(casing_of(&def.name)).or_insert(0) += 1;
        }
    }

    let conventions = by_kind
        .into_iter()
        .filter_map(|(kind, counts)| convention(kind, &counts))
        .collect();

    StyleProfile {
        conventions,
        documented_exports: documented,
        exports,
        test_layout: test_layout(files),
        module_files: module_layout(files),
        source,
    }
}

/// Picks the winning casing for one kind.
///
/// Neither [`Casing::Flat`] nor [`Casing::Other`] can win. A repository
/// whose functions are all one lower-case word has said nothing about
/// whether it prefers snake or camel, and "no consistent convention, 80%
/// of them" is a sentence that contradicts itself. Both still count
/// towards the total, so agreement falls where the evidence is thin and
/// the caller's own threshold drops the kind.
fn convention(kind: &'static str, counts: &BTreeMap<Casing, u32>) -> Option<Convention> {
    let total: u32 = counts.values().copied().sum();
    if total < MIN_FOR_CONVENTION {
        return None;
    }
    let (casing, agreeing) = counts
        .iter()
        .filter(|(casing, _)| !matches!(**casing, Casing::Flat | Casing::Other))
        .max_by(|a, b| a.1.cmp(b.1).then_with(|| b.0.cmp(a.0)))
        .map(|(casing, count)| (*casing, *count))?;

    Some(Convention {
        kind: kind.to_owned(),
        casing,
        agreeing,
        total,
    })
}

/// The stable name for a definition kind.
const fn kind_name(kind: DefKind) -> &'static str {
    match kind {
        DefKind::Function => "function",
        DefKind::Method => "method",
        DefKind::Class => "class",
        DefKind::Interface => "interface",
        DefKind::Enum => "enum",
        DefKind::TypeAlias => "type",
        DefKind::Module => "module",
        DefKind::Constant => "constant",
        DefKind::Variable => "variable",
        DefKind::Macro => "macro",
        DefKind::Section => "section",
    }
}

/// Returns true when a path looks like a test file.
fn is_test_path(path: &std::path::Path) -> bool {
    let text = path.to_string_lossy();
    text.split('/')
        .any(|part| part == "tests" || part == "test" || part == "__tests__" || part == "spec")
        || text.contains("_test.")
        || text.contains(".test.")
        || text.contains("_spec.")
        || text.contains(".spec.")
}

/// Decides where the tests live.
///
/// "Alongside" counts a test file that sits in the same directory as
/// non-test code; "separate" counts one under a directory of its own. A
/// language that keeps its tests inside the file they test — Rust's
/// `#[cfg(test)]` — shows as [`TestLayout::None`] from paths alone, which
/// is honest: no separate test file exists.
fn test_layout(files: &[FileSymbols]) -> TestLayout {
    let mut separate = 0u32;
    let mut alongside = 0u32;

    for file in files {
        if !is_test_path(&file.path) {
            continue;
        }
        let in_test_directory = file
            .path
            .parent()
            .and_then(|dir| dir.file_name())
            .is_some_and(|name| {
                matches!(
                    name.to_string_lossy().as_ref(),
                    "tests" | "test" | "__tests__" | "spec"
                )
            });
        if in_test_directory {
            separate = separate.saturating_add(1);
        } else {
            alongside = alongside.saturating_add(1);
        }
    }

    match (separate, alongside) {
        (0, 0) => TestLayout::None,
        (s, a) if s > a.saturating_mul(2) => TestLayout::Separate,
        (s, a) if a > s.saturating_mul(2) => TestLayout::Alongside,
        _ => TestLayout::Mixed,
    }
}

/// Decides how modules are named.
fn module_layout(files: &[FileSymbols]) -> ModuleLayout {
    let directory_files = files
        .iter()
        .filter(|file| {
            file.path
                .file_name()
                .is_some_and(|name| DIRECTORY_FILES.contains(&name.to_string_lossy().as_ref()))
        })
        .count();

    // A repository with several directory files uses that convention; one
    // with none uses named files. A handful either way is mixed, and
    // saying so beats picking the larger by one.
    match directory_files {
        0 => ModuleLayout::NamedFile,
        n if n >= 3 => ModuleLayout::DirectoryFile,
        _ => ModuleLayout::Mixed,
    }
}

/// Measures what only the source bytes say.
///
/// Takes `(path, source)` pairs so a caller that already read every file
/// to parse it does not read them again.
#[must_use]
pub fn measure_source<'a>(sources: impl IntoIterator<Item = &'a [u8]>) -> SourceShape {
    let mut tabs = 0u32;
    let mut space_widths: BTreeMap<u8, u32> = BTreeMap::new();
    let mut widths: Vec<u32> = Vec::new();
    let mut file_lengths: Vec<u32> = Vec::new();

    for source in sources {
        let text = String::from_utf8_lossy(source);
        let mut lines = 0u32;
        for line in text.lines() {
            lines = lines.saturating_add(1);
            widths.push(u32::try_from(line.chars().count()).unwrap_or(u32::MAX));

            if line.starts_with('\t') {
                tabs = tabs.saturating_add(1);
            } else {
                let spaces = line.len() - line.trim_start_matches(' ').len();
                // Only the first level tells us the unit. Deeper
                // indentation is a multiple of it and would vote for the
                // wrong width.
                if (1..=8).contains(&spaces) {
                    let width = u8::try_from(spaces).unwrap_or(u8::MAX);
                    *space_widths.entry(width).or_insert(0) += 1;
                }
            }
        }
        file_lengths.push(lines);
    }

    let indent = decide_indent(tabs, &space_widths);
    SourceShape {
        indent,
        line_width_p95: percentile(&mut widths, 0.95),
        file_lines_median: percentile(&mut file_lengths, 0.50),
    }
}

/// Chooses the indentation unit from the counts.
fn decide_indent(tabs: u32, space_widths: &BTreeMap<u8, u32>) -> Option<Indent> {
    let spaces_total: u32 = space_widths.values().copied().sum();
    if tabs == 0 && spaces_total == 0 {
        return None;
    }
    if tabs > spaces_total {
        return Some(Indent::Tab);
    }
    space_widths
        .iter()
        .max_by(|a, b| a.1.cmp(b.1).then_with(|| b.0.cmp(a.0)))
        .map(|(width, _)| Indent::Spaces(*width))
}

/// The `fraction` percentile of `values`, which this sorts in place.
///
/// Returns `0` for an empty input rather than failing: a repository with
/// no lines is not an error, it is a repository with no lines.
fn percentile(values: &mut [u32], fraction: f64) -> u32 {
    if values.is_empty() {
        return 0;
    }
    values.sort_unstable();
    #[allow(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "a line count is far inside f64's exact integer range, and the index is \
                  clamped to the slice before it is used"
    )]
    let index = ((values.len() as f64 - 1.0) * fraction).round() as usize;
    values[index.min(values.len() - 1)]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extract::{Def, Span};
    use std::path::PathBuf;

    fn def(name: &str, kind: DefKind, exported: bool, doc: bool) -> Def {
        Def {
            name: name.to_owned(),
            kind,
            range: Span {
                start_byte: 0,
                end_byte: 1,
                start_row: 0,
                start_column: 0,
                end_row: 0,
                end_column: 1,
            },
            exported,
            doc_present: doc,
            is_interface_like: false,
        }
    }

    fn file(path: &str, defs: Vec<Def>) -> FileSymbols {
        FileSymbols {
            path: PathBuf::from(path),
            language: crate::syntax::Language::Rust,
            imports: Vec::new(),
            defs,
            refs: Vec::new(),
        }
    }

    // --- casing ---------------------------------------------------------

    #[test]
    fn each_convention_is_recognised() {
        assert_eq!(casing_of("read_events"), Casing::Snake);
        assert_eq!(casing_of("readEvents"), Casing::Camel);
        assert_eq!(casing_of("ReadEvents"), Casing::Pascal);
        assert_eq!(casing_of("READ_EVENTS"), Casing::ScreamingSnake);
        assert_eq!(casing_of("read-events"), Casing::Kebab);
    }

    #[test]
    fn one_lower_case_word_votes_for_nothing() {
        // `new`, `run` and `id` are written identically under snake and
        // camel. Counting them for either would decide the vote on names
        // that carry no evidence at all.
        for name in ["new", "run", "id"] {
            assert_eq!(casing_of(name), Casing::Flat, "{name}");
        }
    }

    #[test]
    fn a_leading_underscore_does_not_change_the_convention() {
        assert_eq!(casing_of("_read_events"), Casing::Snake);
    }

    #[test]
    fn a_name_that_is_not_a_word_is_other() {
        assert_eq!(casing_of(""), Casing::Other);
        assert_eq!(casing_of("42"), Casing::Other);
    }

    // --- conventions ----------------------------------------------------

    #[test]
    fn a_majority_convention_is_reported_with_its_agreement() {
        let files = vec![file(
            "src/a.rs",
            vec![
                def("read_events", DefKind::Function, true, true),
                def("write_events", DefKind::Function, true, true),
                def("openFile", DefKind::Function, true, true),
                def("close_file", DefKind::Function, true, true),
            ],
        )];
        let profile = profile(&files, None);
        let convention = profile.convention_for("function").expect("counted");
        assert_eq!(convention.casing, Casing::Snake);
        assert_eq!(convention.agreeing, 3);
        assert_eq!(convention.total, 4);
        assert!((convention.agreement() - 0.75).abs() < 1e-9);
    }

    #[test]
    fn a_kind_with_too_few_definitions_is_not_a_convention() {
        // Two agreeing is a coincidence, not a house rule.
        let files = vec![file(
            "src/a.rs",
            vec![
                def("Alpha", DefKind::Class, true, false),
                def("Beta", DefKind::Class, true, false),
            ],
        )];
        assert!(profile(&files, None).convention_for("class").is_none());
    }

    #[test]
    fn a_kind_whose_names_are_not_identifiers_yields_no_convention() {
        // Markdown headings arrive as `section` definitions. "Name a
        // section in no consistent convention (80% of them do)" is a
        // sentence that contradicts itself, so no rule is reported.
        let files = vec![file(
            "README.md",
            vec![
                def("How to use this", DefKind::Section, true, false),
                def("What it is", DefKind::Section, true, false),
                def("Why it exists", DefKind::Section, true, false),
            ],
        )];
        assert!(profile(&files, None).convention_for("section").is_none());
    }

    #[test]
    fn flat_names_never_become_the_house_rule() {
        let files = vec![file(
            "src/a.rs",
            vec![
                def("new", DefKind::Method, true, false),
                def("run", DefKind::Method, true, false),
                def("id", DefKind::Method, true, false),
                def("read_events", DefKind::Method, true, false),
            ],
        )];
        let profile = profile(&files, None);
        let convention = profile.convention_for("method").expect("counted");
        assert_eq!(convention.casing, Casing::Snake);
        assert_eq!(convention.total, 4, "the flat names still count against it");
        assert_eq!(convention.agreeing, 1);
    }

    // --- documentation --------------------------------------------------

    #[test]
    fn doc_density_counts_exports_only() {
        // A private helper with no doc comment says nothing about whether
        // the repository documents its public surface.
        let files = vec![file(
            "src/a.rs",
            vec![
                def("a", DefKind::Function, true, true),
                def("b", DefKind::Function, true, false),
                def("c", DefKind::Function, false, false),
            ],
        )];
        let profile = profile(&files, None);
        assert_eq!(profile.exports, 2);
        assert_eq!(profile.documented_exports, 1);
        assert!((profile.doc_density() - 0.5).abs() < 1e-9);
    }

    #[test]
    fn a_repository_with_no_exports_has_no_doc_density() {
        assert!((profile(&[], None).doc_density() - 0.0).abs() < 1e-9);
    }

    // --- layout ---------------------------------------------------------

    #[test]
    fn tests_in_their_own_directory_are_separate() {
        let files = vec![
            file("src/a.rs", vec![]),
            file("tests/one.rs", vec![]),
            file("tests/two.rs", vec![]),
            file("tests/three.rs", vec![]),
        ];
        assert_eq!(profile(&files, None).test_layout, TestLayout::Separate);
    }

    #[test]
    fn tests_beside_the_code_are_alongside() {
        let files = vec![
            file("src/a.rs", vec![]),
            file("src/a_test.rs", vec![]),
            file("src/b_test.rs", vec![]),
            file("src/c_test.rs", vec![]),
        ];
        assert_eq!(profile(&files, None).test_layout, TestLayout::Alongside);
    }

    #[test]
    fn a_repository_that_tests_inside_its_source_files_reports_none() {
        // Rust's `#[cfg(test)]` leaves no test file to find. Reporting
        // "none" is honest; guessing "alongside" would be inventing a
        // convention from an absence.
        let files = vec![file("src/a.rs", vec![]), file("src/b.rs", vec![])];
        assert_eq!(profile(&files, None).test_layout, TestLayout::None);
    }

    #[test]
    fn directory_files_are_recognised_as_a_module_convention() {
        let files = vec![
            file("src/a/mod.rs", vec![]),
            file("src/b/mod.rs", vec![]),
            file("src/c/mod.rs", vec![]),
        ];
        assert_eq!(
            profile(&files, None).module_files,
            ModuleLayout::DirectoryFile
        );
    }

    #[test]
    fn a_repository_with_no_directory_files_names_its_modules() {
        let files = vec![file("src/a.rs", vec![]), file("src/b.rs", vec![])];
        assert_eq!(profile(&files, None).module_files, ModuleLayout::NamedFile);
    }

    // --- source bytes ---------------------------------------------------

    #[test]
    fn four_space_indentation_is_measured() {
        let source = b"fn main() {\n    let x = 1;\n    let y = 2;\n}\n";
        let shape = measure_source([source.as_slice()]);
        assert_eq!(shape.indent, Some(Indent::Spaces(4)));
    }

    #[test]
    fn tabs_win_when_they_outnumber_spaces() {
        let source = b"fn main() {\n\tlet x = 1;\n\tlet y = 2;\n}\n";
        assert_eq!(
            measure_source([source.as_slice()]).indent,
            Some(Indent::Tab)
        );
    }

    #[test]
    fn a_file_with_no_indentation_reports_none() {
        assert_eq!(measure_source([b"one\ntwo\n".as_slice()]).indent, None);
    }

    #[test]
    fn line_width_and_file_length_are_measured() {
        // Three files of 2, 3 and 4 lines: an odd count, so the median is
        // the middle one and does not depend on how ties are broken.
        let two = b"ab\ncd\n";
        let three = b"aaaaaaaaaa\nbb\ncc\n";
        let four = b"a\nb\nc\nd\n";
        let shape = measure_source([two.as_slice(), three.as_slice(), four.as_slice()]);
        assert_eq!(shape.line_width_p95, 10, "the one long line is the tail");
        assert_eq!(shape.file_lines_median, 3);
    }

    #[test]
    fn measuring_nothing_is_not_a_failure() {
        let shape = measure_source(std::iter::empty::<&[u8]>());
        assert_eq!(shape.indent, None);
        assert_eq!(shape.line_width_p95, 0);
    }

    // --- determinism ----------------------------------------------------

    #[test]
    fn the_same_input_profiles_identically() {
        // Rule 29: an agent is told to follow this. A profile that moved
        // between runs would move the instruction with it.
        let files = vec![file(
            "src/a.rs",
            vec![
                def("read_events", DefKind::Function, true, true),
                def("Alpha", DefKind::Class, true, true),
                def("BETA", DefKind::Constant, true, false),
                def("write_events", DefKind::Function, true, true),
                def("close_all", DefKind::Function, false, false),
            ],
        )];
        assert_eq!(profile(&files, None), profile(&files, None));
    }
}
