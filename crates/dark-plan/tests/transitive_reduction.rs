//! Task unit `E6`'s "Done when": "An over-connected 12-node fixture
//! reduces to the correct minimal edge set."
//!
//! Builds a fixture with two distinct shapes in one graph: a six-way
//! sibling fan-out from one root (which should stay exactly as it is,
//! and also trips the out-degree flag), and a five-hop chain carrying
//! three edges that are already implied by the chain (which should all
//! be removed). `crate::wire::repair_wiring` is called directly, not
//! through `ChartPipeline`: see `dark_plan::wire`'s module documentation
//! for why the pipeline itself does not call it.

use dark_plan::chart::{ChartedEdge, ChartedTicket, TicketKind};
use dark_plan::wire::repair_wiring;

fn ticket(id: &str, ordinal: i64) -> ChartedTicket {
    ChartedTicket {
        id: id.to_owned(),
        name: id.to_owned(),
        question: format!("What resolves {id}?"),
        ticket_type: TicketKind::Task,
        hitl: false,
        ordinal,
        axis: vec![],
    }
}

fn edge(from: &str, to: &str) -> ChartedEdge {
    ChartedEdge {
        blocker: from.to_owned(),
        blocked: to.to_owned(),
    }
}

#[test]
fn an_over_connected_twelve_node_fixture_reduces_to_the_minimal_edge_set() {
    let tickets: Vec<ChartedTicket> = (1..=12)
        .map(|n| ticket(&format!("T{n:02}"), i64::from(n - 1)))
        .collect();
    assert_eq!(
        tickets.len(),
        12,
        "the fixture must hold exactly 12 tickets"
    );

    let edges = vec![
        // T01 fans out to six siblings; none of them block each other, so
        // every one of these six edges is necessary and none is removed.
        edge("T01", "T02"),
        edge("T01", "T03"),
        edge("T01", "T04"),
        edge("T01", "T05"),
        edge("T01", "T06"),
        edge("T01", "T07"),
        // A chain from T07 through T12.
        edge("T07", "T08"),
        edge("T08", "T09"),
        edge("T09", "T10"),
        edge("T10", "T11"),
        edge("T11", "T12"),
        // Redundant: each is already implied by the chain above.
        edge("T07", "T09"),
        edge("T07", "T10"),
        edge("T07", "T12"),
    ];
    assert_eq!(
        edges.len(),
        14,
        "the over-connected fixture starts with 14 edges"
    );

    let report = repair_wiring(&tickets, edges).expect("a cycle-free graph repairs cleanly");

    assert!(report.cycles_broken.is_empty(), "the fixture has no cycle");
    assert_eq!(report.transitive_edges_removed.len(), 3);
    for removed in &report.transitive_edges_removed {
        assert_eq!(removed.blocker, "T07");
        assert!(["T09", "T10", "T12"].contains(&removed.blocked.as_str()));
    }

    let mut kept: Vec<(String, String)> = report
        .edges
        .iter()
        .map(|edge| (edge.blocker.clone(), edge.blocked.clone()))
        .collect();
    kept.sort();

    let mut expected: Vec<(String, String)> = vec![
        ("T01".to_owned(), "T02".to_owned()),
        ("T01".to_owned(), "T03".to_owned()),
        ("T01".to_owned(), "T04".to_owned()),
        ("T01".to_owned(), "T05".to_owned()),
        ("T01".to_owned(), "T06".to_owned()),
        ("T01".to_owned(), "T07".to_owned()),
        ("T07".to_owned(), "T08".to_owned()),
        ("T08".to_owned(), "T09".to_owned()),
        ("T09".to_owned(), "T10".to_owned()),
        ("T10".to_owned(), "T11".to_owned()),
        ("T11".to_owned(), "T12".to_owned()),
    ];
    expected.sort();
    assert_eq!(
        kept, expected,
        "the minimal edge set is exactly the fan-out plus the chain"
    );

    assert_eq!(
        report.high_out_degree,
        vec!["T01".to_owned()],
        "T01 blocks six others in the final graph, past the five-edge cap"
    );
}
