//! The report [`Document`] and how it is built.
//!
//! [`build`] reads a [`Sources`] value — everything F1 to F3 already
//! computed — and produces the JSON shape task unit `F4`'s "Do" item 1
//! gives, field for field. See the module documentation for the mappings
//! this stage makes between that shape and what [`crate::seam`] and
//! [`crate::graph`] actually carry, and for the rounding, sorting, and
//! path-formatting rules that keep two runs, and two platforms, byte for
//! byte identical.
//!
//! # Field mappings the PRD's example names but the analysis does not
//! carry literally
//!
//! - **`seams[].from` / `seams[].to`, `bridges[].from` / `bridges[].to`.**
//!   The PRD's example writes these as qualified symbol names
//!   (`"dark-core::turn"`). A seam score is computed over the **F-graph**
//!   (`seam::assemble`'s own module documentation explains why: co-change
//!   is a fact about files), so its two endpoints are files, not symbols.
//!   This stage writes the repository-relative file path of each endpoint,
//!   in [`path_to_string`]'s `/`-joined form — `"crates/dark-core/src/session.rs"`,
//!   not `"dark-core::turn"`.
//! - **`seams[].crosses_community`.** [`Terms::crosses_community`] is the
//!   `X(e)` term ready for the weighted sum: `1.0` or `0.0`, a float. The
//!   JSON field is a boolean, mapped as `terms.crosses_community >= 0.5`
//!   (equivalently, `== 1.0`; `>=` reads as the more obviously total
//!   comparison of the two).
//! - **`seams[].abstractness_target`.** [`Terms::abstractness`] is the
//!   `A(v)` term — the target's abstractness, per Do step 7's own naming.
//!   The JSON field spells out *which* endpoint it is the abstractness of,
//!   since a seam report lists two paths and "abstractness" alone would
//!   leave a reader guessing.
//! - **`seams[].cochange`.** [`Terms::inverse_cochange`] stores `1 - C(e)`,
//!   the form the score actually sums. A report is more legible showing the
//!   raw coupling a person can reason about directly, so this field is
//!   `1.0 - terms.inverse_cochange`, recovering `C(e)`.
//! - **`seams[].test_proximity`.** [`Terms::tested`] is the `T(e)` term:
//!   the PRD's example names the JSON field `test_proximity` for it
//!   directly, so this is a rename with no change of value.
//! - **`modules[].community`.** [`SeamAnalysis::communities`] partitions
//!   the **F-graph** — Do step 4 runs Louvain once, and `seam::assemble`
//!   reads that one partition for the seam score's `X(e)` term. The
//!   `modules` table needs a partition scoped to *modules*, which is a
//!   different graph (the M-graph). This stage runs a second, independent
//!   Louvain pass over `graphs.modules`, reusing
//!   [`crate::seam::community::detect`] (it is generic over any
//!   `DiGraph`, so nothing new needed writing) rather than inventing a
//!   from-files aggregate such as a majority vote. **The two partitions
//!   use unrelated numbering** — a seam's `crosses_community` boolean and
//!   a module's `community` integer are not comparable, and a `community`
//!   id here is not "the F-graph community most of this module's files sit
//!   in."
//! - **`hotspots`.** Nothing in `F1` to `F3` computes a hotspot ranking;
//!   see [`HOTSPOT_CA_WEIGHT`] for the formula this stage chose and why.
//! - **`unresolved_refs`.** A plain count, across every file, of
//!   [`Ref`](crate::extract::Ref) values whose `resolved_to` is `None`.
//!
//! # Rounding
//!
//! Every floating-point figure is rounded to [`ROUND_DECIMALS`] places
//! before it is serialised ([`round`]). Two platforms computing the exact
//! same [`crate::seam`] analysis can still disagree in a float's last bit —
//! summation order inside a `BTreeMap` iteration, for instance, is fixed
//! *within* a platform but the underlying floating-point unit's rounding of
//! a long dependency chain is not guaranteed identical to the last ulp
//! across every target this workspace builds for. Rounding to four decimal
//! places before serialising is coarser than that residual noise floor, so
//! it absorbs it rather than propagating it into the hashed bytes.
//!
//! # Sorting
//!
//! - `seams` keeps [`SeamAnalysis::seams`]'s own order — already ranked
//!   highest score first, tie broken by edge index (Rule 32) — truncated to
//!   the top [`MAX_REPORTED_SEAMS`].
//! - `bridges` lists every bridge (Do step 8: "Report every bridge,
//!   whatever its score"), re-sorted by `(from, to)` path string. This is
//!   not a redundant sort: [`crate::seam::Structure::bridges`]'s own order
//!   comes from the edge and node identifiers `graph::build` assigned by
//!   [`crate::discover::compare_paths`], which sorts *native* [`Path`]
//!   bytes — `\`-separated on Windows. Re-sorting by the `/`-joined string
//!   this module actually writes keeps that order identical on every
//!   platform; see `output::path`'s module documentation for the full
//!   argument.
//! - `modules` and `hotspots` are sorted the same way, by path string, for
//!   the same reason.
//!
//! Every one of these three lists is sorted by the byte order of an
//! already-`/`-joined string ([`compare_path_strings`]), never by calling
//! [`crate::discover::compare_paths`] on the underlying [`Path`] — see
//! `output::path`'s module documentation for why those two disagree on
//! Windows whenever a path continues into a subdirectory at a point where a
//! sibling's next byte falls between `/` and `\`.

use std::collections::BTreeMap;

use petgraph::graph::NodeIndex;
use serde::Serialize;

use crate::discover::DiscoverOptions;
use crate::extract::FileSymbols;
use crate::graph::Graphs;
use crate::seam::{CoChange, SeamAnalysis, Weights, community, metrics};

use super::config_hash;
use super::path::{compare_path_strings, path_to_string};

/// The schema version this stage writes. Bump it, and document the change
/// here, the day a field is added, renamed, or removed — a reader of an old
/// `.dark/explore/*.json` file needs to tell which shape it is looking at.
pub const VERSION: u32 = 1;

/// How many decimal places every floating-point figure is rounded to before
/// serialising. See the module documentation's "Rounding" section for why
/// four, and why at all.
pub const ROUND_DECIMALS: i32 = 4;

/// The largest number of ranked seams this stage writes to `seams`. Do step
/// 8 of task unit `F3` asks for "the highest-scoring N edges" without
/// fixing `N`; this is F4's own choice of report length, not a figure the
/// PRD specifies. 50 is far more than a person reads in one sitting and far
/// less than a large repository's full edge list, which is also reported —
/// completely, not just its top scorers — through `bridges`. Every scored
/// edge is still available from [`SeamAnalysis::seams`] directly; this
/// constant bounds only what this stage writes to the report file.
pub const MAX_REPORTED_SEAMS: usize = 50;

/// The largest number of ranked hotspots this stage writes to `hotspots`.
/// See [`MAX_REPORTED_SEAMS`] for why this stage picks a bound at all, and
/// [`HOTSPOT_CA_WEIGHT`] for how a hotspot is ranked in the first place.
pub const MAX_REPORTED_HOTSPOTS: usize = 50;

/// The weight [`build`] gives normalised afferent coupling (`Ca`) in the
/// hotspot score. See [`HOTSPOT_D_WEIGHT`] for the formula in full.
pub const HOTSPOT_CA_WEIGHT: f64 = 0.4;

/// The weight [`build`] gives distance from the main sequence (`D`) — see
/// [`crate::seam::NodeMetrics::distance`] — in the hotspot score.
pub const HOTSPOT_D_WEIGHT: f64 = 0.3;

/// The weight [`build`] gives normalised churn (commits touching the file,
/// within [`CoChange`]'s window) in the hotspot score.
///
/// # The hotspot formula
///
/// Nothing in task units `F1` to `F3` defines a hotspot ranking; this
/// stage's brief asks for "files ranked by a documented combination of
/// `Ca`, `D`, and churn," so here is that combination and the reasoning
/// behind it:
///
/// ```text
/// hotspot_score(file) = 0.4 × Ca_norm(file) + 0.3 × D(file) + 0.3 × churn_norm(file)
/// ```
///
/// `Ca` and churn are unbounded counts, so each is rescaled to 0–1 across
/// the repository's files first, with the same min-max technique
/// [`crate::seam::assemble`] uses for its own terms (equal raw values
/// rescale to 0 rather than dividing by zero). `D` is already 0–1.
///
/// `Ca` carries the most weight because it is the direct blast-radius
/// proxy: it says how many other files are affected when this one changes
/// badly. `D` and churn split the rest evenly — `D` says the file's shape
/// does not match how it is used (Martin's distance from the main
/// sequence: neither cleanly abstract-and-depended-on nor cleanly
/// concrete-and-depending), and churn says people keep touching it. A file
/// that is heavily depended upon, badly shaped, *and* constantly changed
/// should rank above a file that only scores high on one axis — a badly
/// shaped file nobody imports, or a heavily churned leaf test file — which
/// a pure sum without the `Ca` weighting would not distinguish as sharply.
///
/// These three weights are a fixed part of the ranking algorithm, not a
/// user-facing setting, so they do not feed
/// [`config_hash::compute`](super::config_hash::compute); see that
/// module's own documentation for the line it draws between "algorithm"
/// and "configuration."
pub const HOTSPOT_CHURN_WEIGHT: f64 = 0.3;

/// Rounds `value` to [`ROUND_DECIMALS`] places, and folds `-0.0` to `0.0`
/// so a value that rounds to zero never serialises as the visually odd
/// `-0.0`.
fn round(value: f64) -> f64 {
    let factor = 10f64.powi(ROUND_DECIMALS);
    let rounded = (value * factor).round() / factor;
    if rounded == 0.0 { 0.0 } else { rounded }
}

/// Rescales `values` into the range 0 to 1 by their minimum and maximum.
/// Mirrors `seam::assemble::min_max_normalise` (private to that module, and
/// keyed on `EdgeIndex` rather than `NodeIndex`): equal raw values rescale
/// to 0 rather than dividing by zero.
fn min_max_normalise(values: &BTreeMap<NodeIndex, f64>) -> BTreeMap<NodeIndex, f64> {
    let mut lowest = f64::INFINITY;
    let mut highest = f64::NEG_INFINITY;
    for &value in values.values() {
        lowest = lowest.min(value);
        highest = highest.max(value);
    }
    let spread = highest - lowest;
    values
        .iter()
        .map(|(&node, &value)| {
            let scaled = if spread > 0.0 {
                (value - lowest) / spread
            } else {
                0.0
            };
            (node, scaled)
        })
        .collect()
}

/// Everything [`build`] needs, gathered from the pipeline stages this task
/// unit needs (`Z1`, `F3`) rather than computed here.
pub struct Sources<'a> {
    /// Every file's extracted symbols, F2's own output. [`build`] reads
    /// this for [`Document::unresolved_refs`]; every other field comes from
    /// `graphs` or `analysis`.
    pub files: &'a [FileSymbols],
    /// The F-graph, S-graph, and M-graph task unit `F2` built.
    pub graphs: &'a Graphs,
    /// The scored seams, communities, structure, and betweenness task unit
    /// `F3` computed over the F-graph.
    pub analysis: &'a SeamAnalysis,
    /// The co-change reading `F3` computed from the repository's history.
    /// Its own [`CoChange::window`] feeds [`Document::config_hash`].
    pub cochange: &'a CoChange,
    /// The options discovery ran with. Feeds [`Document::config_hash`].
    pub discover_options: &'a DiscoverOptions,
    /// The seam-score weights `F3`'s formula used. Feeds
    /// [`Document::config_hash`].
    pub weights: &'a Weights,
    /// The commit half of Rule 29's promise. See [`super::tree_sha`] for
    /// why this is not [`crate::discover::Snapshot::tree_hash`] passed
    /// through unchanged.
    pub tree_sha: blake3::Hash,
}

/// The summary counts at the top of the report.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Stats {
    /// How many files the F-graph holds — every file a supported grammar
    /// parsed and F2 extracted symbols from. A discovered file with no
    /// matching grammar (a `Cargo.toml`, a `LICENSE`) is not counted here:
    /// it never became an F-graph node.
    pub files: u32,
    /// The sum of every file's definitions.
    pub defs: u32,
    /// How many F-graph edges: one file importing another.
    pub edges_f: u32,
    /// How many S-graph edges: one definition referencing another.
    pub edges_s: u32,
    /// The F-graph community partition's modularity. See
    /// [`crate::seam::Communities::modularity`].
    pub modularity: f64,
    /// Whether betweenness was sampled rather than computed exactly. See
    /// [`crate::seam::Betweenness::sampled`].
    pub betweenness_sampled: bool,
}

/// One directory in the M-graph.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Module {
    /// The directory's path, repository-relative, `/`-joined. The
    /// repository root itself is `""`.
    pub path: String,
    /// How many files sit directly in this directory.
    pub files: u32,
    /// Afferent coupling in the M-graph: how many other directories import
    /// a file in this one.
    #[serde(rename = "Ca")]
    pub ca: u32,
    /// Efferent coupling in the M-graph: how many other directories a file
    /// in this one imports.
    #[serde(rename = "Ce")]
    pub ce: u32,
    /// Instability: `Ce / (Ca + Ce)`.
    #[serde(rename = "I")]
    pub i: f64,
    /// Abstractness: interface-like definitions over all definitions,
    /// summed across the directory's files.
    #[serde(rename = "A")]
    pub a: f64,
    /// Distance from the main sequence: `|A + I - 1|`.
    #[serde(rename = "D")]
    pub d: f64,
    /// The module-level community identifier. See the module
    /// documentation's note on why this is a *second* Louvain partition,
    /// scoped to the M-graph, distinct from the F-graph partition a seam's
    /// `crosses_community` reads.
    pub community: u32,
}

/// One scored F-graph edge — a candidate seam.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Seam {
    /// The source file, repository-relative, `/`-joined.
    pub from: String,
    /// The target file, repository-relative, `/`-joined.
    pub to: String,
    /// The combined score, from 0 to 1. See `seam::score`'s module
    /// documentation for the formula.
    pub score: f64,
    /// Whether this edge is a bridge: removing it disconnects the F-graph.
    /// A bridge is also reported in `bridges`, whatever it scores here.
    pub hard: bool,
    /// `B(e)`: normalised edge betweenness.
    pub betweenness: f64,
    /// `X(e)`: whether this edge crosses an F-graph community boundary.
    pub crosses_community: bool,
    /// `A(v)`: the target file's abstractness.
    pub abstractness_target: f64,
    /// `C(a, b)`: the raw co-change coupling between the two files, from 0
    /// to 1. The score itself sums `1 - C(a, b)`; see the module
    /// documentation's mapping note.
    pub cochange: f64,
    /// `T(e)`: the fraction of the edge's two endpoints that a test file
    /// references.
    pub test_proximity: f64,
}

/// One bridge: an F-graph edge whose removal disconnects the graph.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Bridge {
    /// The source file, repository-relative, `/`-joined.
    pub from: String,
    /// The target file, repository-relative, `/`-joined.
    pub to: String,
    /// Always `true`: every entry in this list is a bridge by construction
    /// (task unit `F3`'s "Do" item 3 finds bridges with Tarjan). The field
    /// exists so a reader scanning `bridges` and `seams` side by side sees
    /// the same shape in both.
    pub hard: bool,
}

/// One file, ranked by [`HOTSPOT_CA_WEIGHT`]'s formula.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Hotspot {
    /// The file's path, repository-relative, `/`-joined.
    pub path: String,
    /// Afferent coupling: how many other files import this one.
    #[serde(rename = "Ca")]
    pub ca: u32,
    /// Distance from the main sequence.
    #[serde(rename = "D")]
    pub d: f64,
    /// How many commits in [`CoChange`]'s window touched this file.
    pub churn: u32,
}

/// The whole report: task unit `F4`, "Do" item 1's JSON shape.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Document {
    /// The schema version. See [`VERSION`].
    pub version: u32,
    /// The commit half of Rule 29's promise, as lowercase hexadecimal. See
    /// [`super::tree_sha`].
    pub tree_sha: String,
    /// The configuration half of Rule 29's promise, as lowercase
    /// hexadecimal. See `output::config_hash`'s module documentation for
    /// exactly what feeds it.
    pub config_hash: String,
    /// The summary counts.
    pub stats: Stats,
    /// Every M-graph directory, sorted by path.
    pub modules: Vec<Module>,
    /// The highest-scoring seams, highest first, capped at
    /// [`MAX_REPORTED_SEAMS`].
    pub seams: Vec<Seam>,
    /// Every bridge, sorted by path.
    pub bridges: Vec<Bridge>,
    /// The highest-ranked hotspots, highest first, capped at
    /// [`MAX_REPORTED_HOTSPOTS`].
    pub hotspots: Vec<Hotspot>,
    /// How many references, across every file, resolution left unresolved.
    pub unresolved_refs: u32,
}

fn build_stats(sources: &Sources<'_>) -> Stats {
    let files = u32::try_from(sources.graphs.files.node_count()).unwrap_or(u32::MAX);
    let defs = sources
        .graphs
        .files
        .node_weights()
        .map(|node| node.total_defs)
        .fold(0u32, u32::saturating_add);
    let edges_f = u32::try_from(sources.graphs.files.edge_count()).unwrap_or(u32::MAX);
    let edges_s = u32::try_from(sources.graphs.symbols.edge_count()).unwrap_or(u32::MAX);

    Stats {
        files,
        defs,
        edges_f,
        edges_s,
        modularity: round(sources.analysis.communities.modularity),
        betweenness_sampled: sources.analysis.betweenness.sampled,
    }
}

fn build_modules(graphs: &Graphs) -> Vec<Module> {
    // A second, module-scoped Louvain partition. See the module
    // documentation's mapping note on `modules[].community`.
    let module_communities = community::detect(&graphs.modules);

    let mut modules: Vec<Module> = graphs
        .modules
        .node_indices()
        .map(|node| {
            let directory = &graphs.modules[node];
            let node_metrics = metrics::for_node(
                &graphs.modules,
                node,
                directory.total_defs,
                directory.interface_like_defs,
            );
            let community_id = module_communities.of_node.get(&node).copied().unwrap_or(0);

            Module {
                path: path_to_string(&directory.path),
                files: directory.files,
                ca: node_metrics.ca,
                ce: node_metrics.ce,
                i: round(node_metrics.instability),
                a: round(node_metrics.abstractness),
                d: round(node_metrics.distance),
                community: u32::try_from(community_id).unwrap_or(u32::MAX),
            }
        })
        .collect();

    modules.sort_by(|a, b| compare_path_strings(&a.path, &b.path));
    modules
}

fn build_seams(graphs: &Graphs, analysis: &SeamAnalysis) -> Vec<Seam> {
    analysis
        .seams
        .iter()
        .take(MAX_REPORTED_SEAMS)
        .map(|scored| Seam {
            from: path_to_string(&graphs.files[scored.from].path),
            to: path_to_string(&graphs.files[scored.to].path),
            score: round(scored.score),
            hard: scored.hard,
            betweenness: round(scored.terms.betweenness),
            crosses_community: scored.terms.crosses_community >= 0.5,
            abstractness_target: round(scored.terms.abstractness),
            cochange: round(1.0 - scored.terms.inverse_cochange),
            test_proximity: round(scored.terms.tested),
        })
        .collect()
}

fn build_bridges(graphs: &Graphs, analysis: &SeamAnalysis) -> Vec<Bridge> {
    let mut bridges: Vec<Bridge> = analysis
        .structure
        .bridges
        .iter()
        .map(|bridge| Bridge {
            from: path_to_string(&graphs.files[bridge.from].path),
            to: path_to_string(&graphs.files[bridge.to].path),
            hard: true,
        })
        .collect();

    bridges.sort_by(|a, b| {
        compare_path_strings(&a.from, &b.from).then_with(|| compare_path_strings(&a.to, &b.to))
    });
    bridges
}

fn build_hotspots(graphs: &Graphs, cochange: &CoChange) -> Vec<Hotspot> {
    struct Raw {
        node: NodeIndex,
        path: String,
        ca: u32,
        distance: f64,
        churn: u32,
    }

    let raw: Vec<Raw> = graphs
        .files
        .node_indices()
        .map(|node| {
            let file = &graphs.files[node];
            let node_metrics = metrics::for_node(
                &graphs.files,
                node,
                file.total_defs,
                file.interface_like_defs,
            );
            Raw {
                node,
                path: path_to_string(&file.path),
                ca: node_metrics.ca,
                distance: node_metrics.distance,
                churn: cochange.touched(&file.path),
            }
        })
        .collect();

    let ca_values: BTreeMap<NodeIndex, f64> =
        raw.iter().map(|r| (r.node, f64::from(r.ca))).collect();
    let churn_values: BTreeMap<NodeIndex, f64> =
        raw.iter().map(|r| (r.node, f64::from(r.churn))).collect();
    let ca_norm = min_max_normalise(&ca_values);
    let churn_norm = min_max_normalise(&churn_values);

    let mut ranked: Vec<(&Raw, f64)> = raw
        .iter()
        .map(|r| {
            let score = HOTSPOT_CA_WEIGHT * ca_norm.get(&r.node).copied().unwrap_or(0.0)
                + HOTSPOT_D_WEIGHT * r.distance
                + HOTSPOT_CHURN_WEIGHT * churn_norm.get(&r.node).copied().unwrap_or(0.0);
            (r, score)
        })
        .collect();

    ranked.sort_by(|a, b| {
        b.1.total_cmp(&a.1)
            .then_with(|| compare_path_strings(&a.0.path, &b.0.path))
    });

    ranked
        .into_iter()
        .take(MAX_REPORTED_HOTSPOTS)
        .map(|(r, _)| Hotspot {
            path: r.path.clone(),
            ca: r.ca,
            d: round(r.distance),
            churn: r.churn,
        })
        .collect()
}

fn count_unresolved_refs(files: &[FileSymbols]) -> u32 {
    let count = files
        .iter()
        .flat_map(|file| file.refs.iter())
        .filter(|reference| reference.resolved_to.is_none())
        .count();
    u32::try_from(count).unwrap_or(u32::MAX)
}

/// Builds the report [`Document`] from `sources`.
///
/// Infallible: every input is already-computed data, and there is no
/// further validation to fail. The five weights summing to one, for
/// example, is [`crate::seam::analyse`]'s own concern — a [`SeamAnalysis`]
/// this function receives already passed that check.
#[must_use]
pub fn build(sources: &Sources<'_>) -> Document {
    let config_hash = config_hash::compute(
        sources.weights,
        sources.cochange.window,
        sources.discover_options,
    );

    Document {
        version: VERSION,
        tree_sha: sources.tree_sha.to_string(),
        config_hash: config_hash.to_string(),
        stats: build_stats(sources),
        modules: build_modules(sources.graphs),
        seams: build_seams(sources.graphs, sources.analysis),
        bridges: build_bridges(sources.graphs, sources.analysis),
        hotspots: build_hotspots(sources.graphs, sources.cochange),
        unresolved_refs: count_unresolved_refs(sources.files),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_keeps_four_decimal_places() {
        assert!((round(0.812_345) - 0.8123).abs() < f64::EPSILON);
    }

    #[test]
    fn round_folds_negative_zero_to_positive_zero() {
        let rounded = round(-0.000_001);
        assert!(rounded.abs() < f64::EPSILON);
        assert!(!rounded.is_sign_negative(), "must not serialise as -0.0");
    }

    #[test]
    fn min_max_normalise_rescales_into_the_unit_range() {
        let values: BTreeMap<NodeIndex, f64> = [
            (NodeIndex::new(0), 0.0),
            (NodeIndex::new(1), 5.0),
            (NodeIndex::new(2), 10.0),
        ]
        .into_iter()
        .collect();
        let scaled = min_max_normalise(&values);
        assert!(scaled[&NodeIndex::new(0)].abs() < f64::EPSILON);
        assert!((scaled[&NodeIndex::new(1)] - 0.5).abs() < 1e-9);
        assert!((scaled[&NodeIndex::new(2)] - 1.0).abs() < 1e-9);
    }

    #[test]
    fn min_max_normalise_handles_equal_values_without_dividing_by_zero() {
        let values: BTreeMap<NodeIndex, f64> = [(NodeIndex::new(0), 3.0), (NodeIndex::new(1), 3.0)]
            .into_iter()
            .collect();
        let scaled = min_max_normalise(&values);
        assert!(scaled.values().all(|v| v.abs() < f64::EPSILON));
    }

    #[test]
    fn building_an_empty_repository_produces_an_empty_report() {
        let graphs = crate::graph::build(&[]);
        let analysis =
            crate::seam::analyse(&graphs, &CoChange::default(), &Weights::default()).unwrap();
        let discover_options = DiscoverOptions::default();
        let weights = Weights::default();
        let sources = Sources {
            files: &[],
            graphs: &graphs,
            analysis: &analysis,
            cochange: &CoChange::default(),
            discover_options: &discover_options,
            weights: &weights,
            tree_sha: blake3::hash(b""),
        };

        let document = build(&sources);
        assert_eq!(document.version, VERSION);
        assert!(document.modules.is_empty());
        assert!(document.seams.is_empty());
        assert!(document.bridges.is_empty());
        assert!(document.hotspots.is_empty());
        assert_eq!(document.unresolved_refs, 0);
    }
}
