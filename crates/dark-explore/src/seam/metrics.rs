//! Per-node coupling and abstractness metrics.
//!
//! These are the classical Martin metrics, computed over whichever graph the
//! caller passes. See Do step 1 of task unit `F3`.

use petgraph::Direction;
use petgraph::graph::{DiGraph, NodeIndex};

/// The metrics for one node.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NodeMetrics {
    /// Afferent coupling: how many nodes depend on this one.
    pub ca: u32,
    /// Efferent coupling: how many nodes this one depends on.
    pub ce: u32,
    /// Instability, `Ce / (Ca + Ce)`, in the range 0 to 1.
    ///
    /// A node nothing depends on and that depends on nothing scores 0
    /// rather than dividing by zero.
    pub instability: f64,
    /// Abstractness: interface-like definitions over all definitions.
    ///
    /// A node with no definitions at all scores 0.
    pub abstractness: f64,
    /// Distance from the main sequence, `|A + I - 1|`.
    ///
    /// Zero is the ideal: a node is either abstract and depended upon, or
    /// concrete and depending on others. One is the worst.
    pub distance: f64,
}

/// Computes [`NodeMetrics`] for one node of any directed graph.
///
/// `total_defs` and `interface_like_defs` come from the node's own weight;
/// this function takes them as numbers so it serves the F-graph, the
/// S-graph, and the M-graph without knowing any of their weight types.
#[must_use]
pub fn for_node<N, E>(
    graph: &DiGraph<N, E>,
    node: NodeIndex,
    total_defs: u32,
    interface_like_defs: u32,
) -> NodeMetrics {
    let ca = u32::try_from(graph.neighbors_directed(node, Direction::Incoming).count())
        .unwrap_or(u32::MAX);
    let ce = u32::try_from(graph.neighbors_directed(node, Direction::Outgoing).count())
        .unwrap_or(u32::MAX);

    let coupling = f64::from(ca) + f64::from(ce);
    let instability = if coupling == 0.0 {
        0.0
    } else {
        f64::from(ce) / coupling
    };

    let abstractness = if total_defs == 0 {
        0.0
    } else {
        f64::from(interface_like_defs) / f64::from(total_defs)
    };

    NodeMetrics {
        ca,
        ce,
        instability,
        abstractness,
        distance: (abstractness + instability - 1.0).abs(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds `a -> b -> c`, so `b` has one in and one out.
    fn chain() -> (DiGraph<(), ()>, [NodeIndex; 3]) {
        let mut graph = DiGraph::new();
        let a = graph.add_node(());
        let b = graph.add_node(());
        let c = graph.add_node(());
        graph.add_edge(a, b, ());
        graph.add_edge(b, c, ());
        (graph, [a, b, c])
    }

    #[test]
    fn a_node_in_the_middle_of_a_chain_is_half_unstable() {
        let (graph, [_, b, _]) = chain();
        let metrics = for_node(&graph, b, 0, 0);
        assert_eq!(metrics.ca, 1);
        assert_eq!(metrics.ce, 1);
        assert!((metrics.instability - 0.5).abs() < f64::EPSILON);
    }

    #[test]
    fn a_source_is_fully_unstable_and_a_sink_is_fully_stable() {
        let (graph, [a, _, c]) = chain();
        assert!((for_node(&graph, a, 0, 0).instability - 1.0).abs() < f64::EPSILON);
        assert!(for_node(&graph, c, 0, 0).instability.abs() < f64::EPSILON);
    }

    #[test]
    fn an_isolated_node_scores_zero_rather_than_dividing_by_zero() {
        let mut graph: DiGraph<(), ()> = DiGraph::new();
        let lone = graph.add_node(());
        let metrics = for_node(&graph, lone, 0, 0);

        assert!(metrics.instability.is_finite(), "must not be NaN");
        assert!(metrics.instability.abs() < f64::EPSILON);
    }

    #[test]
    fn abstractness_is_the_interface_like_share() {
        let mut graph: DiGraph<(), ()> = DiGraph::new();
        let node = graph.add_node(());
        let metrics = for_node(&graph, node, 4, 1);
        assert!((metrics.abstractness - 0.25).abs() < f64::EPSILON);
    }

    #[test]
    fn a_node_with_no_definitions_is_not_abstract() {
        let mut graph: DiGraph<(), ()> = DiGraph::new();
        let node = graph.add_node(());
        let metrics = for_node(&graph, node, 0, 0);

        assert!(metrics.abstractness.is_finite(), "must not be NaN");
        assert!(metrics.abstractness.abs() < f64::EPSILON);
    }

    #[test]
    fn an_abstract_sink_and_a_concrete_source_both_sit_on_the_main_sequence() {
        let (graph, [a, _, c]) = chain();

        // Fully abstract and fully stable: A = 1, I = 0.
        let abstract_sink = for_node(&graph, c, 2, 2);
        assert!(abstract_sink.distance.abs() < f64::EPSILON);

        // Fully concrete and fully unstable: A = 0, I = 1.
        let concrete_source = for_node(&graph, a, 2, 0);
        assert!(concrete_source.distance.abs() < f64::EPSILON);
    }

    #[test]
    fn an_abstract_source_is_as_far_from_the_main_sequence_as_it_gets() {
        let (graph, [a, _, _]) = chain();
        // A = 1 and I = 1: abstract, yet depending on everything.
        let metrics = for_node(&graph, a, 2, 2);
        assert!((metrics.distance - 1.0).abs() < f64::EPSILON);
    }
}
