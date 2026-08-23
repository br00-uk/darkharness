//! Community detection with the Louvain method.
//!
//! A community is a cluster of nodes that reference each other far more
//! than they reference anything outside. An edge that crosses a community
//! boundary is a candidate seam, which is what Do step 7 of task unit `F3`
//! scores it for.
//!
//! # Why the visit order is fixed
//!
//! Louvain is greedy: it moves each node to whichever neighbouring
//! community gains the most modularity, and the result depends on the order
//! it considers nodes in. Two runs over the same graph in different orders
//! produce different, equally valid partitions. Rule 32 requires the same
//! bytes out for the same commit, so this implementation visits nodes in
//! node-index order — which `graph::build` fixed to sorted path order using
//! F1's byte comparator — and breaks every tie by the lower community
//! identifier. Do step 4 of task unit `F3` names the same settings: seed 0,
//! resolution 1.0, and at most 100 passes.

use std::collections::BTreeMap;

use petgraph::graph::{DiGraph, NodeIndex};
use petgraph::visit::EdgeRef;

/// The resolution parameter. 1.0 is standard modularity; a higher value
/// finds smaller communities. Do step 4 of task unit `F3` fixes it at 1.0.
pub const RESOLUTION: f64 = 1.0;

/// How many passes Louvain may make before it stops, whether or not it has
/// converged. Do step 4 of task unit `F3`.
pub const MAX_PASSES: usize = 100;

/// A partition of the graph into communities.
#[derive(Debug, Clone, PartialEq)]
pub struct Communities {
    /// Which community each node belongs to, by node index. Community
    /// identifiers are renumbered from zero in first-appearance order, so
    /// they are stable for a given graph.
    pub of_node: BTreeMap<NodeIndex, usize>,
    /// The modularity of this partition, between -0.5 and 1.0. Higher means
    /// the communities are more clearly separated.
    pub modularity: f64,
    /// How many passes ran before the partition stopped changing. Equal to
    /// [`MAX_PASSES`] when it never settled.
    pub passes: usize,
}

impl Communities {
    /// Whether these two nodes sit in different communities.
    ///
    /// This is the `X(e)` term of the seam score: an edge that crosses a
    /// community boundary is a candidate seam.
    #[must_use]
    pub fn crosses(&self, from: NodeIndex, to: NodeIndex) -> bool {
        match (self.of_node.get(&from), self.of_node.get(&to)) {
            (Some(a), Some(b)) => a != b,
            // A node the partition does not know cannot be said to cross
            // anything.
            _ => false,
        }
    }
}

/// The undirected, weighted view Louvain works on.
///
/// Modularity is an undirected idea. A directed edge and its reverse are
/// the same connection for this purpose, so their weights add.
struct Weighted {
    /// Neighbour weights per node, both directions folded together.
    adjacency: BTreeMap<NodeIndex, BTreeMap<NodeIndex, f64>>,
    /// The total weight of every edge incident on a node, self-loops
    /// counted twice, as modularity requires.
    degree: BTreeMap<NodeIndex, f64>,
    /// Twice the total edge weight. Zero for a graph with no edges.
    total: f64,
}

fn weighted_view<N, E>(graph: &DiGraph<N, E>) -> Weighted {
    let mut adjacency: BTreeMap<NodeIndex, BTreeMap<NodeIndex, f64>> = BTreeMap::new();
    for node in graph.node_indices() {
        adjacency.entry(node).or_default();
    }
    for edge in graph.edge_references() {
        let (source, target) = (edge.source(), edge.target());
        *adjacency
            .entry(source)
            .or_default()
            .entry(target)
            .or_insert(0.0) += 1.0;
        if source != target {
            *adjacency
                .entry(target)
                .or_default()
                .entry(source)
                .or_insert(0.0) += 1.0;
        }
    }

    let mut degree: BTreeMap<NodeIndex, f64> = BTreeMap::new();
    let mut total = 0.0;
    for (node, neighbours) in &adjacency {
        let mut sum = 0.0;
        for (neighbour, weight) in neighbours {
            // A self-loop contributes twice to a node's degree.
            sum += if neighbour == node {
                weight * 2.0
            } else {
                *weight
            };
        }
        degree.insert(*node, sum);
        total += sum;
    }

    Weighted {
        adjacency,
        degree,
        total,
    }
}

/// Computes the modularity of one partition.
fn modularity_of(view: &Weighted, of_node: &BTreeMap<NodeIndex, usize>) -> f64 {
    if view.total == 0.0 {
        return 0.0;
    }

    // Per community: the weight of edges wholly inside it, and the total
    // degree of its members.
    let mut inside: BTreeMap<usize, f64> = BTreeMap::new();
    let mut incident: BTreeMap<usize, f64> = BTreeMap::new();

    for (node, neighbours) in &view.adjacency {
        let Some(&community) = of_node.get(node) else {
            continue;
        };
        *incident.entry(community).or_insert(0.0) += view.degree.get(node).copied().unwrap_or(0.0);
        for (neighbour, weight) in neighbours {
            if of_node.get(neighbour) == Some(&community) {
                *inside.entry(community).or_insert(0.0) += weight;
            }
        }
    }

    let mut modularity = 0.0;
    for (community, incident_weight) in &incident {
        let inside_weight = inside.get(community).copied().unwrap_or(0.0);
        let share = incident_weight / view.total;
        modularity += inside_weight / view.total - RESOLUTION * share * share;
    }
    modularity
}

/// Partitions `graph` into communities.
///
/// Runs the local-moving phase of Louvain repeatedly. Each pass walks every
/// node in node-index order and moves it to whichever neighbouring
/// community gains the most modularity, breaking a tie by the lower
/// community identifier. The pass count stops at [`MAX_PASSES`].
#[must_use]
pub fn detect<N, E>(graph: &DiGraph<N, E>) -> Communities {
    let view = weighted_view(graph);

    // Every node starts in its own community.
    let mut of_node: BTreeMap<NodeIndex, usize> = graph
        .node_indices()
        .enumerate()
        .map(|(index, node)| (node, index))
        .collect();

    let mut passes = 0;
    if view.total > 0.0 {
        for pass in 1..=MAX_PASSES {
            passes = pass;
            let mut moved = false;

            // Node-index order, not the map's own order: see the module
            // note on why this is fixed.
            for node in graph.node_indices() {
                let current = of_node[&node];
                let node_degree = view.degree.get(&node).copied().unwrap_or(0.0);

                // The weight this node has into each neighbouring
                // community, itself excluded.
                let mut into: BTreeMap<usize, f64> = BTreeMap::new();
                if let Some(neighbours) = view.adjacency.get(&node) {
                    for (neighbour, weight) in neighbours {
                        if *neighbour == node {
                            continue;
                        }
                        if let Some(&community) = of_node.get(neighbour) {
                            *into.entry(community).or_insert(0.0) += weight;
                        }
                    }
                }

                // Total degree per community, this node removed from its
                // own, so a move is judged against the graph without it.
                let mut community_degree: BTreeMap<usize, f64> = BTreeMap::new();
                for (other, &community) in &of_node {
                    if *other == node {
                        continue;
                    }
                    *community_degree.entry(community).or_insert(0.0) +=
                        view.degree.get(other).copied().unwrap_or(0.0);
                }

                let gain_of = |community: usize| -> f64 {
                    let shared = into.get(&community).copied().unwrap_or(0.0);
                    let rest = community_degree.get(&community).copied().unwrap_or(0.0);
                    shared - RESOLUTION * rest * node_degree / view.total
                };

                let mut best = current;
                let mut best_gain = gain_of(current);
                for &candidate in into.keys() {
                    let gain = gain_of(candidate);
                    // A strictly better gain wins; an equal one only wins
                    // with a lower identifier, so a tie never depends on
                    // iteration order.
                    if gain > best_gain || (gain == best_gain && candidate < best) {
                        best = candidate;
                        best_gain = gain;
                    }
                }

                if best != current {
                    of_node.insert(node, best);
                    moved = true;
                }
            }

            if !moved {
                break;
            }
        }
    }

    // Renumber from zero in first-appearance order, so the identifiers
    // themselves are stable rather than an artefact of the starting
    // assignment.
    let mut renumbered: BTreeMap<usize, usize> = BTreeMap::new();
    let mut final_of_node: BTreeMap<NodeIndex, usize> = BTreeMap::new();
    for node in graph.node_indices() {
        let raw = of_node[&node];
        let next = renumbered.len();
        let community = *renumbered.entry(raw).or_insert(next);
        final_of_node.insert(node, community);
    }

    let modularity = modularity_of(&view, &final_of_node);
    Communities {
        of_node: final_of_node,
        modularity,
        passes,
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

    /// Two dense clusters joined by one edge: 0-1-2 and 3-4-5, with 2 -> 3.
    fn two_clusters() -> DiGraph<(), ()> {
        graph_from(&[(0, 1), (1, 2), (2, 0), (3, 4), (4, 5), (5, 3), (2, 3)], 6)
    }

    #[test]
    fn two_clusters_joined_by_one_edge_are_found_as_two_communities() {
        let found = detect(&two_clusters());
        let community = |n: usize| found.of_node[&NodeIndex::new(n)];

        assert_eq!(community(0), community(1));
        assert_eq!(community(1), community(2));
        assert_eq!(community(3), community(4));
        assert_eq!(community(4), community(5));
        assert_ne!(
            community(2),
            community(3),
            "the two clusters must not merge into one community"
        );
    }

    #[test]
    fn the_joining_edge_crosses_a_boundary_and_an_inside_edge_does_not() {
        let found = detect(&two_clusters());
        assert!(found.crosses(NodeIndex::new(2), NodeIndex::new(3)));
        assert!(!found.crosses(NodeIndex::new(0), NodeIndex::new(1)));
    }

    #[test]
    fn a_clustered_graph_scores_higher_modularity_than_a_ring() {
        let clustered = detect(&two_clusters());
        // A six-node ring has no community structure worth the name.
        let ring = detect(&graph_from(
            &[(0, 1), (1, 2), (2, 3), (3, 4), (4, 5), (5, 0)],
            6,
        ));
        assert!(
            clustered.modularity > ring.modularity,
            "clustered {} should beat ring {}",
            clustered.modularity,
            ring.modularity
        );
    }

    #[test]
    fn the_partition_is_identical_across_repeated_runs() {
        let graph = two_clusters();
        let first = detect(&graph);
        for _ in 0..10 {
            assert_eq!(
                detect(&graph),
                first,
                "Rule 32: Louvain is greedy, so a fixed visit order is what makes it reproducible"
            );
        }
    }

    #[test]
    fn an_empty_graph_yields_no_communities_and_no_nan() {
        let graph: DiGraph<(), ()> = DiGraph::new();
        let found = detect(&graph);
        assert!(found.of_node.is_empty());
        assert!(found.modularity.is_finite());
    }

    #[test]
    fn a_graph_with_no_edges_puts_every_node_in_its_own_community() {
        let graph = graph_from(&[], 4);
        let found = detect(&graph);
        assert!(found.modularity.is_finite(), "must not divide by zero");

        let mut communities: Vec<usize> = found.of_node.values().copied().collect();
        communities.sort_unstable();
        communities.dedup();
        assert_eq!(communities.len(), 4);
    }

    #[test]
    fn community_identifiers_are_renumbered_from_zero() {
        let found = detect(&two_clusters());
        let mut seen: Vec<usize> = found.of_node.values().copied().collect();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(
            seen,
            vec![0, 1],
            "identifiers must not leak the starting assignment"
        );
    }

    #[test]
    fn the_pass_count_stops_at_the_limit() {
        let found = detect(&two_clusters());
        assert!(found.passes <= MAX_PASSES);
        assert!(
            found.passes > 0,
            "a graph with edges runs at least one pass"
        );
    }
}
