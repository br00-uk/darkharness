//! Stage 7 of the charting pipeline: ask each ticket what blocks it, then
//! repair the graph a weak model's answers produce. Task unit `E6`.
//!
//! [`DefaultWirer`] implements [`Wirer`](crate::chart::stages::Wirer) (task
//! unit `E1`, `crate::chart::stages`): task unit `E6`, Do step 1's one
//! question per ticket, asked once per
//! [`Wirer::wire`](crate::chart::stages::Wirer::wire) call.
//!
//! **The Do step 2 repairs have no seam to run inside the pipeline.** Do
//! step 2 — break cycles, reduce transitively, cap out-degree, check the
//! frontier — is graph-wide: it needs every ticket's answer at once.
//! [`Wirer::wire`] is called once per ticket, and
//! `ChartPipeline::run` (task unit `E1`, `crate::chart::pipeline`, which
//! this task unit must not edit) resolves each returned
//! [`WireAnswer`](crate::chart::stages::WireAnswer) into a
//! [`ChartedEdge`] through its own private `resolve_edges` the moment
//! [`Wirer::wire`] returns — a plain name-to-identifier lookup, with no
//! repair pass and no hook back into one. A stateful [`Wirer`] cannot fix
//! this either: even if it waited for its own call count to reach the
//! ticket total before repairing, `ChartPipeline::stage_wire` has already
//! collected every earlier ticket's raw [`WireAnswer`] into a fixed
//! `Vec` by then, and never asks again.
//!
//! So this module implements Do step 2 as its own public, standalone
//! functions — [`repair_wiring`] and the pieces it calls — operating on a
//! finished `(tickets, edges)` pair rather than on one
//! [`Wirer::wire`] call. This satisfies task unit `E6`'s own Verify
//! commands, which test a fixture ticket-and-edge set directly, but it
//! means `ChartPipeline::chart` and `ChartPipeline::resume`, as currently
//! written, hand back an **unrepaired** graph: a caller that wants task
//! unit `E6`'s guarantees — no cycle, no transitively implied edge, a
//! non-empty frontier — must call [`repair_wiring`] on
//! `ChartOutput::edges` itself. Flagged here, and in the task report,
//! rather than solved by adding a second `Wirer`-shaped trait beside the
//! one task unit `E1` already defined.
//!
//! [`ChartedEdge`]: crate::chart::ticket::ChartedEdge

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;

use dark_contract::{Engine, ErrCode, Error, Grammar, Message, Result, Role, RoleClass};
use serde_json::{Value, json};

use crate::chart::sampling::{BoxFuture, MicroSampling, build_request, run_generation};
use crate::chart::stages::{WireAnswer, Wirer};
use crate::chart::ticket::{ChartedEdge, ChartedTicket};

/// How many other tickets one ticket may block before task unit `E6`, Do
/// step 2.3, flags it.
///
/// "Flag a ticket that blocks more than five others. This usually
/// indicates a parse error."
const OUT_DEGREE_CAP: usize = 5;

/// Builds the fresh stage-7 prompt for one ticket.
///
/// Matches task unit `E6`, Do step 1's example prompt, with `other_names`
/// filling the bullet list.
fn wire_prompt(question: &str, other_names: &[String]) -> Vec<Message> {
    let mut body = format!(
        "Ticket: {question:?}\n\nWhich of these must be answered BEFORE this question can be \
         resolved?\n"
    );
    for name in other_names {
        let _ = writeln!(body, "  \u{b7} {name}");
    }
    body.push_str(
        "\nAnswer with a JSON array holding the names above that must resolve first, in any \
         order, or an empty array when none must. Write nothing else.",
    );

    vec![
        Message::text(
            Role::System,
            "You wire one ticket's blocking edges at a time for a decision map. See only this \
             one ticket and the names of every other ticket. You have no memory of any other \
             ticket's own wiring.",
        ),
        Message::text(Role::User, body),
    ]
}

/// The JSON schema stage 7's grammar constrains the response to: an array
/// drawn only from `other_names`, the exact set task unit `E6`'s example
/// prompt offers.
fn names_schema(other_names: &[String]) -> Value {
    json!({
        "type": "array",
        "items": { "type": "string", "enum": other_names },
        "uniqueItems": true
    })
}

/// Keeps only the names in `names` that appear in `other_names`, in the
/// case [`ChartedTicket::name`] uses, deduplicated and in first-seen
/// order.
///
/// A name the model invented — one that matches nothing it was offered —
/// is dropped rather than turned into a blocker on a ticket that does not
/// exist. This is noise filtering, not a silent failure: an empty result
/// is exactly what "NONE" also produces, and
/// [`repair_wiring`]'s frontier check still catches a wiring pass that
/// went wrong in a way that matters (every ticket blocked, nothing
/// takeable).
fn filter_known_names(names: Vec<String>, other_names: &[String]) -> Vec<String> {
    let mut kept = Vec::new();
    for name in names {
        let trimmed = name.trim();
        if let Some(matched) = other_names.iter().find(|known| known.as_str() == trimmed)
            && !kept.contains(matched)
        {
            kept.push(matched.clone());
        }
    }
    kept
}

/// Parses one stage-7 response into the blocker names it names.
///
/// Tries the JSON array [`names_schema`] asks for first. When the text is
/// not valid JSON — a real weak model does not always follow a grammar
/// that was merely requested — falls back to reading it the way task unit
/// `E6`'s own example prompt is phrased: `"NONE"`, or a list of names
/// separated by newlines, commas, or a bullet marker.
fn parse_wire_answer(text: &str, other_names: &[String]) -> Vec<String> {
    let trimmed = text.trim();

    if let Ok(names) = serde_json::from_str::<Vec<String>>(trimmed) {
        return filter_known_names(names, other_names);
    }

    if trimmed.eq_ignore_ascii_case("none") {
        return Vec::new();
    }

    let candidates: Vec<String> = trimmed
        .split(['\n', ',', '\u{b7}', '-', '*'])
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(ToOwned::to_owned)
        .collect();
    filter_known_names(candidates, other_names)
}

/// The build specification's per-ticket wiring question.
///
/// See the module documentation for why the graph-wide repairs task unit
/// `E6` also owns live in [`repair_wiring`], a separate, standalone step,
/// rather than in this trait implementation.
#[derive(Debug, Default, Clone, Copy)]
pub struct DefaultWirer;

impl Wirer for DefaultWirer {
    fn wire<'a>(
        &'a self,
        engine: &'a dyn Engine,
        class: RoleClass,
        sampling: MicroSampling,
        ticket: &'a ChartedTicket,
        other_names: &'a [String],
    ) -> BoxFuture<'a, Result<WireAnswer>> {
        Box::pin(async move {
            // A ticket with nothing else on the map to name has nothing to
            // ask about: skip the call entirely rather than spend a
            // generation to be told "NONE".
            if other_names.is_empty() {
                return Ok(WireAnswer::default());
            }

            let messages = wire_prompt(&ticket.question, other_names);
            let mut request = build_request(class, messages, sampling);
            if sampling.grammar {
                request.grammar = Some(Grammar::JsonSchema(names_schema(other_names)));
            }

            let generation = run_generation(engine, request).await?;
            let blocked_by = parse_wire_answer(&generation.text, other_names);
            Ok(WireAnswer { blocked_by })
        })
    }
}

/// One edge [`break_cycles`] dropped to remove a cycle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CycleBreak {
    /// The ticket identifier the dropped edge no longer blocks on.
    pub blocker: String,
    /// The ticket identifier the dropped edge no longer blocks.
    pub blocked: String,
    /// The full cycle, as ticket identifiers, that this break removed.
    pub cycle: Vec<String>,
}

/// What [`repair_wiring`] did to one raw edge set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WireRepairReport {
    /// The final, repaired edge set.
    pub edges: Vec<ChartedEdge>,
    /// Every cycle break [`break_cycles`] made, in the order it made them.
    pub cycles_broken: Vec<CycleBreak>,
    /// Every edge [`transitive_reduce`] removed as implied by another path.
    pub transitive_edges_removed: Vec<ChartedEdge>,
    /// The identifiers of tickets that block more than [`OUT_DEGREE_CAP`]
    /// others in the final edge set. Purely a flag: task unit `E6`, Do
    /// step 2.3, does not ask for an edge to be removed over this, only
    /// reported.
    pub high_out_degree: Vec<String>,
}

/// Removes an edge that blocks a ticket on itself, and collapses an exact
/// duplicate blocker-blocked pair to one edge.
///
/// A self-loop or a duplicate is not a cycle in the sense task unit `E6`
/// means (nothing "blocks the frontier permanently" over it), but leaving
/// either in would make [`transitive_reduce`]'s reachability search see a
/// path that is not really a second, independent route.
fn dedupe_edges(edges: Vec<ChartedEdge>) -> Vec<ChartedEdge> {
    let mut seen: HashSet<(String, String)> = HashSet::new();
    edges
        .into_iter()
        .filter(|edge| edge.blocker != edge.blocked)
        .filter(|edge| seen.insert((edge.blocker.clone(), edge.blocked.clone())))
        .collect()
}

/// Builds a blocker-to-blocked adjacency list from `edges`.
fn adjacency(edges: &[ChartedEdge]) -> HashMap<&str, Vec<&str>> {
    let mut graph: HashMap<&str, Vec<&str>> = HashMap::new();
    for edge in edges {
        graph
            .entry(edge.blocker.as_str())
            .or_default()
            .push(edge.blocked.as_str());
    }
    graph
}

/// Finds one cycle in the blocking graph, when one exists.
///
/// Depth-first, visiting tickets in `tickets`'s own order so the result is
/// deterministic for a given input. Returns the cycle as ticket
/// identifiers, in path order (the edge from the last entry back to the
/// first is the one that closes the cycle).
#[must_use]
pub fn detect_cycle(tickets: &[ChartedTicket], edges: &[ChartedEdge]) -> Option<Vec<String>> {
    let graph = adjacency(edges);
    let mut visited: HashSet<&str> = HashSet::new();

    for ticket in tickets {
        if !visited.contains(ticket.id.as_str())
            && let Some(cycle) = dfs_for_cycle(ticket.id.as_str(), &graph, &mut visited)
        {
            return Some(cycle);
        }
    }
    None
}

/// The recursive half of [`detect_cycle`].
fn dfs_for_cycle<'a>(
    start: &'a str,
    graph: &HashMap<&'a str, Vec<&'a str>>,
    visited: &mut HashSet<&'a str>,
) -> Option<Vec<String>> {
    let mut stack: Vec<&str> = vec![start];
    let mut on_stack: HashSet<&str> = HashSet::from([start]);
    visited.insert(start);

    walk(start, graph, visited, &mut stack, &mut on_stack)
}

/// Depth-first walk that reports the first back-edge it finds, as the
/// cycle it closes.
fn walk<'a>(
    node: &'a str,
    graph: &HashMap<&'a str, Vec<&'a str>>,
    visited: &mut HashSet<&'a str>,
    stack: &mut Vec<&'a str>,
    on_stack: &mut HashSet<&'a str>,
) -> Option<Vec<String>> {
    let Some(children) = graph.get(node) else {
        stack.pop();
        on_stack.remove(node);
        return None;
    };

    for &child in children {
        if on_stack.contains(child) {
            let start_index = stack.iter().position(|&n| n == child).unwrap_or(0);
            return Some(stack[start_index..].iter().map(|&n| n.to_owned()).collect());
        }
        if !visited.contains(child) {
            visited.insert(child);
            stack.push(child);
            on_stack.insert(child);
            if let Some(cycle) = walk(child, graph, visited, stack, on_stack) {
                return Some(cycle);
            }
        }
    }

    stack.pop();
    on_stack.remove(node);
    None
}

/// Returns `ticket_id`'s ordinal, or `i64::MIN` when no ticket carries that
/// identifier (defensive: [`break_cycles`] only ever asks about
/// identifiers [`detect_cycle`] found on the edges it was given, which
/// come from `tickets` in the first place).
fn ordinal_of(tickets: &[ChartedTicket], ticket_id: &str) -> i64 {
    tickets
        .iter()
        .find(|ticket| ticket.id == ticket_id)
        .map_or(i64::MIN, |ticket| ticket.ordinal)
}

/// Breaks every cycle in `edges`, one edge at a time.
///
/// Task unit `E6`, Do step 2.1: "Drop the edge whose blocker has the
/// higher ordinal. Report every break." When a cycle has more than one
/// edge tied on the highest blocker ordinal, this drops the first one the
/// cycle path names, which keeps the result deterministic for the same
/// input without needing a second tie-break rule the build specification
/// never gives.
///
/// # Errors
///
/// Returns [`ErrCode::MapCycle`] on the (unreachable in ordinary use)
/// event that more break attempts than `edges` holds still leave a cycle —
/// a defensive bound, since each successful break strictly shrinks a
/// finite edge set.
fn break_cycles(
    tickets: &[ChartedTicket],
    mut edges: Vec<ChartedEdge>,
) -> Result<(Vec<ChartedEdge>, Vec<CycleBreak>)> {
    let mut breaks = Vec::new();
    let attempt_budget = edges.len() + 1;

    for _ in 0..=attempt_budget {
        let Some(cycle) = detect_cycle(tickets, &edges) else {
            return Ok((edges, breaks));
        };

        let mut drop_index = None;
        let mut drop_ordinal = i64::MIN;
        for position in 0..cycle.len() {
            let from_id = &cycle[position];
            let to_id = &cycle[(position + 1) % cycle.len()];
            let Some(edge_index) = edges
                .iter()
                .position(|edge| &edge.blocker == from_id && &edge.blocked == to_id)
            else {
                continue;
            };
            let ordinal = ordinal_of(tickets, from_id);
            if drop_index.is_none() || ordinal > drop_ordinal {
                drop_index = Some(edge_index);
                drop_ordinal = ordinal;
            }
        }

        let Some(drop_index) = drop_index else {
            // detect_cycle found a cycle, but none of its consecutive
            // pairs matched a real edge: a defensive dead end, not a
            // reachable state for edges this module itself built.
            break;
        };
        let dropped = edges.remove(drop_index);
        breaks.push(CycleBreak {
            blocker: dropped.blocker,
            blocked: dropped.blocked,
            cycle,
        });
    }

    Err(Error::new(
        ErrCode::MapCycle,
        "could not break every cycle within the edge budget",
    )
    .with_remedy("Remove one edge on the reported path."))
}

/// Returns whether `to` is reachable from `from` using every edge in
/// `edges` except the one at `skip_index`.
fn reachable_without(edges: &[ChartedEdge], skip_index: usize, from: &str, to: &str) -> bool {
    let mut visited: HashSet<&str> = HashSet::from([from]);
    let mut stack: Vec<&str> = vec![from];

    while let Some(node) = stack.pop() {
        for (index, edge) in edges.iter().enumerate() {
            if index == skip_index || edge.blocker.as_str() != node {
                continue;
            }
            let next = edge.blocked.as_str();
            if next == to {
                return true;
            }
            if visited.insert(next) {
                stack.push(next);
            }
        }
    }
    false
}

/// Removes every edge implied by another path.
///
/// Task unit `E6`, Do step 2.2: "When A blocks B and B blocks C, remove
/// any A-to-C edge. A model asserts implied edges. This repair removes
/// them." Runs to a fixed point: removing one implied edge can reveal
/// another edge is now only reachable through the one just removed being
/// itself part of the remaining chain, so this keeps scanning until one
/// full pass finds nothing left to remove. On the cycle-free graph
/// [`break_cycles`] hands it, the result does not depend on the order
/// edges are removed in — a DAG's transitive reduction is unique.
fn transitive_reduce(edges: Vec<ChartedEdge>) -> (Vec<ChartedEdge>, Vec<ChartedEdge>) {
    let mut kept = edges;
    let mut removed = Vec::new();

    loop {
        let redundant = kept
            .iter()
            .enumerate()
            .find(|(index, edge)| reachable_without(&kept, *index, &edge.blocker, &edge.blocked))
            .map(|(index, _)| index);

        match redundant {
            Some(index) => removed.push(kept.remove(index)),
            None => break,
        }
    }

    (kept, removed)
}

/// Flags every ticket that blocks more than [`OUT_DEGREE_CAP`] others in
/// `edges`, sorted by ticket identifier for a stable report.
///
/// Task unit `E6`, Do step 2.3: "Cap out-degree. Flag a ticket that blocks
/// more than five others. This usually indicates a parse error." Purely
/// informational: no edge is removed over this.
fn cap_out_degree(tickets: &[ChartedTicket], edges: &[ChartedEdge]) -> Vec<String> {
    let mut out_degree: HashMap<&str, usize> = HashMap::new();
    for edge in edges {
        *out_degree.entry(edge.blocker.as_str()).or_insert(0) += 1;
    }

    let mut flagged: Vec<String> = tickets
        .iter()
        .filter(|ticket| {
            out_degree
                .get(ticket.id.as_str())
                .is_some_and(|&count| count > OUT_DEGREE_CAP)
        })
        .map(|ticket| ticket.id.clone())
        .collect();
    flagged.sort();
    flagged
}

/// Checks that at least one ticket is takeable now.
///
/// # Errors
///
/// Returns [`ErrCode::MapEmptyFrontier`] when every ticket in `tickets`
/// has an incoming edge in `edges` (or `tickets` is empty). Task unit
/// `E6`, Do step 2.4: "When the wired graph has an empty frontier, the
/// wiring is wrong. Every map must start with a takeable ticket. Fail the
/// stage. Retry it." [`repair_wiring`] runs this last, after cycle
/// breaking and transitive reduction, so it checks the graph a caller
/// would actually see.
pub fn check_frontier(tickets: &[ChartedTicket], edges: &[ChartedEdge]) -> Result<()> {
    let blocked: HashSet<&str> = edges.iter().map(|edge| edge.blocked.as_str()).collect();
    let takeable = tickets
        .iter()
        .any(|ticket| !blocked.contains(ticket.id.as_str()));

    if takeable {
        Ok(())
    } else {
        Err(Error::new(
            ErrCode::MapEmptyFrontier,
            "every ticket is blocked; the wired graph has no takeable ticket",
        ))
    }
}

/// Runs every Do step 2 repair over one raw wired edge set, in the order
/// task unit `E6` lists them: break cycles, reduce transitively, cap
/// out-degree, check the frontier.
///
/// See the module documentation for why a caller must call this itself —
/// `ChartPipeline::chart` and `ChartPipeline::resume` do not.
///
/// # Errors
///
/// Returns [`ErrCode::MapCycle`] when [`break_cycles`] cannot finish (see
/// its own documentation), or [`ErrCode::MapEmptyFrontier`] when the
/// repaired graph still leaves every ticket blocked.
pub fn repair_wiring(
    tickets: &[ChartedTicket],
    edges: Vec<ChartedEdge>,
) -> Result<WireRepairReport> {
    let deduped = dedupe_edges(edges);
    let (acyclic, cycles_broken) = break_cycles(tickets, deduped)?;
    let (reduced, transitive_edges_removed) = transitive_reduce(acyclic);
    let high_out_degree = cap_out_degree(tickets, &reduced);
    check_frontier(tickets, &reduced)?;

    Ok(WireRepairReport {
        edges: reduced,
        cycles_broken,
        transitive_edges_removed,
        high_out_degree,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chart::ticket::TicketKind;
    use dark_engine_fake::script::Turn;
    use dark_engine_fake::{FakeEngine, Script};

    fn ticket(id: &str, ordinal: i64) -> ChartedTicket {
        ChartedTicket {
            id: id.to_owned(),
            name: format!("ticket {id}"),
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

    // -- DefaultWirer --------------------------------------------------

    #[tokio::test]
    async fn a_ticket_with_no_other_names_is_never_asked() {
        let engine = FakeEngine::new(Script::default());
        let wirer = DefaultWirer;
        let solo = ticket("A", 0);

        let answer = wirer
            .wire(
                &engine,
                RoleClass::Architect,
                MicroSampling::classify(),
                &solo,
                &[],
            )
            .await
            .expect("nothing to ask about never fails");

        assert!(answer.blocked_by.is_empty());
        assert_eq!(engine.turns_played(), 0);
    }

    #[tokio::test]
    async fn a_json_array_response_is_read_and_filtered_to_known_names() {
        let engine = FakeEngine::new(Script {
            turns: vec![Turn {
                text: r#"["b", "made up name"]"#.to_owned(),
                ..Default::default()
            }],
            ..Default::default()
        });
        let wirer = DefaultWirer;
        let ticket_a = ticket("A", 0);
        let other_names = vec!["b".to_owned(), "c".to_owned()];

        let answer = wirer
            .wire(
                &engine,
                RoleClass::Architect,
                MicroSampling::classify(),
                &ticket_a,
                &other_names,
            )
            .await
            .expect("wiring succeeds");

        assert_eq!(answer.blocked_by, vec!["b".to_owned()]);
    }

    #[tokio::test]
    async fn a_plain_none_answer_is_read_as_no_blockers() {
        let engine = FakeEngine::new(Script {
            turns: vec![Turn {
                text: "NONE".to_owned(),
                ..Default::default()
            }],
            ..Default::default()
        });
        let wirer = DefaultWirer;
        let ticket_a = ticket("A", 0);
        let other_names = vec!["b".to_owned()];

        let answer = wirer
            .wire(
                &engine,
                RoleClass::Architect,
                MicroSampling::classify(),
                &ticket_a,
                &other_names,
            )
            .await
            .expect("wiring succeeds");

        assert!(answer.blocked_by.is_empty());
    }

    #[tokio::test]
    async fn a_bulleted_plain_text_answer_still_names_the_blockers() {
        let engine = FakeEngine::new(Script {
            turns: vec![Turn {
                text: "  \u{b7} b\n  \u{b7} c".to_owned(),
                ..Default::default()
            }],
            ..Default::default()
        });
        let wirer = DefaultWirer;
        let ticket_a = ticket("A", 0);
        let other_names = vec!["b".to_owned(), "c".to_owned(), "d".to_owned()];

        let answer = wirer
            .wire(
                &engine,
                RoleClass::Architect,
                MicroSampling::classify(),
                &ticket_a,
                &other_names,
            )
            .await
            .expect("wiring succeeds");

        assert_eq!(answer.blocked_by, vec!["b".to_owned(), "c".to_owned()]);
    }

    #[tokio::test]
    async fn the_request_carries_a_json_schema_grammar_when_asked_for_one() {
        let engine = FakeEngine::new(Script {
            turns: vec![Turn {
                text: "[]".to_owned(),
                ..Default::default()
            }],
            ..Default::default()
        });
        let wirer = DefaultWirer;
        let ticket_a = ticket("A", 0);
        let other_names = vec!["b".to_owned()];

        wirer
            .wire(
                &engine,
                RoleClass::Architect,
                MicroSampling::classify(),
                &ticket_a,
                &other_names,
            )
            .await
            .expect("wiring succeeds");

        let seen = engine.seen_requests();
        assert!(matches!(seen[0].grammar, Some(Grammar::JsonSchema(_))));
    }

    // -- detect_cycle / break_cycles ------------------------------------

    /// Task unit `E6`'s brief: "Test a five-node cycle."
    #[test]
    fn detect_cycle_finds_the_cycle_in_a_five_node_ring() {
        let tickets: Vec<ChartedTicket> = ["A", "B", "C", "D", "E"]
            .iter()
            .enumerate()
            .map(|(index, id)| ticket(id, i64::try_from(index).unwrap()))
            .collect();
        let edges = vec![
            edge("A", "B"),
            edge("B", "C"),
            edge("C", "D"),
            edge("D", "E"),
            edge("E", "A"),
        ];

        let cycle = detect_cycle(&tickets, &edges).expect("a five-node ring is a cycle");
        assert_eq!(cycle.len(), 5);
        for id in ["A", "B", "C", "D", "E"] {
            assert!(cycle.contains(&id.to_owned()));
        }
    }

    #[test]
    fn break_cycles_drops_the_edge_whose_blocker_has_the_higher_ordinal() {
        let tickets: Vec<ChartedTicket> = ["A", "B", "C", "D", "E"]
            .iter()
            .enumerate()
            .map(|(index, id)| ticket(id, i64::try_from(index).unwrap()))
            .collect();
        let edges = vec![
            edge("A", "B"),
            edge("B", "C"),
            edge("C", "D"),
            edge("D", "E"),
            edge("E", "A"), // E has the highest ordinal (4) of any blocker on this ring.
        ];

        let (remaining, breaks) = break_cycles(&tickets, edges).expect("the ring is breakable");

        assert!(
            detect_cycle(&tickets, &remaining).is_none(),
            "no cycle remains"
        );
        assert_eq!(breaks.len(), 1);
        assert_eq!(breaks[0].blocker, "E");
        assert_eq!(breaks[0].blocked, "A");
        assert_eq!(breaks[0].cycle.len(), 5);
        assert_eq!(remaining.len(), 4);
    }

    // -- transitive_reduce -----------------------------------------------

    #[test]
    fn transitive_reduce_removes_an_edge_implied_by_a_two_hop_path() {
        let edges = vec![edge("A", "B"), edge("B", "C"), edge("A", "C")];

        let (kept, removed) = transitive_reduce(edges);

        assert_eq!(removed, vec![edge("A", "C")]);
        assert_eq!(kept.len(), 2);
        assert!(kept.contains(&edge("A", "B")));
        assert!(kept.contains(&edge("B", "C")));
    }

    #[test]
    fn transitive_reduce_keeps_edges_between_unrelated_siblings() {
        let edges = vec![edge("A", "B"), edge("A", "C"), edge("A", "D")];

        let (kept, removed) = transitive_reduce(edges.clone());

        assert!(removed.is_empty());
        assert_eq!(kept.len(), edges.len());
    }

    // -- cap_out_degree ---------------------------------------------------

    #[test]
    fn cap_out_degree_flags_a_ticket_blocking_more_than_five_others_without_removing_edges() {
        let tickets: Vec<ChartedTicket> = ["A", "B", "C", "D", "E", "F", "G", "H"]
            .iter()
            .enumerate()
            .map(|(index, id)| ticket(id, i64::try_from(index).unwrap()))
            .collect();
        let edges = vec![
            edge("A", "B"),
            edge("A", "C"),
            edge("A", "D"),
            edge("A", "E"),
            edge("A", "F"),
            edge("A", "G"),
        ];

        let flagged = cap_out_degree(&tickets, &edges);

        assert_eq!(flagged, vec!["A".to_owned()]);
    }

    #[test]
    fn cap_out_degree_does_not_flag_a_ticket_at_exactly_the_cap() {
        let tickets: Vec<ChartedTicket> = ["A", "B", "C", "D", "E", "F"]
            .iter()
            .enumerate()
            .map(|(index, id)| ticket(id, i64::try_from(index).unwrap()))
            .collect();
        let edges = vec![
            edge("A", "B"),
            edge("A", "C"),
            edge("A", "D"),
            edge("A", "E"),
            edge("A", "F"),
        ];

        assert!(cap_out_degree(&tickets, &edges).is_empty());
    }

    // -- check_frontier ----------------------------------------------------

    #[test]
    fn check_frontier_rejects_a_graph_where_every_ticket_is_blocked() {
        let tickets = vec![ticket("A", 0), ticket("B", 1)];
        let edges = vec![edge("A", "B"), edge("B", "A")];

        let err = check_frontier(&tickets, &edges).expect_err("a mutual block has no frontier");
        assert_eq!(err.code, ErrCode::MapEmptyFrontier);
    }

    #[test]
    fn check_frontier_accepts_a_graph_with_an_unblocked_ticket() {
        let tickets = vec![ticket("A", 0), ticket("B", 1)];
        let edges = vec![edge("A", "B")];

        check_frontier(&tickets, &edges).expect("A is takeable");
    }

    // -- repair_wiring, end to end -----------------------------------------

    #[test]
    fn repair_wiring_breaks_cycles_reduces_transitively_and_flags_out_degree_together() {
        let tickets: Vec<ChartedTicket> = ["A", "B", "C", "D", "E"]
            .iter()
            .enumerate()
            .map(|(index, id)| ticket(id, i64::try_from(index).unwrap()))
            .collect();
        // A five-node cycle A->B->C->D->E->A, plus an edge implied once
        // the cycle is broken (A->C, implied by A->B->C).
        let edges = vec![
            edge("A", "B"),
            edge("B", "C"),
            edge("C", "D"),
            edge("D", "E"),
            edge("E", "A"),
            edge("A", "C"),
        ];

        let report = repair_wiring(&tickets, edges).expect("the fixture repairs cleanly");

        assert_eq!(report.cycles_broken.len(), 1);
        assert_eq!(report.transitive_edges_removed, vec![edge("A", "C")]);
        assert!(report.high_out_degree.is_empty());
        assert!(detect_cycle(&tickets, &report.edges).is_none());
        check_frontier(&tickets, &report.edges).expect("a takeable ticket remains");
    }

    #[test]
    fn repair_wiring_ignores_a_self_blocking_edge() {
        let tickets = vec![ticket("A", 0)];
        let edges = vec![edge("A", "A")];

        let report = repair_wiring(&tickets, edges).expect("a single self-loop still repairs");
        assert!(report.edges.is_empty());
    }
}
