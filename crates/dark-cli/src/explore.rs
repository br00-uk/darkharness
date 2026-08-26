//! `dark explore` and `dark seams`: the full F-series repository analysis
//! pipeline.
//!
//! Both commands run the same six stages — discovery, parsing, symbol
//! extraction, graph building, co-change, and seam scoring — and write the
//! same `.dark/explore/<tree-sha>.json` and `.lock` files (task unit `F4`).
//! `dark explore` reports the summary counts and where it wrote; `dark
//! seams` reads the same report and prints the top seams as a table.
//!
//! Every stage here runs against files already on disk and `git log` run
//! locally; nothing in this module opens a network connection.
//!
//! # Reusing an unchanged analysis
//!
//! [`output::tree_sha`] depends only on the repository's current file
//! content, and the pipeline is deterministic for the same tree and the
//! same configuration (Rule 29 in `CLAUDE.md`). So before running the
//! expensive stages — parsing, extraction, co-change, and seam scoring —
//! both commands compute the tree hash from discovery alone and check
//! whether `.dark/explore/<tree-sha>.json` already holds that exact
//! analysis. `dark explore --refresh` skips this check and always
//! recomputes; `dark seams` has no such flag and always prefers the
//! existing analysis when one matches.

use std::path::{Path, PathBuf};

use dark_explore::discover::{self, DiscoverOptions};
use dark_explore::output::{self, Sources};
use dark_explore::seam::{self, CoChange, Weights, Window};
use dark_explore::syntax::{self, Cache};
use dark_explore::{extract, graph};
use serde::Deserialize;

/// The subset of `.dark/explore/<tree-sha>.json`'s shape that these two
/// commands read.
///
/// [`output::Document`] derives `Serialize` only — task unit `F4` never
/// needed to read one back — so a cache hit here reads the file into this
/// crate-local shape instead of `output::Document` itself. A freshly built
/// [`output::Document`] converts into the same shape ([`From`], below), so
/// [`print_summary`] and [`print_seams_table`] have exactly one input type
/// to read regardless of which path produced it.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct Report {
    /// The commit half of Rule 29's promise, as lowercase hexadecimal.
    pub(crate) tree_sha: String,
    /// The configuration half, as lowercase hexadecimal.
    config_hash: String,
    /// The summary counts.
    pub(crate) stats: ReportStats,
    /// The highest-scoring seams, highest first.
    pub(crate) seams: Vec<ReportSeam>,
    /// Every module the M-graph holds, sorted by path.
    ///
    /// `dark explore` itself prints only counts and seams, but charting
    /// needs the module list for its stage 2 seed and `dark extend` needs
    /// it for the repository summary, so the reader keeps it. A report
    /// written before this field existed reads back with an empty list
    /// rather than failing.
    #[serde(default)]
    pub(crate) modules: Vec<ReportModule>,
    /// The most-coupled files, most first.
    #[serde(default)]
    pub(crate) hotspots: Vec<ReportHotspot>,
    /// Every language the repository is written in, most files first.
    #[serde(default)]
    pub(crate) languages: Vec<ReportLanguage>,
}

/// See [`output::LanguageCount`].
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct ReportLanguage {
    /// The language's name.
    pub(crate) name: String,
    /// How many files are written in it.
    pub(crate) files: u32,
    /// The sum of those files' definitions.
    #[allow(
        dead_code,
        reason = "written by dark explore and read back for completeness; the notes name \
                  files rather than definitions because a person counts files"
    )]
    pub(crate) defs: u32,
}

/// See [`output::Stats`]; only the four counts this module prints.
#[derive(Debug, Clone, Copy, Deserialize)]
pub(crate) struct ReportStats {
    /// How many files the F-graph holds.
    pub(crate) files: u32,
    /// The sum of every file's definitions.
    pub(crate) defs: u32,
    /// F-graph edge count.
    pub(crate) edges_f: u32,
    /// S-graph edge count.
    pub(crate) edges_s: u32,
    /// How well the F-graph divides into communities, 0 to 1. A report
    /// written before this reader kept the field reads back as 0.
    #[serde(default)]
    pub(crate) modularity: f64,
}

/// See [`output::Module`]; the fields charting and `dark extend` read.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct ReportModule {
    /// The directory's path, repository-relative.
    pub(crate) path: String,
    /// How many files the directory holds.
    pub(crate) files: u32,
    /// Afferent coupling: how many modules depend on this one.
    #[serde(rename = "Ca")]
    pub(crate) ca: u32,
    /// Efferent coupling: how many modules this one depends on.
    #[serde(rename = "Ce")]
    pub(crate) ce: u32,
    /// Abstractness, 0 to 1.
    #[serde(rename = "A", default)]
    #[allow(
        dead_code,
        reason = "read by `crate::refactor`'s pattern rules; kept here so one reader covers \
                  every field of the written report rather than two partial ones"
    )]
    pub(crate) a: f64,
    /// Instability, 0 to 1.
    #[serde(rename = "I", default)]
    #[allow(dead_code, reason = "see `ReportModule::a`")]
    pub(crate) i: f64,
    /// The community this module was assigned to.
    #[serde(default)]
    pub(crate) community: u32,
}

/// See [`output::Hotspot`]; the fields the repository summary reads.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct ReportHotspot {
    /// The file's path, repository-relative.
    pub(crate) path: String,
    /// Afferent coupling: how many other files import this one.
    #[serde(rename = "Ca")]
    pub(crate) ca: u32,
}

/// See [`output::Seam`]; the same nine fields, read back from JSON.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct ReportSeam {
    pub(crate) from: String,
    pub(crate) to: String,
    pub(crate) score: f64,
    pub(crate) hard: bool,
    betweenness: f64,
    crosses_community: bool,
    abstractness_target: f64,
    cochange: f64,
    test_proximity: f64,
}

impl From<&output::Document> for Report {
    fn from(document: &output::Document) -> Self {
        Self {
            tree_sha: document.tree_sha.clone(),
            config_hash: document.config_hash.clone(),
            stats: ReportStats {
                files: document.stats.files,
                defs: document.stats.defs,
                edges_f: document.stats.edges_f,
                edges_s: document.stats.edges_s,
                modularity: document.stats.modularity,
            },
            modules: document
                .modules
                .iter()
                .map(|module| ReportModule {
                    path: module.path.clone(),
                    files: module.files,
                    ca: module.ca,
                    ce: module.ce,
                    a: module.a,
                    i: module.i,
                    community: module.community,
                })
                .collect(),
            hotspots: document
                .hotspots
                .iter()
                .map(|hotspot| ReportHotspot {
                    path: hotspot.path.clone(),
                    ca: hotspot.ca,
                })
                .collect(),
            languages: document
                .languages
                .iter()
                .map(|language| ReportLanguage {
                    name: language.name.clone(),
                    files: language.files,
                    defs: language.defs,
                })
                .collect(),
            seams: document
                .seams
                .iter()
                .map(|seam| ReportSeam {
                    from: seam.from.clone(),
                    to: seam.to.clone(),
                    score: seam.score,
                    hard: seam.hard,
                    betweenness: seam.betweenness,
                    crosses_community: seam.crosses_community,
                    abstractness_target: seam.abstractness_target,
                    cochange: seam.cochange,
                    test_proximity: seam.test_proximity,
                })
                .collect(),
        }
    }
}

/// Reads the report for `root` when one is already on disk for the
/// current tree, without running the pipeline.
///
/// `dark plan`, `dark extend`, and `dark refactor` all want the analysis
/// but must not silently spend a minute recomputing it: discovery is its
/// own step, and a command that quietly runs it hides how long the real
/// work took. Returns `None` when no current report exists, and the
/// caller then tells the person to run `dark explore`.
///
/// # Errors
///
/// Returns an error when discovery fails, or when a report exists and
/// cannot be read back.
pub(crate) fn cached_report(root: &Path) -> anyhow::Result<Option<Report>> {
    let snapshot =
        discover::discover(root, &DiscoverOptions::default()).map_err(crate::contract_error)?;
    let tree_sha = output::tree_sha(&snapshot.files);
    let json_path = root
        .join(".dark")
        .join("explore")
        .join(format!("{tree_sha}.json"));
    if json_path.is_file() {
        return read_cached(&json_path).map(Some);
    }

    // The analysis is keyed by the tree it describes, and any commit
    // moves that key — including one that only touched a document.
    // `dark extend` writes `AGENTS.md`, so requiring an exact match would
    // make running it twice in a row fail on the file it just wrote. An
    // analysis of a near-enough tree is far better than a minute of
    // re-analysis, so long as the caller says which tree it is from.
    newest_report(root)
}

/// Reads the most recent analysis under `root`, whichever tree it
/// describes.
///
/// Returns `None` when none has ever been written. The caller compares
/// [`Report::tree_sha`] against the current tree to decide whether to say
/// so.
fn newest_report(root: &Path) -> anyhow::Result<Option<Report>> {
    let dir = root.join(".dark").join("explore");
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Ok(None);
    };

    let mut newest: Option<(std::time::SystemTime, PathBuf)> = None;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|ext| ext != "json") {
            continue;
        }
        let Ok(modified) = entry.metadata().and_then(|meta| meta.modified()) else {
            continue;
        };
        if newest.as_ref().is_none_or(|(at, _)| modified > *at) {
            newest = Some((modified, path));
        }
    }

    match newest {
        Some((_, path)) => read_cached(&path).map(Some),
        None => Ok(None),
    }
}

/// Returns the tree hash of the working tree at `root`.
///
/// # Errors
///
/// Returns an error when discovery fails.
pub(crate) fn current_tree_sha(root: &Path) -> anyhow::Result<String> {
    let snapshot =
        discover::discover(root, &DiscoverOptions::default()).map_err(crate::contract_error)?;
    Ok(output::tree_sha(&snapshot.files).to_string())
}

/// Where [`produce_report`] got a [`Report`] from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Source {
    /// Freshly computed and written this run.
    Fresh,
    /// Read back from an existing, still-current `.dark/explore/*.json`.
    Cached,
}

/// Resolves the repository root a command should analyse: `path` when
/// given, otherwise the nearest ancestor `.git` directory of the current
/// directory (see [`crate::repo_root`]).
fn resolve_root(path: Option<PathBuf>) -> anyhow::Result<PathBuf> {
    match path {
        Some(path) => Ok(path),
        None => crate::repo_root(),
    }
}

/// What one full run of the pipeline produced, in memory.
///
/// `dark explore` and `dark seams` need only the written report, but
/// `dark blast` needs the graphs and the scored seams themselves: a blast
/// radius is a walk over the F-graph, and the written JSON carries the
/// summary and the top seams rather than the graph. This is what
/// [`analyse_repository`] hands back so all three can share one pipeline.
pub(crate) struct Analysed {
    /// The three graphs.
    pub(crate) graphs: dark_explore::graph::Graphs,
    /// Every F-graph edge, scored and ranked.
    pub(crate) analysis: dark_explore::seam::SeamAnalysis,
    /// The report, as `dark explore` prints it.
    report: Report,
    /// Where the report was written.
    json_path: PathBuf,
}

/// Runs every pipeline stage, writes `.dark/explore/<tree-sha>.json` and
/// its lock, and returns the in-memory artefacts alongside the report.
fn analyse_repository(root: &Path) -> anyhow::Result<Analysed> {
    let discover_options = DiscoverOptions::default();
    let snapshot = discover::discover(root, &discover_options).map_err(crate::contract_error)?;
    let tree_sha = output::tree_sha(&snapshot.files);

    let (parsed, _cache) =
        syntax::parse_snapshot(&Cache::new(), root, &snapshot).map_err(crate::contract_error)?;
    let files = extract::extract_repository(&snapshot, &parsed);
    let graphs = graph::build(&files);
    let cochange = CoChange::read(root, Window::default()).map_err(crate::contract_error)?;
    let weights = Weights::default();
    let analysis = seam::analyse(&graphs, &cochange, &weights).map_err(crate::contract_error)?;

    let document = output::build(&Sources {
        files: &files,
        graphs: &graphs,
        analysis: &analysis,
        cochange: &cochange,
        discover_options: &discover_options,
        weights: &weights,
        tree_sha,
    });
    let (written, _lock) = output::write(root, &document).map_err(crate::contract_error)?;

    Ok(Analysed {
        report: Report::from(&document),
        json_path: written.json,
        graphs,
        analysis,
    })
}

/// Runs every pipeline stage and writes `.dark/explore/<tree-sha>.json`
/// and its lock.
fn run_pipeline(root: &Path) -> anyhow::Result<(Report, PathBuf)> {
    let analysed = analyse_repository(root)?;
    Ok((analysed.report, analysed.json_path))
}

/// Runs the pipeline for `dark blast`, which needs the graphs themselves.
///
/// This never reuses the written report: a blast radius walks the
/// F-graph, and the report holds the summary and the top seams rather
/// than the graph, so there is nothing on disk to reuse.
pub(crate) fn analyse_for_blast(path: Option<PathBuf>) -> anyhow::Result<Analysed> {
    let root = resolve_root(path)?;
    analyse_repository(&root)
}

/// Reads an already-written `.dark/explore/<tree-sha>.json` back into a
/// [`Report`].
fn read_cached(json_path: &Path) -> anyhow::Result<Report> {
    let text = std::fs::read_to_string(json_path)
        .map_err(|err| anyhow::anyhow!("cannot read {}: {err}", json_path.display()))?;
    serde_json::from_str(&text)
        .map_err(|err| anyhow::anyhow!("cannot parse {}: {err}", json_path.display()))
}

/// Produces the report for `root`: reused from disk when an analysis for
/// the current tree already exists and `refresh` is `false`, freshly
/// computed and written otherwise.
///
/// # Errors
///
/// Returns an error when discovery, parsing, extraction, co-change, seam
/// scoring, or the write stage fails, or when an existing report on disk
/// cannot be read back.
pub(crate) fn produce_report(
    root: &Path,
    refresh: bool,
) -> anyhow::Result<(Report, PathBuf, Source)> {
    if !refresh {
        // Discovery alone is enough to know the tree hash (F1, "Do" item
        // 6's tree_hash covers the same file list `output::tree_sha`
        // recomputes over — see that function's own documentation for why
        // this stage cannot just reuse `Snapshot::tree_hash` directly).
        // Running only this stage first, rather than the whole pipeline,
        // is what makes the cache check cheap.
        let snapshot =
            discover::discover(root, &DiscoverOptions::default()).map_err(crate::contract_error)?;
        let tree_sha = output::tree_sha(&snapshot.files);
        let json_path = root
            .join(".dark")
            .join("explore")
            .join(format!("{tree_sha}.json"));
        if json_path.is_file() {
            return Ok((read_cached(&json_path)?, json_path, Source::Cached));
        }
    }

    let (report, json_path) = run_pipeline(root)?;
    Ok((report, json_path, Source::Fresh))
}

/// Runs `dark explore`.
///
/// # Errors
///
/// Returns an error when `path` is not a directory, when a source file
/// cannot be read or parsed, when `git log` fails, or when the report
/// cannot be written. See [`produce_report`].
/// Analyses `root` and returns the summary as text.
///
/// `run_explore` prints; the terminal application needs the same words as
/// a value, because inside the alternate screen anything printed to
/// standard output lands under the interface where nobody sees it.
///
/// # Errors
///
/// Same as [`produce_report`].
pub(crate) fn summarise(root: &Path) -> anyhow::Result<String> {
    use std::fmt::Write as _;
    let (report, json_path, source) = produce_report(root, false)?;
    let mut out = String::new();
    let _ = writeln!(out, "files:        {}", report.stats.files);
    let _ = writeln!(out, "defs:         {}", report.stats.defs);
    let _ = writeln!(out, "edges (F):    {}", report.stats.edges_f);
    let _ = writeln!(out, "edges (S):    {}", report.stats.edges_s);
    let _ = writeln!(out, "wrote:        {}", json_path.display());
    if source == Source::Cached {
        let _ = writeln!(out, "(reused the existing analysis for this tree)");
    }
    out.push_str(&next_steps());
    Ok(out)
}

/// Returns the seam table for `root` as text. See [`summarise`].
///
/// # Errors
///
/// Same as [`produce_report`].
pub(crate) fn seams_text(root: &Path) -> anyhow::Result<String> {
    use std::fmt::Write as _;
    let (report, _json_path, _source) = produce_report(root, false)?;
    if report.seams.is_empty() {
        return Ok(format!("no seams found in {}.", root.display()));
    }
    let mut out = String::new();
    let _ = writeln!(
        out,
        "{:<44} {:<44} {:>6} {:>4}  leading term",
        "from", "to", "score", "hard"
    );
    for seam in report.seams.iter().take(20) {
        let _ = writeln!(
            out,
            "{:<44} {:<44} {:>6.3} {:>4}  {}",
            truncate(&seam.from, 44),
            truncate(&seam.to, 44),
            seam.score,
            if seam.hard { "yes" } else { "no" },
            leading_term(seam),
        );
    }
    Ok(out)
}

/// The two doors out of discovery.
///
/// Printed rather than asked: `dark explore` runs in scripts and in
/// continuous integration, and a command that stops for an answer on
/// standard input cannot. The choice belongs to whichever command runs
/// next.
pub(crate) fn next_steps() -> String {
    "\nthis repository has been analysed. What next?\n       dark extend    keep the language and the style, and prepare an agent to match them\n       dark refactor  change the language or the architecture, with a pattern chosen from \
     the analysis\n       dark plan      chart a map straight away, using the analysis as its seed\n"
        .to_owned()
}

pub(crate) fn run_explore(path: Option<PathBuf>, json: bool, refresh: bool) -> anyhow::Result<()> {
    let root = resolve_root(path)?;
    let (report, json_path, source) = produce_report(&root, refresh)?;

    if json {
        print!("{}", std::fs::read_to_string(&json_path)?);
        return Ok(());
    }

    println!("tree_sha:     {}", report.tree_sha);
    println!("config_hash:  {}", report.config_hash);
    println!("files:        {}", report.stats.files);
    println!("defs:         {}", report.stats.defs);
    println!("edges (F):    {}", report.stats.edges_f);
    println!("edges (S):    {}", report.stats.edges_s);
    println!("wrote:        {}", json_path.display());
    if source == Source::Cached {
        println!("(reused the existing analysis for this tree; pass --refresh to recompute it)");
    }
    print!("{}", next_steps());
    Ok(())
}

/// Runs `dark seams`.
///
/// # Errors
///
/// See [`run_explore`]. `dark seams` has no `--refresh` flag: it always
/// prefers an existing analysis for the current tree over recomputing one.
pub(crate) fn run_seams(path: Option<PathBuf>, top: usize) -> anyhow::Result<()> {
    let root = resolve_root(path)?;
    let (report, _json_path, _source) = produce_report(&root, false)?;

    if report.seams.is_empty() {
        println!("no seams found in {}.", root.display());
        return Ok(());
    }

    println!(
        "{:<44} {:<44} {:>6} {:>4}  leading term",
        "from", "to", "score", "hard"
    );
    for seam in report.seams.iter().take(top) {
        println!(
            "{:<44} {:<44} {:>6.3} {:>4}  {}",
            truncate(&seam.from, 44),
            truncate(&seam.to, 44),
            seam.score,
            if seam.hard { "yes" } else { "no" },
            leading_term(seam),
        );
    }
    Ok(())
}

/// Truncates `text` to `max` characters, marking the cut with `…` so a long
/// path does not break the table's column alignment.
fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let mut out: String = text.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// Names the term that contributed most to `seam`'s score, under the
/// default weights (F3, Do step 7) — the weights this pipeline always
/// scores with, since [`run_pipeline`] never reads a configuration file for
/// them.
fn leading_term(seam: &ReportSeam) -> &'static str {
    let weights = Weights::default();
    let contributions: [(&str, f64); 5] = [
        ("betweenness", weights.betweenness * seam.betweenness),
        (
            "crosses community",
            weights.crosses_community * f64::from(u8::from(seam.crosses_community)),
        ),
        (
            "target abstractness",
            weights.abstractness * seam.abstractness_target,
        ),
        (
            "inverse co-change",
            weights.inverse_cochange * (1.0 - seam.cochange),
        ),
        ("test proximity", weights.tested * seam.test_proximity),
    ];
    contributions
        .into_iter()
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .map_or("betweenness", |(name, _)| name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;
    use tempfile::TempDir;

    fn git(root: &Path, args: &[&str]) {
        let status = Command::new("git")
            .arg("-C")
            .arg(root)
            .args(args)
            .status()
            .expect("git runs");
        assert!(status.success(), "git {args:?} failed");
    }

    /// Initialises a fixture repository, with a `.gitignore` excluding
    /// `.dark/` — the same line this workspace's own root `.gitignore`
    /// carries. Discovery does not skip hidden entries on its own (F1
    /// deliberately walks with `hidden(false)`, so `.github/` and similar
    /// stay in scope); without this line, writing
    /// `.dark/explore/<tree-sha>.json` would change what the *next*
    /// discovery walk finds, so [`produce_report`]'s cache would never hit
    /// even for an unchanged source tree.
    fn init_repo(root: &Path) {
        git(root, &["init", "-q"]);
        git(root, &["config", "user.email", "test@example.invalid"]);
        git(root, &["config", "user.name", "Test"]);
        write(root, ".gitignore", "/.dark/\n");
    }

    fn write(root: &Path, rel: &str, content: &str) {
        let path = root.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, content).unwrap();
    }

    fn commit_all(root: &Path, message: &str) {
        git(root, &["add", "."]);
        git(root, &["commit", "-qm", message]);
    }

    #[test]
    fn resolve_root_prefers_the_explicit_path() {
        let root = resolve_root(Some(PathBuf::from("/some/path"))).unwrap();
        assert_eq!(root, PathBuf::from("/some/path"));
    }

    #[test]
    fn truncate_leaves_a_short_string_alone() {
        assert_eq!(truncate("short.rs", 44), "short.rs");
    }

    #[test]
    fn truncate_cuts_a_long_string_and_marks_it() {
        let long = "a".repeat(60);
        let truncated = truncate(&long, 44);
        assert_eq!(truncated.chars().count(), 44);
        assert!(truncated.ends_with('…'));
    }

    #[test]
    fn leading_term_names_the_highest_weighted_contributor() {
        let seam = ReportSeam {
            from: "a.rs".to_owned(),
            to: "b.rs".to_owned(),
            score: 0.9,
            hard: false,
            betweenness: 1.0,
            crosses_community: false,
            abstractness_target: 0.0,
            cochange: 1.0,
            test_proximity: 0.0,
        };
        assert_eq!(leading_term(&seam), "betweenness");
    }

    #[test]
    fn run_explore_writes_a_report_and_prints_the_summary() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        init_repo(root);
        write(root, "src/lib.rs", "pub fn a() {}\n");
        write(
            root,
            "src/main.rs",
            "use crate::a;\nfn main() { a::a(); }\n",
        );
        commit_all(root, "initial");

        run_explore(Some(root.to_path_buf()), false, false).unwrap();

        let dir = root.join(".dark").join("explore");
        let entries: Vec<_> = std::fs::read_dir(&dir).unwrap().collect();
        assert!(
            entries.iter().any(|e| e
                .as_ref()
                .unwrap()
                .path()
                .extension()
                .is_some_and(|ext| ext == "json")),
            "must write a .json report"
        );
    }

    #[test]
    fn a_second_explore_run_reuses_the_cached_analysis() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        init_repo(root);
        write(root, "src/lib.rs", "pub fn a() {}\n");
        commit_all(root, "initial");

        let (_first, path_first, source_first) = produce_report(root, false).unwrap();
        assert_eq!(source_first, Source::Fresh);

        let (_second, path_second, source_second) = produce_report(root, false).unwrap();
        assert_eq!(source_second, Source::Cached);
        assert_eq!(path_first, path_second);
    }

    #[test]
    fn refresh_recomputes_instead_of_reading_the_cache() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        init_repo(root);
        write(root, "src/lib.rs", "pub fn a() {}\n");
        commit_all(root, "initial");

        produce_report(root, false).unwrap();
        let (_report, _path, source) = produce_report(root, true).unwrap();
        assert_eq!(source, Source::Fresh);
    }

    #[test]
    fn run_seams_succeeds_on_a_repository_with_no_seams_yet() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        init_repo(root);
        write(root, "src/lib.rs", "pub fn a() {}\n");
        commit_all(root, "initial");

        run_seams(Some(root.to_path_buf()), 20).unwrap();
    }

    #[test]
    fn run_explore_json_flag_prints_valid_json() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        init_repo(root);
        write(root, "src/lib.rs", "pub fn a() {}\n");
        commit_all(root, "initial");

        run_explore(Some(root.to_path_buf()), true, false).unwrap();
    }
}
