//! A hand-rolled timing harness for the discover and syntax stages.
//!
//! This is not a `criterion` benchmark: `dark-explore` adds no benchmarking
//! dependency beyond what F1 already needs, and a plain, `harness = false`
//! binary is enough to answer the question F1 asks — how long a cold run
//! takes over roughly 100k lines, and how long a warm, fully-cached run
//! takes over the same tree. Run it with `cargo bench -p dark-explore
//! parse`.
//!
//! The container this ran in during development has no fixed relationship
//! to the machine a person runs `dark` on, so treat the numbers this prints
//! as a measurement of this container, not as a claim that the PRD's
//! targets (cold under 5 seconds, warm under 200 milliseconds) hold on
//! every machine.

use std::fmt::Write as _;
use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};

use dark_explore::discover::{self, DiscoverOptions};
use dark_explore::syntax::{self, Cache};
use tempfile::TempDir;

/// The number of files the synthetic fixture writes.
const FILE_COUNT: usize = 500;
/// The number of function definitions each file holds. Each definition
/// spans three lines (signature, body, closing brace), so
/// `FILE_COUNT * LINES_PER_FILE * 3` is the fixture's total line count.
const LINES_PER_FILE: usize = 100;

/// The number of source lines that one function definition spans.
const LINES_PER_DEFINITION: usize = 3;

/// Writes a synthetic repository of small, syntactically valid Rust
/// functions, sized to land close to 100k lines.
fn write_fixture(root: &Path) {
    for file_index in 0..FILE_COUNT {
        let mut content = String::new();
        for line_index in 0..LINES_PER_FILE {
            let _ = writeln!(
                content,
                "fn f_{file_index}_{line_index}(x: i32) -> i32 {{\n    x + {line_index}\n}}"
            );
        }
        let path = root.join(format!("src/module_{file_index:04}.rs"));
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }
}

fn run(root: &Path, cache: &Cache) -> (discover::Snapshot, Cache, Duration) {
    let start = Instant::now();
    let snapshot = discover::discover(root, &DiscoverOptions::default()).unwrap();
    let (files, next_cache) = syntax::parse_snapshot(cache, root, &snapshot).unwrap();
    let elapsed = start.elapsed();
    assert_eq!(
        files.len(),
        FILE_COUNT,
        "the fixture must parse every file it wrote"
    );
    (snapshot, next_cache, elapsed)
}

fn main() {
    let dir = TempDir::new().expect("failed to create the fixture directory");
    write_fixture(dir.path());
    let total_lines = FILE_COUNT * LINES_PER_FILE * LINES_PER_DEFINITION;

    let (_snapshot, cache, cold) = run(dir.path(), &Cache::new());
    let (_snapshot, _cache, warm) = run(dir.path(), &cache);

    println!("dark-explore parse timing ({total_lines} lines across {FILE_COUNT} files)");
    println!("  cold (empty cache): {cold:?}   (PRD target: under 5 s)");
    println!("  warm (full cache):  {warm:?}   (PRD target: under 200 ms)");
    println!(
        "  these numbers describe this container, not every machine dark runs on; \
         see the F1 report for how they were produced"
    );
}
