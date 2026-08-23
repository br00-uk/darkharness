//! The `extract` stage.
//!
//! Extraction finds every definition, reference, and import in a file
//! ([`FileSymbols`]), then resolves references: first inside the file, by
//! walking `tree-sitter` scopes (`file::extract_file`), then across files,
//! by import map and by unique name (`resolve::resolve_cross_file`). See
//! task unit `F2`.
//!
//! [`graph`](crate::graph) builds the F-graph, S-graph, and M-graph
//! directly from [`extract_repository`]'s output; nothing in this module
//! depends on the graph stage.

mod doc;
mod file;
mod lang;
mod paths;
mod query;
mod resolve;
mod scope;
mod types;
mod util;

pub use types::{
    Def, DefKind, FileSymbols, Import, Ref, ResolutionConfidence, ResolvedSymbol, Span,
};

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

use rayon::prelude::*;

use crate::discover::Snapshot;
use crate::syntax::ParsedFile;

/// Extracts symbols from every file in `parsed` and resolves every
/// reference it can, within and across files.
///
/// The result holds one [`FileSymbols`] per entry in `parsed`, in
/// `parsed`'s own order — which is `snapshot`'s sorted order, the byte
/// comparator from Rule 30 (see [`crate::syntax::parse_snapshot`]'s
/// documentation). Running this function twice over the same `snapshot`
/// and the same parsed trees produces the same result, byte for byte,
/// regardless of how the parallel extraction step below interleaves,
/// because every per-file sort ([`file::extract_file`]) and the resolution
/// pass ([`resolve::resolve_cross_file`]) both order their own work
/// independently of arrival order. See Rule 32.
///
/// `snapshot` supplies the full set of discovered paths — including a file
/// no supported grammar parses, such as a `Cargo.toml` — that import
/// resolution checks membership against; `parsed` supplies the trees
/// extraction actually walks.
#[must_use]
pub fn extract_repository(snapshot: &Snapshot, parsed: &[Arc<ParsedFile>]) -> Vec<FileSymbols> {
    let all_paths: HashSet<PathBuf> = snapshot.files.iter().map(|f| f.path.clone()).collect();

    let mut files: Vec<FileSymbols> = parsed
        .par_iter()
        .map(|file| file::extract_file(file, &all_paths))
        .collect();

    resolve::resolve_cross_file(&mut files);
    files
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use tempfile::TempDir;

    use super::*;
    use crate::discover::{self, DiscoverOptions};
    use crate::syntax::{self, Cache};

    fn write(root: &Path, rel: &str, content: &str) {
        let path = root.join(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, content).unwrap();
    }

    fn extract_dir(root: &Path) -> Vec<FileSymbols> {
        let snapshot = discover::discover(root, &DiscoverOptions::default()).unwrap();
        let (parsed, _cache) = syntax::parse_snapshot(&Cache::new(), root, &snapshot).unwrap();
        extract_repository(&snapshot, &parsed)
    }

    fn file_of<'a>(files: &'a [FileSymbols], path: &str) -> &'a FileSymbols {
        files
            .iter()
            .find(|f| f.path == Path::new(path))
            .unwrap_or_else(|| panic!("no extracted file at {path}"))
    }

    #[test]
    fn resolves_a_same_file_call_by_local_scope() {
        let dir = TempDir::new().unwrap();
        write(
            dir.path(),
            "a.rs",
            "fn helper() {}\nfn caller() { helper(); }\n",
        );

        let files = extract_dir(dir.path());
        let file = file_of(&files, "a.rs");
        let call = file.refs.iter().find(|r| r.name == "helper").unwrap();

        assert_eq!(call.confidence, Some(ResolutionConfidence::Exact));
        let target = call.resolved_to.as_ref().unwrap();
        assert_eq!(target.file, Path::new("a.rs"));
        assert_eq!(file.defs[target.def_index].name, "helper");
    }

    #[test]
    fn resolves_an_import_scoped_call_across_files() {
        let dir = TempDir::new().unwrap();
        write(dir.path(), "Cargo.toml", "[package]\nname = \"x\"\n");
        write(dir.path(), "src/lib.rs", "pub fn helper() {}\n");
        // `crate::helper` resolves straight to the crate root
        // (`src/lib.rs`); `lib.rs` is the root itself, not a module named
        // `lib`, so `crate::lib::helper` would not name anything real.
        write(
            dir.path(),
            "src/main.rs",
            "use crate::helper;\nfn main() { helper(); }\n",
        );

        let files = extract_dir(dir.path());
        let main_file = file_of(&files, "src/main.rs");
        let call = main_file.refs.iter().find(|r| r.name == "helper").unwrap();

        assert_eq!(call.confidence, Some(ResolutionConfidence::ImportScoped));
        let target = call.resolved_to.as_ref().unwrap();
        assert_eq!(target.file, Path::new("src/lib.rs"));
    }

    #[test]
    fn resolves_a_unique_repository_wide_name_with_no_import() {
        let dir = TempDir::new().unwrap();
        write(
            dir.path(),
            "a.py",
            "def only_one_of_this_name():\n    pass\n",
        );
        write(
            dir.path(),
            "b.py",
            "def caller():\n    only_one_of_this_name()\n",
        );

        let files = extract_dir(dir.path());
        let b = file_of(&files, "b.py");
        let call = b
            .refs
            .iter()
            .find(|r| r.name == "only_one_of_this_name")
            .unwrap();

        assert_eq!(call.confidence, Some(ResolutionConfidence::NameOnly));
        let target = call.resolved_to.as_ref().unwrap();
        assert_eq!(target.file, Path::new("a.py"));
    }

    /// The load-bearing test the brief calls out by name: a name that
    /// matches definitions in two different files must not silently pick
    /// one. F2, "Do" item 4 and "Do not" both require this.
    #[test]
    fn an_ambiguous_name_across_two_files_stays_unresolved() {
        let dir = TempDir::new().unwrap();
        write(dir.path(), "a.py", "def ambiguous():\n    pass\n");
        write(dir.path(), "b.py", "def ambiguous():\n    pass\n");
        write(dir.path(), "c.py", "def caller():\n    ambiguous()\n");

        let files = extract_dir(dir.path());
        let c = file_of(&files, "c.py");
        let call = c.refs.iter().find(|r| r.name == "ambiguous").unwrap();

        assert_eq!(call.resolved_to, None);
        assert_eq!(call.confidence, None);
    }

    #[test]
    fn an_unresolvable_reference_is_recorded_not_dropped() {
        let dir = TempDir::new().unwrap();
        write(
            dir.path(),
            "a.py",
            "def caller():\n    nothing_defines_this()\n",
        );

        let files = extract_dir(dir.path());
        let a = file_of(&files, "a.py");
        let call = a
            .refs
            .iter()
            .find(|r| r.name == "nothing_defines_this")
            .unwrap();

        assert_eq!(call.resolved_to, None);
        assert_eq!(call.confidence, None);
    }

    #[test]
    fn extraction_is_stable_across_repeated_runs() {
        let dir = TempDir::new().unwrap();
        write(dir.path(), "a.rs", "pub fn a() { b(); }\n");
        write(dir.path(), "b.rs", "pub fn b() {}\n");
        write(
            dir.path(),
            "c.py",
            "class Foo:\n    '''doc'''\n    def bar(self):\n        pass\n",
        );

        let first = extract_dir(dir.path());
        let second = extract_dir(dir.path());

        assert_eq!(first.len(), second.len());
        for (a, b) in first.iter().zip(second.iter()) {
            assert_eq!(a, b);
        }
    }
}
