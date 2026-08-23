//! Assembles the five terms into scored seams.
//!
//! The modules beside this one each compute one ingredient: metrics,
//! bridges, communities, betweenness, and co-change. This module is where
//! they meet the formula of Do step 7 of task unit `F3`, over the F-graph.
//!
//! # Why the F-graph
//!
//! Co-change is a fact about files: git records which files a commit
//! touched, not which symbols. A seam is a place to cut, and a cut runs
//! between files. The formula therefore scores F-graph edges, and
//! [`symbol_scores`] projects those scores onto the S-graph for the blast
//! radius, whose traversal task unit `F3` defines over symbols.
//!
//! # Normalisation
//!
//! Do step 7 says to normalise `B` and `C` by minimum and maximum across
//! all edges. Both terms are therefore relative to this graph: the same
//! edge in a different repository gets a different score. When every edge
//! has the same raw value there is no spread to normalise, and the term is
//! 0 for every edge rather than a division by zero.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use dark_contract::{ErrCode, Error, Result};
use petgraph::graph::{EdgeIndex, NodeIndex};
use petgraph::visit::EdgeRef;

use crate::graph::Graphs;
use crate::seam::betweenness::{self, Betweenness};
use crate::seam::cochange::CoChange;
use crate::seam::community::{self, Communities};
use crate::seam::metrics;
use crate::seam::score::{self, ScoredSeam, Terms, Weights, rank};
use crate::seam::structure::{self, Structure};

/// Everything one seam pass computed, kept so a report can show its
/// working rather than only its conclusion.
#[derive(Debug, Clone, PartialEq)]
pub struct SeamAnalysis {
    /// Every F-graph edge, scored and ranked highest first. A bridge is
    /// marked `hard` whatever it scores.
    pub seams: Vec<ScoredSeam>,
    /// The community partition the `X` term read.
    pub communities: Communities,
    /// The bridges and articulation points the `hard` flag read.
    pub structure: Structure,
    /// The raw betweenness the `B` term was normalised from, with its
    /// sampling record.
    pub betweenness: Betweenness,
}

/// Whether a path names a test file.
///
/// The judgement is by path convention, because that is what survives
/// across the thirteen grammars without a parser: a `tests` or `test`
/// directory anywhere in the path, or a file name in one of the common
/// test-naming shapes.
#[must_use]
pub fn is_test_path(path: &std::path::Path) -> bool {
    let in_test_dir = path.components().any(|part| {
        matches!(
            part.as_os_str().to_str(),
            Some("tests" | "test" | "__tests__")
        )
    });
    if in_test_dir {
        return true;
    }

    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    let stem = name.split('.').next().unwrap_or(name);
    stem.starts_with("test_")
        || stem.ends_with("_test")
        || name.contains(".test.")
        || name.contains(".spec.")
}

/// Scores every F-graph edge and ranks the result.
///
/// # Errors
///
/// Returns [`ErrCode::ExploreParse`] when `weights` does not sum to one: the
/// score would leave the range 0 to 1, and the bounding threshold would
/// silently mean something different.
pub fn analyse(graphs: &Graphs, cochange: &CoChange, weights: &Weights) -> Result<SeamAnalysis> {
    if !weights.sum_to_one() {
        return Err(Error::new(
            ErrCode::ExploreParse,
            "the seam weights do not sum to one, so the score would leave the range 0 to 1",
        )
        .with_remedy("Make the five seam weights in the configuration sum to one."));
    }

    let files = &graphs.files;
    let communities = community::detect(files);
    let structure = structure::find(files);
    let betweenness = betweenness::compute(files);

    let bridges: BTreeSet<EdgeIndex> = structure.bridges.iter().map(|b| b.edge).collect();
    let tested = test_referenced(graphs);

    // Raw values first, so both normalisations see every edge.
    let mut raw_b: BTreeMap<EdgeIndex, f64> = BTreeMap::new();
    let mut raw_c: BTreeMap<EdgeIndex, f64> = BTreeMap::new();
    for edge in files.edge_references() {
        raw_b.insert(
            edge.id(),
            betweenness.of_edge.get(&edge.id()).copied().unwrap_or(0.0),
        );
        raw_c.insert(
            edge.id(),
            cochange.coupling(&files[edge.source()].path, &files[edge.target()].path),
        );
    }
    let b_norm = min_max_normalise(&raw_b);
    let c_norm = min_max_normalise(&raw_c);

    let mut seams = Vec::with_capacity(files.edge_count());
    for edge in files.edge_references() {
        let (from, to) = (edge.source(), edge.target());
        let target = &files[to];
        let abstractness =
            metrics::for_node(files, to, target.total_defs, target.interface_like_defs)
                .abstractness;

        let terms = Terms {
            betweenness: b_norm.get(&edge.id()).copied().unwrap_or(0.0),
            crosses_community: if communities.crosses(from, to) {
                1.0
            } else {
                0.0
            },
            abstractness,
            inverse_cochange: 1.0 - c_norm.get(&edge.id()).copied().unwrap_or(0.0),
            tested: match (tested.contains(&from), tested.contains(&to)) {
                (true, true) => 1.0,
                (true, false) | (false, true) => 0.5,
                (false, false) => 0.0,
            },
        };

        seams.push(ScoredSeam {
            edge: edge.id(),
            from,
            to,
            score: terms.score(weights),
            terms,
            hard: bridges.contains(&edge.id()),
        });
    }

    Ok(SeamAnalysis {
        seams: rank(seams),
        communities,
        structure,
        betweenness,
    })
}

/// What a change to one named symbol can affect.
///
/// The answer to `dark blast <symbol>`, as plain data: no graph index
/// appears in it, so a caller needs no `petgraph` dependency of its own
/// to ask the question or to print the answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SymbolBlast {
    /// How many definitions in the repository carry this name.
    pub definitions: usize,
    /// How many other definitions reference it, directly or through
    /// others, with nothing stopping the walk.
    pub reachable: usize,
    /// How many of those are inside the nearest bounding seams.
    pub bounded: usize,
    /// The files holding the bounded definitions, sorted, without the
    /// files the named definitions are themselves in.
    pub files: Vec<PathBuf>,
    /// How many seams stopped the bounded walk.
    pub bounding_seams: usize,
}

impl SymbolBlast {
    /// How much of the unbounded reach the seams cut away, from 0 to 1.
    ///
    /// A large reach with a small bounded set means a seam already limits
    /// the change. Returns 0 when nothing references the symbol.
    #[must_use]
    pub fn containment(&self) -> f64 {
        if self.reachable == 0 {
            return 0.0;
        }
        // A repository cannot hold more definitions than this cast
        // handles, but say so rather than cast and hope.
        let reachable = u32::try_from(self.reachable).unwrap_or(u32::MAX);
        let bounded = u32::try_from(self.bounded).unwrap_or(u32::MAX);
        1.0 - f64::from(bounded) / f64::from(reachable)
    }
}

/// Computes what a change to every definition named `symbol` can affect.
///
/// Walks the S-graph backwards from those definitions, twice: once with
/// nothing stopping it, and once stopped at any edge whose projected seam
/// score reaches [`score::BOUNDING_THRESHOLD`]. See
/// [`score::blast_radius`], which does the walk, and [`symbol_scores`],
/// which projects the file seam scores onto the S-graph.
///
/// Returns `None` when no definition carries that name — which is a
/// different answer from "it affects nothing", and the caller should say
/// so differently.
///
/// The counts exclude the named definitions themselves: what a person
/// asked is what *else* a change would reach.
#[must_use]
pub fn blast_for_symbol(
    graphs: &Graphs,
    seams: &[ScoredSeam],
    symbol: &str,
) -> Option<SymbolBlast> {
    let start: BTreeSet<NodeIndex> = graphs
        .symbols
        .node_indices()
        .filter(|index| graphs.symbols[*index].name == symbol)
        .collect();
    if start.is_empty() {
        return None;
    }

    let scores = symbol_scores(graphs, seams);
    let radius = score::blast_radius(&graphs.symbols, &start, &scores, score::BOUNDING_THRESHOLD);

    let own_files: BTreeSet<&Path> = start
        .iter()
        .map(|index| graphs.symbols[*index].file.as_path())
        .collect();
    // Sorted and deduplicated: several affected definitions usually sit
    // in one file, and a list repeating one path says less than one
    // naming each file once. A file the symbol is itself defined in is
    // dropped — telling a person a change reaches its own file says
    // nothing they did not know.
    let files: Vec<PathBuf> = radius
        .bounded
        .difference(&start)
        .map(|index| graphs.symbols[*index].file.as_path())
        .filter(|path| !own_files.contains(path))
        .collect::<BTreeSet<&Path>>()
        .into_iter()
        .map(Path::to_path_buf)
        .collect();

    Some(SymbolBlast {
        definitions: start.len(),
        reachable: radius.reachable.len().saturating_sub(start.len()),
        bounded: radius.bounded.len().saturating_sub(start.len()),
        files,
        bounding_seams: radius.bounding_seams.len(),
    })
}

/// Projects file seam scores onto the S-graph, for the blast radius.
///
/// An S-graph edge between two symbols in different files takes the score
/// of the F-graph edge between those files. An edge inside one file scores
/// 0: a seam is a cut between files, and there is no seam inside one.
#[must_use]
pub fn symbol_scores(graphs: &Graphs, seams: &[ScoredSeam]) -> BTreeMap<EdgeIndex, f64> {
    // File pair to score, one lookup per S-edge.
    let by_files: BTreeMap<(&std::path::Path, &std::path::Path), f64> = seams
        .iter()
        .map(|seam| {
            (
                (
                    graphs.files[seam.from].path.as_path(),
                    graphs.files[seam.to].path.as_path(),
                ),
                seam.score,
            )
        })
        .collect();

    let mut scores = BTreeMap::new();
    for edge in graphs.symbols.edge_references() {
        let from_file = graphs.symbols[edge.source()].file.as_path();
        let to_file = graphs.symbols[edge.target()].file.as_path();
        if from_file == to_file {
            continue;
        }
        if let Some(&score) = by_files.get(&(from_file, to_file)) {
            scores.insert(edge.id(), score);
        }
    }
    scores
}

/// The F-graph nodes that at least one test file imports.
fn test_referenced(graphs: &Graphs) -> BTreeSet<NodeIndex> {
    let mut referenced = BTreeSet::new();
    for edge in graphs.files.edge_references() {
        if is_test_path(&graphs.files[edge.source()].path) {
            referenced.insert(edge.target());
        }
    }
    referenced
}

/// Rescales values into the range 0 to 1 by their minimum and maximum.
///
/// When every value is the same there is no spread to rescale, and every
/// value becomes 0 rather than a division by zero.
fn min_max_normalise(values: &BTreeMap<EdgeIndex, f64>) -> BTreeMap<EdgeIndex, f64> {
    let mut lowest = f64::INFINITY;
    let mut highest = f64::NEG_INFINITY;
    for &value in values.values() {
        lowest = lowest.min(value);
        highest = highest.max(value);
    }
    let spread = highest - lowest;

    values
        .iter()
        .map(|(&edge, &value)| {
            let scaled = if spread > 0.0 {
                (value - lowest) / spread
            } else {
                0.0
            };
            (edge, scaled)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    #[test]
    fn a_tests_directory_anywhere_in_the_path_marks_a_test_file() {
        assert!(is_test_path(Path::new("crates/x/tests/roundtrip.rs")));
        assert!(is_test_path(Path::new("pkg/__tests__/app.js")));
        assert!(!is_test_path(Path::new("src/testing_helpers.rs")));
    }

    #[test]
    fn common_test_file_names_are_recognised() {
        assert!(is_test_path(Path::new("src/engine_test.rs")));
        assert!(is_test_path(Path::new("src/test_engine.py")));
        assert!(is_test_path(Path::new("src/app.test.ts")));
        assert!(is_test_path(Path::new("src/app.spec.ts")));
        assert!(!is_test_path(Path::new("src/attest.rs")));
        assert!(!is_test_path(Path::new("src/contest_entry.rs")));
    }

    #[test]
    fn min_max_rescales_into_the_unit_range() {
        let values: BTreeMap<EdgeIndex, f64> = [
            (EdgeIndex::new(0), 2.0),
            (EdgeIndex::new(1), 4.0),
            (EdgeIndex::new(2), 6.0),
        ]
        .into_iter()
        .collect();

        let scaled = min_max_normalise(&values);
        assert!(scaled[&EdgeIndex::new(0)].abs() < f64::EPSILON);
        assert!((scaled[&EdgeIndex::new(1)] - 0.5).abs() < 1e-9);
        assert!((scaled[&EdgeIndex::new(2)] - 1.0).abs() < 1e-9);
    }

    #[test]
    fn identical_values_normalise_to_zero_rather_than_dividing_by_zero() {
        let values: BTreeMap<EdgeIndex, f64> = [(EdgeIndex::new(0), 3.0), (EdgeIndex::new(1), 3.0)]
            .into_iter()
            .collect();

        let scaled = min_max_normalise(&values);
        assert!(scaled.values().all(|v| v.abs() < f64::EPSILON));
    }

    #[test]
    fn weights_that_do_not_sum_to_one_are_refused_with_a_remedy() {
        let graphs = crate::graph::build(&[]);
        let cochange = CoChange::default();
        let wrong = Weights {
            betweenness: 0.9,
            ..Weights::default()
        };

        let error = analyse(&graphs, &cochange, &wrong).expect_err("must refuse");
        assert_eq!(error.code, ErrCode::ExploreParse);
        assert!(error.remedy.is_some(), "an error carries its remedy");
    }

    #[test]
    fn an_empty_repository_analyses_to_no_seams() {
        let graphs = crate::graph::build(&[]);
        let found = analyse(&graphs, &CoChange::default(), &Weights::default())
            .expect("an empty graph is not an error");
        assert!(found.seams.is_empty());
    }
}
