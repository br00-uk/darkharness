//! Edge betweenness with Brandes' algorithm.
//!
//! An edge with high betweenness carries many of the graph's shortest
//! paths. Cutting there separates more than cutting elsewhere, which is
//! what makes it a candidate seam. See Do step 5 of task unit `F3`.
//!
//! # Sampling, and why it is not random
//!
//! Brandes is one full traversal per source node, so the exact figure costs
//! `O(nodes × edges)`. Above [`SAMPLE_THRESHOLD`] nodes that is too slow,
//! so this samples sources — but Rule 32 requires the same bytes out for
//! the same commit, and a random sample would break that. It therefore
//! takes every `k`-th node by node index, which `graph::build` fixed to
//! sorted path order using F1's byte comparator. The result records both
//! that it sampled and the `k` it used, because a sampled figure and an
//! exact one are not the same number and the output must not pretend
//! otherwise.

use std::collections::BTreeMap;

use petgraph::Direction;
use petgraph::graph::{DiGraph, EdgeIndex, NodeIndex};
use petgraph::visit::EdgeRef;

/// Above this many nodes, sample sources rather than walking them all.
/// Do step 5 of task unit `F3`.
pub const SAMPLE_THRESHOLD: usize = 5_000;

/// Roughly how many sources to sample once past [`SAMPLE_THRESHOLD`].
pub const TARGET_SAMPLES: usize = 1_000;

/// Edge betweenness over one graph.
#[derive(Debug, Clone, PartialEq)]
pub struct Betweenness {
    /// The raw score per edge. An edge no shortest path uses is absent.
    pub of_edge: BTreeMap<EdgeIndex, f64>,
    /// Whether the sources were sampled rather than walked in full.
    pub sampled: bool,
    /// The sampling stride. `1` when every source was walked.
    pub k: usize,
}

impl Betweenness {
    /// The score for one edge, normalised to the range 0 to 1 by the
    /// largest score in the graph.
    ///
    /// This is the `B(e)` term of the seam score. A graph whose every edge
    /// scores zero yields zero rather than dividing by zero.
    #[must_use]
    pub fn normalised(&self, edge: EdgeIndex) -> f64 {
        let highest = self
            .of_edge
            .values()
            .copied()
            .fold(0.0_f64, |best, score| best.max(score));
        if highest <= 0.0 {
            return 0.0;
        }
        self.of_edge.get(&edge).copied().unwrap_or(0.0) / highest
    }
}

/// Chooses the sampling stride for a graph of `node_count` nodes.
///
/// Returns 1 below the threshold, meaning every source is walked.
#[must_use]
pub fn stride_for(node_count: usize) -> usize {
    if node_count <= SAMPLE_THRESHOLD {
        return 1;
    }
    // Round up, so the sample never exceeds the target.
    node_count.div_ceil(TARGET_SAMPLES).max(1)
}

/// Computes edge betweenness.
///
/// Brandes' algorithm: for each source, a breadth-first pass forwards to
/// count shortest paths, then an accumulation backwards to share the
/// dependency out over the edges that carry them.
#[must_use]
pub fn compute<N, E>(graph: &DiGraph<N, E>) -> Betweenness {
    let node_count = graph.node_count();
    let k = stride_for(node_count);
    let sources: Vec<NodeIndex> = graph.node_indices().step_by(k).collect();

    let mut of_edge: BTreeMap<EdgeIndex, f64> = BTreeMap::new();
    for source in sources {
        accumulate_from(graph, source, &mut of_edge);
    }

    Betweenness {
        of_edge,
        sampled: k > 1,
        k,
    }
}

/// One source's contribution to every edge's betweenness.
fn accumulate_from<N, E>(
    graph: &DiGraph<N, E>,
    source: NodeIndex,
    of_edge: &mut BTreeMap<EdgeIndex, f64>,
) {
    // Forward pass: shortest-path counts and the predecessor edges that
    // achieve them.
    let mut order: Vec<NodeIndex> = Vec::new();
    let mut predecessors: BTreeMap<NodeIndex, Vec<(NodeIndex, EdgeIndex)>> = BTreeMap::new();
    let mut paths: BTreeMap<NodeIndex, f64> = BTreeMap::new();
    let mut distance: BTreeMap<NodeIndex, i64> = BTreeMap::new();

    paths.insert(source, 1.0);
    distance.insert(source, 0);

    let mut queue = std::collections::VecDeque::new();
    queue.push_back(source);

    while let Some(node) = queue.pop_front() {
        order.push(node);
        let node_distance = distance[&node];
        let node_paths = paths[&node];

        // Neighbours in edge order, which petgraph fixes, so the traversal
        // does not depend on a hash map's iteration order.
        for edge in graph.edges_directed(node, Direction::Outgoing) {
            let neighbour = edge.target();
            if neighbour == node {
                continue;
            }
            match distance.get(&neighbour) {
                None => {
                    distance.insert(neighbour, node_distance + 1);
                    paths.insert(neighbour, node_paths);
                    predecessors
                        .entry(neighbour)
                        .or_default()
                        .push((node, edge.id()));
                    queue.push_back(neighbour);
                }
                Some(&seen) if seen == node_distance + 1 => {
                    *paths.entry(neighbour).or_insert(0.0) += node_paths;
                    predecessors
                        .entry(neighbour)
                        .or_default()
                        .push((node, edge.id()));
                }
                // A neighbour already closer than this route is not on a
                // shortest path through this node.
                Some(_) => {}
            }
        }
    }

    // Backward pass: share each node's dependency over the edges that
    // carried a shortest path into it.
    let mut dependency: BTreeMap<NodeIndex, f64> = BTreeMap::new();
    for &node in order.iter().rev() {
        let node_dependency = 1.0 + dependency.get(&node).copied().unwrap_or(0.0);
        let node_paths = paths.get(&node).copied().unwrap_or(0.0);
        if node_paths <= 0.0 {
            continue;
        }
        if let Some(incoming) = predecessors.get(&node) {
            for &(predecessor, edge) in incoming {
                let predecessor_paths = paths.get(&predecessor).copied().unwrap_or(0.0);
                let share = predecessor_paths / node_paths * node_dependency;
                *of_edge.entry(edge).or_insert(0.0) += share;
                *dependency.entry(predecessor).or_insert(0.0) += share;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn graph_from(edges: &[(usize, usize)], node_count: usize) -> DiGraph<(), ()> {
        let mut graph = DiGraph::new();
        let nodes: Vec<NodeIndex> = (0..node_count).map(|_| graph.add_node(())).collect();
        for &(from, to) in edges {
            graph.add_edge(nodes[from], nodes[to], ());
        }
        graph
    }

    #[test]
    fn the_middle_edge_of_a_chain_carries_the_most_traffic() {
        // 0 -> 1 -> 2 -> 3: the middle edge lies on more shortest paths
        // than either end.
        let graph = graph_from(&[(0, 1), (1, 2), (2, 3)], 4);
        let found = compute(&graph);

        let first = found.of_edge[&EdgeIndex::new(0)];
        let middle = found.of_edge[&EdgeIndex::new(1)];
        let last = found.of_edge[&EdgeIndex::new(2)];

        assert!(
            middle > first && middle > last,
            "middle {middle} should beat {first} and {last}"
        );
    }

    #[test]
    fn the_edge_joining_two_clusters_scores_highest() {
        // Two triangles joined by 2 -> 3. Every path from the first cluster
        // to the second runs through that one edge.
        let graph = graph_from(&[(0, 1), (1, 2), (2, 0), (3, 4), (4, 5), (5, 3), (2, 3)], 6);
        let found = compute(&graph);

        let joining = found
            .of_edge
            .iter()
            .max_by(|a, b| a.1.partial_cmp(b.1).expect("no NaN"))
            .map(|(edge, _)| *edge)
            .expect("some edge scores");

        let endpoints = graph.edge_endpoints(joining).expect("the edge exists");
        assert_eq!(
            (endpoints.0.index(), endpoints.1.index()),
            (2, 3),
            "the bridge between the clusters must score highest"
        );
    }

    #[test]
    fn normalising_puts_the_top_edge_at_one_and_stays_in_range() {
        let graph = graph_from(&[(0, 1), (1, 2), (2, 3)], 4);
        let found = compute(&graph);

        for edge in graph.edge_indices() {
            let score = found.normalised(edge);
            assert!(
                (0.0..=1.0).contains(&score),
                "edge {edge:?} normalised to {score}, outside 0 to 1"
            );
        }
        let top = graph
            .edge_indices()
            .map(|e| found.normalised(e))
            .fold(0.0_f64, f64::max);
        assert!((top - 1.0).abs() < 1e-9);
    }

    #[test]
    fn a_graph_with_no_edges_normalises_to_zero_rather_than_dividing_by_zero() {
        let graph = graph_from(&[], 3);
        let found = compute(&graph);
        let score = found.normalised(EdgeIndex::new(0));
        assert!(score.is_finite(), "must not be NaN");
        assert!(score.abs() < f64::EPSILON);
    }

    #[test]
    fn a_small_graph_walks_every_source() {
        let graph = graph_from(&[(0, 1), (1, 2)], 3);
        let found = compute(&graph);
        assert!(!found.sampled);
        assert_eq!(found.k, 1);
    }

    #[test]
    fn the_stride_stays_at_one_up_to_the_threshold() {
        assert_eq!(stride_for(0), 1);
        assert_eq!(stride_for(SAMPLE_THRESHOLD), 1);
    }

    #[test]
    fn past_the_threshold_the_stride_keeps_the_sample_near_the_target() {
        for node_count in [SAMPLE_THRESHOLD + 1, 10_000, 50_000, 1_000_000] {
            let k = stride_for(node_count);
            assert!(k > 1, "{node_count} nodes should sample");
            let samples = node_count.div_ceil(k);
            assert!(
                samples <= TARGET_SAMPLES,
                "{node_count} nodes with stride {k} gives {samples} sources, over the target"
            );
        }
    }

    #[test]
    fn the_result_is_identical_across_repeated_runs() {
        let graph = graph_from(&[(0, 1), (1, 2), (2, 0), (2, 3), (3, 4), (4, 5), (5, 3)], 6);
        let first = compute(&graph);
        for _ in 0..5 {
            assert_eq!(
                compute(&graph),
                first,
                "Rule 32: sampling by stride, never at random, is what makes this reproducible"
            );
        }
    }

    #[test]
    fn a_self_loop_carries_no_shortest_path() {
        let graph = graph_from(&[(0, 0), (0, 1)], 2);
        let found = compute(&graph);
        assert!(
            !found.of_edge.contains_key(&EdgeIndex::new(0)),
            "a self-loop is on no shortest path between two nodes"
        );
    }
}
