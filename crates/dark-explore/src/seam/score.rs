//! The seam score, and the blast radius it bounds.
//!
//! See Do steps 7 to 9 of task unit `F3`.
//!
//! ```text
//! seam(e) = 0.35 × B(e)        normalised edge betweenness
//!         + 0.25 × X(e)        1 when e crosses a community boundary
//!         + 0.20 × A(v)        abstractness of the target
//!         + 0.10 × (1 - C(e))  inverse co-change
//!         + 0.10 × T(e)        fraction of {u, v} that tests reference
//! ```
//!
//! Every term is in the range 0 to 1, so the score is too.
//!
//! # The co-change term is the one that earns its place
//!
//! The first three terms all read the import graph, and a boundary can look
//! clean to all three while being no boundary at all. Two files that always
//! change together are not really separate, whatever their imports say. The
//! inverse co-change term is what notices.

use std::collections::{BTreeMap, BTreeSet};

use petgraph::Direction;
use petgraph::graph::{DiGraph, EdgeIndex, NodeIndex};
use petgraph::visit::EdgeRef;

/// A seam score at or above this bounds a blast radius. Do step 9 of task
/// unit `F3`.
pub const BOUNDING_THRESHOLD: f64 = 0.6;

/// The weights of the five terms.
///
/// Do step 7 of task unit `F3` reads these from the configuration and puts
/// them in the configuration hash: different weights are a different
/// analysis, so the output must not be mistaken for one produced under
/// other weights.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Weights {
    /// Normalised edge betweenness.
    pub betweenness: f64,
    /// Crossing a community boundary.
    pub crosses_community: f64,
    /// Abstractness of the target.
    pub abstractness: f64,
    /// Inverse co-change.
    pub inverse_cochange: f64,
    /// Test reference.
    pub tested: f64,
}

impl Default for Weights {
    fn default() -> Self {
        Self {
            betweenness: 0.35,
            crosses_community: 0.25,
            abstractness: 0.20,
            inverse_cochange: 0.10,
            tested: 0.10,
        }
    }
}

impl Weights {
    /// Whether these weights sum to 1, within floating-point tolerance.
    ///
    /// A set that does not sum to 1 produces a score outside the range 0 to
    /// 1, which makes [`BOUNDING_THRESHOLD`] mean something different. A
    /// caller reading weights from a configuration should check this.
    #[must_use]
    pub fn sum_to_one(&self) -> bool {
        let total = self.betweenness
            + self.crosses_community
            + self.abstractness
            + self.inverse_cochange
            + self.tested;
        (total - 1.0).abs() < 1e-9
    }
}

/// The five terms of one edge's score, kept apart so a report can say why
/// an edge scored what it did.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Terms {
    /// `B(e)`: normalised edge betweenness.
    pub betweenness: f64,
    /// `X(e)`: 1 when the edge crosses a community boundary.
    pub crosses_community: f64,
    /// `A(v)`: abstractness of the target.
    pub abstractness: f64,
    /// `1 - C(e)`: inverse co-change.
    pub inverse_cochange: f64,
    /// `T(e)`: the fraction of the edge's two endpoints that a test
    /// references.
    pub tested: f64,
}

impl Terms {
    /// Combines the terms under `weights`.
    #[must_use]
    pub fn score(&self, weights: &Weights) -> f64 {
        weights.betweenness * self.betweenness
            + weights.crosses_community * self.crosses_community
            + weights.abstractness * self.abstractness
            + weights.inverse_cochange * self.inverse_cochange
            + weights.tested * self.tested
    }
}

/// One scored edge.
#[derive(Debug, Clone, PartialEq)]
pub struct ScoredSeam {
    /// The edge.
    pub edge: EdgeIndex,
    /// Where it starts.
    pub from: NodeIndex,
    /// Where it points.
    pub to: NodeIndex,
    /// The combined score, from 0 to 1.
    pub score: f64,
    /// The five terms that produced it.
    pub terms: Terms,
    /// Whether this edge is a bridge. Do step 8: a bridge is reported
    /// whatever it scores, because removing it splits the graph regardless
    /// of what the other terms say.
    pub hard: bool,
}

/// Ranks scored seams, highest first.
///
/// Ties break by edge index, so the order is total and reproducible rather
/// than depending on the sort's stability. Rule 32.
#[must_use]
pub fn rank(mut seams: Vec<ScoredSeam>) -> Vec<ScoredSeam> {
    seams.sort_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then_with(|| a.edge.index().cmp(&b.edge.index()))
    });
    seams
}

/// How far a change to one symbol set can reach.
#[derive(Debug, Clone, PartialEq)]
pub struct BlastRadius {
    /// Everything that can reach the starting set, following references
    /// backwards.
    pub reachable: BTreeSet<NodeIndex>,
    /// The same walk, stopped at any edge scoring at or above
    /// [`BOUNDING_THRESHOLD`].
    pub bounded: BTreeSet<NodeIndex>,
    /// The edges that stopped the bounded walk.
    pub bounding_seams: Vec<EdgeIndex>,
}

impl BlastRadius {
    /// How much of the unbounded reach the seams cut away, from 0 to 1.
    ///
    /// This is the useful number. A large reachable set with a small
    /// bounded one means a seam already limits the change. Returns 0 when
    /// nothing is reachable at all.
    #[must_use]
    pub fn containment(&self) -> f64 {
        if self.reachable.is_empty() {
            return 0.0;
        }
        // A graph large enough to lose precision here would need more
        // nodes than a repository can hold, but say so rather than cast and
        // hope.
        let reachable = u32::try_from(self.reachable.len()).unwrap_or(u32::MAX);
        let bounded = u32::try_from(self.bounded.len()).unwrap_or(u32::MAX);
        1.0 - f64::from(bounded) / f64::from(reachable)
    }
}

/// Computes the blast radius of a symbol set.
///
/// Walks the graph backwards from `start`: everything that references a
/// member of the set, then everything that references those, and so on. The
/// bounded walk is the same traversal, stopped at any edge whose score
/// reaches `threshold`.
#[must_use]
pub fn blast_radius<N, E>(
    graph: &DiGraph<N, E>,
    start: &BTreeSet<NodeIndex>,
    scores: &BTreeMap<EdgeIndex, f64>,
    threshold: f64,
) -> BlastRadius {
    let reachable = reverse_reachable(graph, start, scores, f64::INFINITY).0;
    let (bounded, bounding_seams) = reverse_reachable(graph, start, scores, threshold);

    BlastRadius {
        reachable,
        bounded,
        bounding_seams,
    }
}

/// The reverse-reachable set, stopping at any edge scoring at or above
/// `threshold`.
///
/// Returns the set and the edges that stopped it. A threshold of infinity
/// stops at nothing, which is the unbounded walk.
fn reverse_reachable<N, E>(
    graph: &DiGraph<N, E>,
    start: &BTreeSet<NodeIndex>,
    scores: &BTreeMap<EdgeIndex, f64>,
    threshold: f64,
) -> (BTreeSet<NodeIndex>, Vec<EdgeIndex>) {
    let mut seen: BTreeSet<NodeIndex> = start.clone();
    let mut stopped: BTreeSet<EdgeIndex> = BTreeSet::new();
    // A queue rather than recursion, and a sorted set for `seen`, so the
    // walk order does not depend on a hash map.
    let mut queue: std::collections::VecDeque<NodeIndex> = start.iter().copied().collect();

    while let Some(node) = queue.pop_front() {
        for edge in graph.edges_directed(node, Direction::Incoming) {
            let source = edge.source();
            let score = scores.get(&edge.id()).copied().unwrap_or(0.0);
            if score >= threshold {
                stopped.insert(edge.id());
                continue;
            }
            if seen.insert(source) {
                queue.push_back(source);
            }
        }
    }

    (seen, stopped.into_iter().collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn terms(
        betweenness: f64,
        crosses: f64,
        abstractness: f64,
        cochange: f64,
        tested: f64,
    ) -> Terms {
        Terms {
            betweenness,
            crosses_community: crosses,
            abstractness,
            inverse_cochange: 1.0 - cochange,
            tested,
        }
    }

    #[test]
    fn the_default_weights_sum_to_one() {
        assert!(
            Weights::default().sum_to_one(),
            "a score outside 0 to 1 would make the bounding threshold mean something else"
        );
    }

    #[test]
    fn weights_that_do_not_sum_to_one_are_reported_as_such() {
        let wrong = Weights {
            betweenness: 0.9,
            ..Weights::default()
        };
        assert!(!wrong.sum_to_one());
    }

    #[test]
    fn every_term_at_its_maximum_scores_one() {
        let all_high = terms(1.0, 1.0, 1.0, 0.0, 1.0);
        let score = all_high.score(&Weights::default());
        assert!((score - 1.0).abs() < 1e-9, "got {score}");
    }

    #[test]
    fn every_term_at_its_minimum_scores_zero() {
        // Co-change of 1 means the two sides always change together, so the
        // inverse term is 0.
        let all_low = terms(0.0, 0.0, 0.0, 1.0, 0.0);
        assert!(all_low.score(&Weights::default()).abs() < 1e-9);
    }

    #[test]
    fn a_boundary_whose_sides_always_change_together_scores_below_one_that_does_not() {
        // Identical structurally: the only difference is the history.
        let independent = terms(0.8, 1.0, 0.5, 0.0, 0.5);
        let always_together = terms(0.8, 1.0, 0.5, 1.0, 0.5);

        let weights = Weights::default();
        assert!(
            independent.score(&weights) > always_together.score(&weights),
            "a boundary whose two sides always change together is a poor seam, \
             however clean its structure looks"
        );
    }

    fn seam(edge: usize, score: f64) -> ScoredSeam {
        ScoredSeam {
            edge: EdgeIndex::new(edge),
            from: NodeIndex::new(0),
            to: NodeIndex::new(1),
            score,
            terms: terms(0.0, 0.0, 0.0, 1.0, 0.0),
            hard: false,
        }
    }

    #[test]
    fn ranking_puts_the_highest_score_first() {
        let ranked = rank(vec![seam(0, 0.2), seam(1, 0.9), seam(2, 0.5)]);
        let order: Vec<usize> = ranked.iter().map(|s| s.edge.index()).collect();
        assert_eq!(order, vec![1, 2, 0]);
    }

    #[test]
    fn ranking_breaks_a_tie_by_edge_index_rather_than_by_luck() {
        let ranked = rank(vec![seam(2, 0.5), seam(0, 0.5), seam(1, 0.5)]);
        let order: Vec<usize> = ranked.iter().map(|s| s.edge.index()).collect();
        assert_eq!(order, vec![0, 1, 2], "Rule 32: the order must be total");
    }

    /// `a -> b -> c -> d`, so everything reaches `d` backwards.
    fn chain() -> DiGraph<(), ()> {
        let mut graph = DiGraph::new();
        let nodes: Vec<NodeIndex> = (0..4).map(|_| graph.add_node(())).collect();
        for pair in nodes.windows(2) {
            graph.add_edge(pair[0], pair[1], ());
        }
        graph
    }

    #[test]
    fn an_unbounded_blast_radius_reaches_everything_upstream() {
        let graph = chain();
        let start: BTreeSet<NodeIndex> = [NodeIndex::new(3)].into_iter().collect();
        let found = blast_radius(&graph, &start, &BTreeMap::new(), BOUNDING_THRESHOLD);

        assert_eq!(found.reachable.len(), 4, "every node reaches the last one");
        assert_eq!(
            found.bounded.len(),
            4,
            "with no edge scoring, nothing bounds the walk"
        );
        assert!(found.bounding_seams.is_empty());
    }

    #[test]
    fn a_high_scoring_edge_bounds_the_walk_and_is_reported() {
        let graph = chain();
        let start: BTreeSet<NodeIndex> = [NodeIndex::new(3)].into_iter().collect();
        // Score the middle edge, b -> c, above the threshold.
        let scores: BTreeMap<EdgeIndex, f64> = [(EdgeIndex::new(1), 0.8)].into_iter().collect();

        let found = blast_radius(&graph, &start, &scores, BOUNDING_THRESHOLD);

        assert_eq!(found.reachable.len(), 4, "the unbounded walk is unaffected");
        assert_eq!(
            found.bounded.len(),
            2,
            "the seam stops the walk at c, so only c and d remain"
        );
        assert_eq!(found.bounding_seams, vec![EdgeIndex::new(1)]);
    }

    #[test]
    fn containment_is_the_share_the_seam_cut_away() {
        let graph = chain();
        let start: BTreeSet<NodeIndex> = [NodeIndex::new(3)].into_iter().collect();
        let scores: BTreeMap<EdgeIndex, f64> = [(EdgeIndex::new(1), 0.8)].into_iter().collect();

        let found = blast_radius(&graph, &start, &scores, BOUNDING_THRESHOLD);
        // 4 reachable, 2 bounded: the seam cut half away.
        assert!((found.containment() - 0.5).abs() < 1e-9);
    }

    #[test]
    fn an_edge_exactly_at_the_threshold_bounds_the_walk() {
        let graph = chain();
        let start: BTreeSet<NodeIndex> = [NodeIndex::new(3)].into_iter().collect();
        let scores: BTreeMap<EdgeIndex, f64> = [(EdgeIndex::new(1), BOUNDING_THRESHOLD)]
            .into_iter()
            .collect();

        let found = blast_radius(&graph, &start, &scores, BOUNDING_THRESHOLD);
        assert_eq!(
            found.bounded.len(),
            2,
            "Do step 9 says at or above the threshold, not above it"
        );
    }

    #[test]
    fn a_cycle_terminates_rather_than_looping_forever() {
        let mut graph = DiGraph::new();
        let nodes: Vec<NodeIndex> = (0..3).map(|_| graph.add_node(())).collect();
        graph.add_edge(nodes[0], nodes[1], ());
        graph.add_edge(nodes[1], nodes[2], ());
        graph.add_edge(nodes[2], nodes[0], ());

        let start: BTreeSet<NodeIndex> = [nodes[0]].into_iter().collect();
        let found = blast_radius(&graph, &start, &BTreeMap::new(), BOUNDING_THRESHOLD);
        assert_eq!(found.reachable.len(), 3);
    }

    #[test]
    fn a_start_set_with_nothing_upstream_reaches_only_itself() {
        let graph = chain();
        let start: BTreeSet<NodeIndex> = [NodeIndex::new(0)].into_iter().collect();
        let found = blast_radius(&graph, &start, &BTreeMap::new(), BOUNDING_THRESHOLD);

        assert_eq!(found.reachable, start);
        assert!(
            found.containment().abs() < f64::EPSILON,
            "nothing to cut away means nothing was cut away"
        );
    }
}
