//! Bridges, articulation points, and communities.
//!
//! A bridge is a hard seam: remove that one edge and the graph falls into
//! two pieces, so a change on one side cannot reach the other except
//! through it. See Do steps 3 and 4 of task unit `F3`.
//!
//! # Determinism
//!
//! Both algorithms here depend on visit order, and Rule 32 requires the
//! same bytes out for the same commit. Every traversal therefore visits
//! nodes in node-index order, which `graph::build` already fixed to sorted
//! path order using F1's byte comparator. Nothing here reads a clock, a
//! hash map's iteration order, or a random number source.

use std::collections::{BTreeMap, BTreeSet};

use petgraph::graph::{DiGraph, EdgeIndex, NodeIndex};
use petgraph::visit::EdgeRef;

/// One edge whose removal disconnects the graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Bridge {
    /// The edge itself.
    pub edge: EdgeIndex,
    /// The edge's two endpoints, in the direction the edge runs.
    pub from: NodeIndex,
    /// The node the edge points at.
    pub to: NodeIndex,
}

/// What one structural pass found.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Structure {
    /// Every bridge, in node-index order.
    pub bridges: Vec<Bridge>,
    /// Every articulation point, in node-index order. Removing one splits
    /// the graph, the way removing a bridge does, but at a node.
    pub articulation_points: Vec<NodeIndex>,
}

/// The undirected view that Tarjan's algorithm needs.
///
/// A bridge is an undirected idea: it asks whether removing the edge
/// disconnects the graph, and direction has nothing to do with that. The
/// adjacency keeps the edge index so a found bridge can name the original
/// directed edge.
fn undirected_adjacency<N, E>(
    graph: &DiGraph<N, E>,
) -> BTreeMap<NodeIndex, Vec<(NodeIndex, EdgeIndex)>> {
    let mut adjacency: BTreeMap<NodeIndex, Vec<(NodeIndex, EdgeIndex)>> = BTreeMap::new();
    for node in graph.node_indices() {
        adjacency.entry(node).or_default();
    }
    for edge in graph.edge_references() {
        let (source, target) = (edge.source(), edge.target());
        if source == target {
            // A self-loop can never be a bridge: removing it disconnects
            // nothing.
            continue;
        }
        adjacency
            .entry(source)
            .or_default()
            .push((target, edge.id()));
        adjacency
            .entry(target)
            .or_default()
            .push((source, edge.id()));
    }
    for neighbours in adjacency.values_mut() {
        neighbours.sort_unstable();
    }
    adjacency
}

/// State that the depth-first search carries between frames.
struct Search {
    discovery: BTreeMap<NodeIndex, u32>,
    low: BTreeMap<NodeIndex, u32>,
    counter: u32,
    bridges: Vec<Bridge>,
    articulation_points: BTreeSet<NodeIndex>,
}

/// Finds every bridge and articulation point.
///
/// Tarjan's algorithm, written iteratively rather than recursively: a deep
/// repository produces a deep spanning tree, and a recursive walk would
/// overflow the stack on one.
#[must_use]
pub fn find<N, E>(graph: &DiGraph<N, E>) -> Structure {
    let adjacency = undirected_adjacency(graph);
    let mut search = Search {
        discovery: BTreeMap::new(),
        low: BTreeMap::new(),
        counter: 0,
        bridges: Vec::new(),
        articulation_points: BTreeSet::new(),
    };

    for root in graph.node_indices() {
        if search.discovery.contains_key(&root) {
            continue;
        }
        walk_from(&adjacency, root, &mut search);
    }

    search.bridges.sort_unstable();
    Structure {
        bridges: search.bridges,
        articulation_points: search.articulation_points.into_iter().collect(),
    }
}

/// One frame of the iterative depth-first search.
struct Frame {
    node: NodeIndex,
    parent_edge: Option<EdgeIndex>,
    /// How far through this node's neighbour list the search has got.
    next: usize,
    /// How many children of this node the search rooted a subtree at. The
    /// root is an articulation point only when it has more than one.
    children: u32,
}

/// Walks one connected component from `root`.
fn walk_from(
    adjacency: &BTreeMap<NodeIndex, Vec<(NodeIndex, EdgeIndex)>>,
    root: NodeIndex,
    search: &mut Search,
) {
    let empty: Vec<(NodeIndex, EdgeIndex)> = Vec::new();
    search.counter += 1;
    search.discovery.insert(root, search.counter);
    search.low.insert(root, search.counter);

    let mut stack = vec![Frame {
        node: root,
        parent_edge: None,
        next: 0,
        children: 0,
    }];

    while let Some(frame) = stack.last_mut() {
        let node = frame.node;
        let neighbours = adjacency.get(&node).unwrap_or(&empty);

        if frame.next < neighbours.len() {
            let (neighbour, via) = neighbours[frame.next];
            frame.next += 1;

            // Never walk back along the edge this frame arrived on. A
            // parallel edge between the same pair is a different edge
            // index, and it correctly stops either from being a bridge.
            if Some(via) == frame.parent_edge {
                continue;
            }

            if let Some(&seen) = search.discovery.get(&neighbour) {
                let low = search.low.entry(node).or_insert(seen);
                *low = (*low).min(seen);
            } else {
                frame.children += 1;
                search.counter += 1;
                search.discovery.insert(neighbour, search.counter);
                search.low.insert(neighbour, search.counter);
                stack.push(Frame {
                    node: neighbour,
                    parent_edge: Some(via),
                    next: 0,
                    children: 0,
                });
            }
            continue;
        }

        // Every neighbour is explored: fold this node into its parent.
        let finished = stack.pop().expect("the frame was just borrowed");
        let node_low = search.low.get(&finished.node).copied().unwrap_or(0);

        if let Some(parent_frame) = stack.last() {
            let parent = parent_frame.node;
            let parent_discovery = search.discovery.get(&parent).copied().unwrap_or(0);

            let parent_low = search.low.entry(parent).or_insert(node_low);
            *parent_low = (*parent_low).min(node_low);

            if node_low > parent_discovery {
                if let Some(edge) = finished.parent_edge {
                    search.bridges.push(Bridge {
                        edge,
                        from: parent,
                        to: finished.node,
                    });
                }
            }

            // A non-root parent is an articulation point when a child
            // cannot reach above it.
            if stack.len() > 1 && node_low >= parent_discovery {
                search.articulation_points.insert(parent);
            }
        } else if finished.children > 1 {
            // `finished` is this component's root. A root is an
            // articulation point only when it roots more than one subtree
            // of the search *tree*. Counting its neighbours instead would
            // be wrong: in a triangle the root has two neighbours, but the
            // second is reached through the first, so removing the root
            // splits nothing.
            search.articulation_points.insert(finished.node);
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
    fn every_edge_of_a_chain_is_a_bridge() {
        let graph = graph_from(&[(0, 1), (1, 2), (2, 3)], 4);
        let found = find(&graph);
        assert_eq!(found.bridges.len(), 3);
    }

    #[test]
    fn no_edge_of_a_cycle_is_a_bridge() {
        let graph = graph_from(&[(0, 1), (1, 2), (2, 0)], 3);
        let found = find(&graph);
        assert!(
            found.bridges.is_empty(),
            "a cycle has no bridge: every node stays reachable"
        );
        assert!(found.articulation_points.is_empty());
    }

    #[test]
    fn the_edge_joining_two_cycles_is_the_only_bridge() {
        // Two triangles, 0-1-2 and 3-4-5, joined by 2 -> 3.
        let graph = graph_from(&[(0, 1), (1, 2), (2, 0), (3, 4), (4, 5), (5, 3), (2, 3)], 6);
        let found = find(&graph);

        assert_eq!(found.bridges.len(), 1, "only the joining edge is a bridge");
        let bridge = found.bridges[0];
        let endpoints = (bridge.from.index(), bridge.to.index());
        assert!(
            endpoints == (2, 3) || endpoints == (3, 2),
            "the bridge must be the joining edge, got {endpoints:?}"
        );
    }

    #[test]
    fn the_node_joining_two_cycles_is_an_articulation_point() {
        // Two triangles sharing node 0.
        let graph = graph_from(&[(0, 1), (1, 2), (2, 0), (0, 3), (3, 4), (4, 0)], 5);
        let found = find(&graph);

        assert!(found.bridges.is_empty(), "no single edge splits this");
        assert_eq!(
            found.articulation_points,
            vec![NodeIndex::new(0)],
            "removing the shared node splits the two triangles"
        );
    }

    #[test]
    fn a_self_loop_is_never_a_bridge() {
        let graph = graph_from(&[(0, 0), (0, 1)], 2);
        let found = find(&graph);
        assert_eq!(found.bridges.len(), 1, "only the real edge is a bridge");
        assert_eq!(found.bridges[0].to.index(), 1);
    }

    #[test]
    fn a_parallel_edge_stops_either_from_being_a_bridge() {
        // Two separate edges between the same pair: removing one leaves the
        // other, so neither disconnects anything.
        let graph = graph_from(&[(0, 1), (0, 1)], 2);
        let found = find(&graph);
        assert!(found.bridges.is_empty());
    }

    #[test]
    fn a_disconnected_graph_reports_each_components_bridges() {
        // Two chains that never meet.
        let graph = graph_from(&[(0, 1), (2, 3)], 4);
        let found = find(&graph);
        assert_eq!(found.bridges.len(), 2);
    }

    #[test]
    fn the_result_is_identical_across_repeated_runs() {
        let graph = graph_from(&[(0, 1), (1, 2), (2, 0), (2, 3), (3, 4), (4, 5), (5, 3)], 6);
        let first = find(&graph);
        for _ in 0..5 {
            assert_eq!(
                find(&graph),
                first,
                "Rule 32: the same input, the same bytes"
            );
        }
    }

    #[test]
    fn a_deep_chain_does_not_overflow_the_stack() {
        // Tarjan's algorithm is written iteratively for exactly this: a
        // recursive walk overflows well before this depth.
        let edges: Vec<(usize, usize)> = (0..20_000).map(|i| (i, i + 1)).collect();
        let graph = graph_from(&edges, 20_001);
        assert_eq!(find(&graph).bridges.len(), 20_000);
    }
}
