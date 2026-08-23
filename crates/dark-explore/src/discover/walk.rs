//! The discovery walk: find files, filter them, sort them, and hash them.

use std::fs::File;
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};

use dark_contract::{ErrCode, Error, Result};
use ignore::WalkBuilder;
use rayon::prelude::*;

use super::file::DiscoveredFile;
use super::options::{DARKIGNORE_FILENAME, DiscoverOptions, NUL_SCAN_WINDOW};
use super::order::compare_paths;

/// The result of one discovery walk.
///
/// `files` is sorted with [`compare_paths`], so two walks over the same
/// commit and the same options produce the same `Snapshot`, byte for byte.
/// See Rule 29.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    /// The discovered files, sorted by path with the byte comparator.
    pub files: Vec<DiscoveredFile>,
    /// A hash over the sorted `(path, blob_hash)` pairs of every file in
    /// `files`.
    ///
    /// Two snapshots with the same `tree_hash` hold the same files with the
    /// same content. The syntax stage's cache keys its fast path on this
    /// value. See F1, "Do" item 6.
    pub tree_hash: blake3::Hash,
}

/// Walks `root`, filters what it finds, and returns a sorted [`Snapshot`].
///
/// Discovery excludes:
///
/// - anything that a tracked `.gitignore` file excludes;
/// - anything that a `.darkignore` file excludes, using the same pattern
///   syntax as `.gitignore`, negation included;
/// - files larger than `options.max_file_size`;
/// - binary files, tested by a NUL byte in the first
///   [`NUL_SCAN_WINDOW`](super::options::NUL_SCAN_WINDOW) bytes;
/// - directories named in `options.vendor_dirs`, at any depth.
///
/// Discovery ignores the host machine's global Git configuration (the
/// global `core.excludesFile` and the repository-local `.git/info/exclude`)
/// on purpose. Both live outside the commit, so honouring them would make
/// the `Snapshot` depend on the machine that ran the walk, which Rule 29
/// forbids. Only ignore rules that travel with the commit apply.
///
/// # Errors
///
/// Returns [`ErrCode::ExploreParse`] when `root` is not a directory.
/// Returns [`ErrCode::ExploreDirty`] when a file's content changes while
/// discovery reads it, because the walk can then no longer promise the
/// same bytes for the same commit.
pub fn discover(root: &Path, options: &DiscoverOptions) -> Result<Snapshot> {
    if !root.is_dir() {
        return Err(Error::new(
            ErrCode::ExploreParse,
            format!("repository root is not a directory: {}", root.display()),
        )
        .with_remedy("Point dark at a directory that exists."));
    }

    let candidates = collect_candidates(root, options);

    let mut files = candidates
        .into_par_iter()
        .map(|(abs_path, stat_size)| read_and_hash(root, &abs_path, stat_size))
        .collect::<Result<Vec<Option<DiscoveredFile>>>>()?
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();

    files.sort_by(|a, b| compare_paths(&a.path, &b.path));

    let tree_hash = tree_hash(&files);
    Ok(Snapshot { files, tree_hash })
}

/// The directory that discovery always excludes, regardless of
/// `options.vendor_dirs`.
///
/// `.git` holds version-control plumbing, not repository content: its
/// objects are not source files, and reading `.git/info/exclude` patterns
/// as if they were ordinary text would make no sense. Unlike the vendor
/// list, this exclusion is not configurable.
const VCS_DIR: &str = ".git";

/// Walks `root` sequentially and returns the absolute path and stat-time
/// size of every entry that passes the structural filters: a regular file,
/// not excluded by an ignore file, not inside a vendored or VCS directory,
/// and not larger than the configured limit.
///
/// The walk itself stays sequential because the `ignore` crate's ignore-file
/// matching is stateful per directory. The parallel step comes after, in
/// [`read_and_hash`], where each file is independent.
fn collect_candidates(root: &Path, options: &DiscoverOptions) -> Vec<(PathBuf, u64)> {
    let vendor_dirs = options.vendor_dirs.clone();
    let mut builder = WalkBuilder::new(root);
    builder
        .hidden(false)
        .parents(false)
        .git_ignore(true)
        .git_global(false)
        .git_exclude(false)
        .require_git(false)
        .follow_links(false)
        .max_filesize(Some(options.max_file_size))
        .add_custom_ignore_filename(DARKIGNORE_FILENAME)
        .filter_entry(move |entry| match entry.file_name().to_str() {
            Some(name) => name != VCS_DIR && !vendor_dirs.iter().any(|vendor| vendor == name),
            None => true,
        });

    let mut candidates = Vec::new();
    for entry in builder.build() {
        let Ok(entry) = entry else { continue };
        let Some(file_type) = entry.file_type() else {
            continue;
        };
        if !file_type.is_file() {
            continue;
        }
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        candidates.push((entry.into_path(), metadata.len()));
    }
    candidates
}

/// Reads one file, tests it for binary content, and hashes it.
///
/// Returns `Ok(None)` when the file is binary (a NUL byte in the first
/// [`NUL_SCAN_WINDOW`] bytes) or when it disappears or becomes unreadable
/// between the walk and this read — a benign race that discovery treats as
/// "not there any more" rather than as a hard failure.
///
/// Returns `Err` with [`ErrCode::ExploreDirty`] when the file's length
/// changes while discovery reads it: that is a working-tree edit racing the
/// analysis, not a missing file, and it breaks the byte-for-byte promise
/// that Rule 29 makes.
fn read_and_hash(root: &Path, abs_path: &Path, stat_size: u64) -> Result<Option<DiscoveredFile>> {
    let Ok(file) = File::open(abs_path) else {
        return Ok(None);
    };
    let mut reader = BufReader::new(file);
    let mut hasher = blake3::Hasher::new();
    let mut buf = vec![0_u8; 65_536].into_boxed_slice();
    let mut total_read: u64 = 0;

    loop {
        let Ok(read) = reader.read(&mut buf) else {
            return Ok(None);
        };
        if read == 0 {
            break;
        }
        let already_scanned = usize::try_from(total_read).unwrap_or(usize::MAX);
        let scan_len = NUL_SCAN_WINDOW.saturating_sub(already_scanned).min(read);
        if buf[..scan_len].contains(&0) {
            return Ok(None);
        }
        hasher.update(&buf[..read]);
        total_read += read as u64;
    }

    if total_read != stat_size {
        return Err(Error::new(
            ErrCode::ExploreDirty,
            format!(
                "{} changed size while discovery read it: {stat_size} bytes at the walk, \
                 {total_read} bytes on read",
                abs_path.display()
            ),
        ));
    }

    let Ok(relative) = abs_path.strip_prefix(root) else {
        return Ok(None);
    };
    Ok(Some(DiscoveredFile {
        path: relative.to_path_buf(),
        size: total_read,
        blob_hash: hasher.finalize(),
    }))
}

/// Hashes the sorted `(path, blob_hash)` pairs of `files` into one digest.
///
/// The hash length-prefixes each path before hashing it, so no path can be
/// crafted to collide with a neighbour across the boundary. It excludes
/// every timestamp, per Rule 31.
fn tree_hash(files: &[DiscoveredFile]) -> blake3::Hash {
    let mut hasher = blake3::Hasher::new();
    for file in files {
        // The `/`-joined component form, never the native bytes: Windows
        // walks with `\`, and a separator inside the digest would give
        // each platform its own hash for the same tree. On Unix these are
        // the native bytes unchanged. See Rule 32.
        let path_bytes: Vec<u8> = super::order::slash_bytes(&file.path).collect();
        hasher.update(&(path_bytes.len() as u64).to_le_bytes());
        hasher.update(&path_bytes);
        hasher.update(file.blob_hash.as_bytes());
    }
    hasher.finalize()
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::TempDir;

    use super::*;

    /// A small, fixed-seed xorshift generator, used only to reorder a test
    /// vector. It needs no external dependency and it is deterministic
    /// across runs, which keeps a failing test reproducible.
    struct Xorshift(u64);

    impl Xorshift {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }
    }

    fn shuffle<T>(items: &mut [T], seed: u64) {
        let mut rng = Xorshift(seed.max(1));
        for i in (1..items.len()).rev() {
            let j = usize::try_from(rng.next() % (i as u64 + 1)).unwrap_or(0);
            items.swap(i, j);
        }
    }

    fn write(root: &Path, rel: &str, content: &[u8]) {
        let path = root.join(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, content).unwrap();
    }

    fn paths_of(snapshot: &Snapshot) -> Vec<String> {
        snapshot
            .files
            .iter()
            .map(|f| f.path.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn finds_plain_files_in_sorted_order() {
        let dir = TempDir::new().unwrap();
        write(dir.path(), "b.rs", b"fn b() {}");
        write(dir.path(), "a.rs", b"fn a() {}");
        write(dir.path(), "src/c.rs", b"fn c() {}");

        let snapshot = discover(dir.path(), &DiscoverOptions::default()).unwrap();

        assert_eq!(paths_of(&snapshot), vec!["a.rs", "b.rs", "src/c.rs"]);
    }

    #[test]
    fn excludes_files_larger_than_the_limit() {
        let dir = TempDir::new().unwrap();
        write(dir.path(), "small.txt", b"fits");
        write(dir.path(), "big.txt", &[b'x'; 100]);

        let options = DiscoverOptions {
            max_file_size: 10,
            ..DiscoverOptions::default()
        };
        let snapshot = discover(dir.path(), &options).unwrap();

        assert_eq!(paths_of(&snapshot), vec!["small.txt"]);
    }

    #[test]
    fn excludes_binary_files_with_a_nul_byte_in_the_first_window() {
        let dir = TempDir::new().unwrap();
        write(dir.path(), "text.txt", b"hello world");
        write(dir.path(), "binary.bin", b"hello\0world");

        let snapshot = discover(dir.path(), &DiscoverOptions::default()).unwrap();

        assert_eq!(paths_of(&snapshot), vec!["text.txt"]);
    }

    #[test]
    fn a_nul_byte_past_the_scan_window_does_not_exclude_the_file() {
        let dir = TempDir::new().unwrap();
        let mut content = vec![b'a'; NUL_SCAN_WINDOW + 10];
        content[NUL_SCAN_WINDOW + 5] = 0;
        write(dir.path(), "late_nul.txt", &content);

        let snapshot = discover(dir.path(), &DiscoverOptions::default()).unwrap();

        assert_eq!(paths_of(&snapshot), vec!["late_nul.txt"]);
    }

    #[test]
    fn excludes_default_vendor_directories_at_any_depth() {
        let dir = TempDir::new().unwrap();
        write(dir.path(), "vendor/lib.rs", b"vendored");
        write(dir.path(), "node_modules/pkg/index.js", b"vendored");
        write(dir.path(), "third_party/lib.c", b"vendored");
        write(dir.path(), "nested/vendor/deep.rs", b"vendored");
        write(dir.path(), "kept.rs", b"kept");

        let snapshot = discover(dir.path(), &DiscoverOptions::default()).unwrap();

        assert_eq!(paths_of(&snapshot), vec!["kept.rs"]);
    }

    #[test]
    fn respects_gitignore_including_negation() {
        let dir = TempDir::new().unwrap();
        write(dir.path(), ".gitignore", b"*.log\n!important.log\n");
        write(dir.path(), "a.log", b"drop");
        write(dir.path(), "important.log", b"keep");
        write(dir.path(), "b.rs", b"keep");

        let snapshot = discover(dir.path(), &DiscoverOptions::default()).unwrap();

        assert_eq!(
            paths_of(&snapshot),
            vec![".gitignore", "b.rs", "important.log"]
        );
    }

    #[test]
    fn respects_darkignore_alongside_gitignore() {
        let dir = TempDir::new().unwrap();
        write(dir.path(), ".gitignore", b"*.log\n");
        write(dir.path(), ".darkignore", b"*.secret\n");
        write(dir.path(), "a.log", b"drop");
        write(dir.path(), "s.secret", b"drop");
        write(dir.path(), "keep.rs", b"keep");

        let snapshot = discover(dir.path(), &DiscoverOptions::default()).unwrap();

        assert_eq!(
            paths_of(&snapshot),
            vec![".darkignore", ".gitignore", "keep.rs"]
        );
    }

    #[test]
    fn ignores_the_machine_local_git_exclude_file() {
        // .git/info/exclude lives outside the commit; honouring it would
        // make the snapshot depend on the machine that ran the walk.
        let dir = TempDir::new().unwrap();
        write(dir.path(), ".git/info/exclude", b"local_only.rs\n");
        write(dir.path(), "local_only.rs", b"kept anyway");

        let snapshot = discover(dir.path(), &DiscoverOptions::default()).unwrap();

        assert_eq!(paths_of(&snapshot), vec!["local_only.rs"]);
    }

    #[test]
    fn errors_when_the_root_is_not_a_directory() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("plain.txt");
        fs::write(&file, b"not a directory").unwrap();

        let err = discover(&file, &DiscoverOptions::default()).unwrap_err();

        assert_eq!(err.code, ErrCode::ExploreParse);
    }

    #[test]
    fn tree_hash_is_stable_across_repeated_walks() {
        let dir = TempDir::new().unwrap();
        write(dir.path(), "a.rs", b"fn a() {}");
        write(dir.path(), "b.rs", b"fn b() {}");

        let first = discover(dir.path(), &DiscoverOptions::default()).unwrap();
        let second = discover(dir.path(), &DiscoverOptions::default()).unwrap();

        assert_eq!(first.tree_hash, second.tree_hash);
    }

    #[test]
    fn tree_hash_changes_when_a_file_changes() {
        let dir = TempDir::new().unwrap();
        write(dir.path(), "a.rs", b"fn a() {}");
        let before = discover(dir.path(), &DiscoverOptions::default()).unwrap();

        write(dir.path(), "a.rs", b"fn a() { changed() }");
        let after = discover(dir.path(), &DiscoverOptions::default()).unwrap();

        assert_ne!(before.tree_hash, after.tree_hash);
    }

    /// Pins Rule 32 for the discovery stage: shuffling how candidate files
    /// arrive at the parallel hashing step must not change the sorted,
    /// hashed output.
    #[test]
    fn parallel_hashing_produces_identical_bytes_regardless_of_input_order() {
        let dir = TempDir::new().unwrap();
        for i in 0..64 {
            write(
                dir.path(),
                &format!("file_{i:03}.rs"),
                format!("fn f{i}() {{}}").as_bytes(),
            );
        }
        let options = DiscoverOptions::default();

        let baseline = discover(dir.path(), &options).unwrap();

        for seed in [1_u64, 7, 31, 4242, 999_999] {
            let mut candidates = collect_candidates(dir.path(), &options);
            shuffle(&mut candidates, seed);
            let mut files = candidates
                .into_par_iter()
                .map(|(abs, size)| read_and_hash(dir.path(), &abs, size))
                .collect::<Result<Vec<Option<DiscoveredFile>>>>()
                .unwrap()
                .into_iter()
                .flatten()
                .collect::<Vec<_>>();
            files.sort_by(|a, b| compare_paths(&a.path, &b.path));
            let shuffled = Snapshot {
                tree_hash: tree_hash(&files),
                files,
            };

            assert_eq!(shuffled, baseline);
        }
    }
}
