//! Pins each of the thirteen grammars' parse output against a committed
//! S-expression snapshot.
//!
//! F1's own tests already prove that parsing one file twice, in any
//! parallel arrival order, produces the same `S`-expression (Rule 32); this
//! test adds a second guarantee task unit `F2` needs: that the *committed*
//! shape of each grammar's tree — the one every `tags.scm` query in
//! `src/extract/queries/` is written against — has not silently drifted
//! out from under those queries, whether from a `tree-sitter-*` version
//! bump or anything else. `tests/fixtures/sexpr/<language>.sexp` is the
//! committed answer for `tests/fixtures/sexpr/<language>.<ext>`; a query
//! file's own doc comments cite the exact node kinds these fixtures
//! exercise.

use std::fs;
use std::path::Path;

use dark_explore::syntax::Language;

fn fixture_dir() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sexpr")
}

/// Parses `<name>.<ext>` and asserts its `S`-expression matches the
/// committed `<name>.sexp`.
fn assert_matches_golden(language: Language, ext: &str) {
    let dir = fixture_dir();
    let name = language.name();
    let source = fs::read_to_string(dir.join(format!("{name}.{ext}")))
        .unwrap_or_else(|e| panic!("failed to read the {name} fixture: {e}"));
    let golden = fs::read_to_string(dir.join(format!("{name}.sexp")))
        .unwrap_or_else(|e| panic!("failed to read the {name} golden snapshot: {e}"));

    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(&language.grammar())
        .unwrap_or_else(|e| panic!("{name}: grammar failed to load: {e}"));
    let tree = parser
        .parse(&source, None)
        .unwrap_or_else(|| panic!("{name}: the parser returned no tree"));

    let actual = format!("{}\n", tree.root_node().to_sexp());
    assert_eq!(
        actual, golden,
        "{name}'s parse tree no longer matches its committed snapshot; \
         if the grammar's shape genuinely changed, update {name}.sexp \
         deliberately and re-check every tags.scm pattern that reads the \
         node kinds involved"
    );
    assert!(
        !tree.root_node().has_error(),
        "{name}'s golden fixture must parse cleanly, with no ERROR node"
    );
}

#[test]
fn rust_matches_its_golden_snapshot() {
    assert_matches_golden(Language::Rust, "rs");
}

#[test]
fn go_matches_its_golden_snapshot() {
    assert_matches_golden(Language::Go, "go");
}

#[test]
fn typescript_matches_its_golden_snapshot() {
    assert_matches_golden(Language::TypeScript, "ts");
}

#[test]
fn tsx_matches_its_golden_snapshot() {
    assert_matches_golden(Language::Tsx, "tsx");
}

#[test]
fn javascript_matches_its_golden_snapshot() {
    assert_matches_golden(Language::JavaScript, "js");
}

#[test]
fn python_matches_its_golden_snapshot() {
    assert_matches_golden(Language::Python, "py");
}

#[test]
fn java_matches_its_golden_snapshot() {
    assert_matches_golden(Language::Java, "java");
}

#[test]
fn csharp_matches_its_golden_snapshot() {
    assert_matches_golden(Language::CSharp, "cs");
}

#[test]
fn ruby_matches_its_golden_snapshot() {
    assert_matches_golden(Language::Ruby, "rb");
}

#[test]
fn c_matches_its_golden_snapshot() {
    assert_matches_golden(Language::C, "c");
}

#[test]
fn cpp_matches_its_golden_snapshot() {
    assert_matches_golden(Language::Cpp, "cpp");
}

#[test]
fn sql_matches_its_golden_snapshot() {
    assert_matches_golden(Language::Sql, "sql");
}

#[test]
fn markdown_matches_its_golden_snapshot() {
    assert_matches_golden(Language::Markdown, "md");
}
