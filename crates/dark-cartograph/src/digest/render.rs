//! Turns a [`MapSnapshot`] into digest text, compressing when the text
//! runs over budget.
//!
//! [`render`] is a pure function of its inputs: the same snapshot and
//! the same tier always produce the same bytes. That purity is what
//! keeps the digest safe to sit in the context prefix (see the module
//! documentation on `crate::digest`, and Rule 5 in `CLAUDE.md`) — two
//! calls with unchanged map state must never disagree, or the engine
//! pays for a full prefill it did not need.

use crate::frontier::FrontierTicket;
use crate::journal::MapStatus;

use super::estimate::{ESTIMATED_BUDGET, estimate_tokens};
use super::query::{Blocked, Decision, Fog, MapSnapshot, ScopeExclusion};

/// How many of the most recent decisions survive the first compression
/// step (task unit `D3`, step 2.1).
const DECISIONS_KEPT: usize = 10;

/// How compressed the blocked-tickets section is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum BlockedLevel {
    /// One line per blocked ticket, naming it and every active blocker.
    #[default]
    Full,
    /// One compact `blocked ← blockers` fragment per ticket, all joined
    /// onto a single line (task unit `D3`, step 2.2).
    EdgeNotation,
    /// Only the count in the section header (task unit `D3`, step 2.3).
    CountOnly,
}

/// Which compression steps are active. Every field only ever moves
/// towards more compression; [`escalate`] is the one place that changes
/// one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct Compression {
    /// `true` once decisions collapse to the ten most recent plus a
    /// count (task unit `D3`, step 2.1).
    decisions_collapsed: bool,
    /// How compressed the blocked section is.
    blocked: BlockedLevel,
    /// `true` once every fog patch truncates to its first sentence
    /// (task unit `D3`, step 2.3).
    fog_truncated: bool,
}

impl Compression {
    /// Returns the next, more compressed state in the fixed sequence
    /// task unit `D3` step 2 names, or `None` once every step has run.
    ///
    /// The frontier never appears here: it never compresses, at any
    /// step (task unit `D3`, step 3).
    fn escalate(self) -> Option<Self> {
        if !self.decisions_collapsed {
            return Some(Self {
                decisions_collapsed: true,
                ..self
            });
        }
        if self.blocked == BlockedLevel::Full {
            return Some(Self {
                blocked: BlockedLevel::EdgeNotation,
                ..self
            });
        }
        if self.blocked == BlockedLevel::EdgeNotation {
            return Some(Self {
                blocked: BlockedLevel::CountOnly,
                ..self
            });
        }
        if !self.fog_truncated {
            return Some(Self {
                fog_truncated: true,
                ..self
            });
        }
        None
    }
}

/// The digest tier: how much of the map a rendered digest carries.
///
/// See task unit `D3`, step 4.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier {
    /// Everything: header, destination, notes, decisions, frontier,
    /// blocked, fog, and scope exclusions. For ticket resolution turns.
    Full,
    /// Destination, notes, and the frontier only. About 300 tokens. For
    /// charting stages that need orientation.
    FrontierOnly,
    /// Nothing. For charting stages 4 to 7, and for resolution
    /// recording.
    None,
}

/// Renders `snapshot` at `tier`, compressing [`Tier::Full`] when the
/// text runs over the estimated budget.
///
/// Tries every [`Compression`] state in the fixed sequence task unit
/// `D3` step 2 names, from least to most compressed, and returns the
/// first render that fits [`ESTIMATED_BUDGET`]. Returns the most
/// compressed render when none fits — a digest over budget is still a
/// working digest, and refusing to produce one would serve the caller
/// worse.
pub(super) fn render(snapshot: &MapSnapshot, tier: Tier) -> String {
    match tier {
        Tier::None => String::new(),
        Tier::FrontierOnly => render_frontier_only(snapshot),
        Tier::Full => render_full(snapshot),
    }
}

/// Renders the `FrontierOnly` tier: destination, notes, and the
/// frontier, with no compression — this tier is already small (task
/// unit `D3`, step 4).
fn render_frontier_only(snapshot: &MapSnapshot) -> String {
    let mut blocks = vec![destination_block(snapshot)];
    if let Some(notes) = notes_block(snapshot) {
        blocks.push(notes);
    }
    blocks.push(frontier_block(&snapshot.frontier));
    blocks.join("\n\n")
}

/// Renders the `Full` tier, escalating compression until the estimated
/// size fits [`ESTIMATED_BUDGET`] or every step has run.
fn render_full(snapshot: &MapSnapshot) -> String {
    let mut compression = Compression::default();
    let mut text = render_at(snapshot, compression);

    while estimate_tokens(&text) > ESTIMATED_BUDGET {
        let Some(next) = compression.escalate() else {
            break;
        };
        compression = next;
        text = render_at(snapshot, compression);
    }

    text
}

/// Renders every `Full`-tier section at one [`Compression`] state.
fn render_at(snapshot: &MapSnapshot, compression: Compression) -> String {
    let mut blocks = vec![header_block(snapshot), destination_block(snapshot)];
    if let Some(notes) = notes_block(snapshot) {
        blocks.push(notes);
    }
    if let Some(decisions) = decisions_block(&snapshot.decisions, compression.decisions_collapsed) {
        blocks.push(decisions);
    }
    blocks.push(frontier_block(&snapshot.frontier));
    if let Some(blocked) = blocked_block(&snapshot.blocked, compression.blocked) {
        blocks.push(blocked);
    }
    if let Some(fog) = fog_block(&snapshot.fog, compression.fog_truncated) {
        blocks.push(fog);
    }
    if let Some(scope) = scope_exclusions_block(&snapshot.scope_exclusions) {
        blocks.push(scope);
    }
    blocks.join("\n\n")
}

/// Renders the `MAP:` header line.
fn header_block(snapshot: &MapSnapshot) -> String {
    format!(
        "MAP: {}  [{} · {} tickets · {} resolved]",
        snapshot.name,
        status_str(snapshot.status),
        snapshot.ticket_count,
        snapshot.resolved_count
    )
}

/// Renders the `DESTINATION` block.
fn destination_block(snapshot: &MapSnapshot) -> String {
    format!("DESTINATION\n  {}", snapshot.destination)
}

/// Renders the `NOTES` block, when the map has notes.
fn notes_block(snapshot: &MapSnapshot) -> Option<String> {
    snapshot
        .notes
        .as_ref()
        .filter(|notes| !notes.is_empty())
        .map(|notes| format!("NOTES\n  {notes}"))
}

/// Renders the `DECISIONS SO FAR` block, when the map has resolved any
/// tickets. `decisions` is already sorted most-recently-resolved first
/// (see `crate::digest::query::load_decisions`).
fn decisions_block(decisions: &[Decision], collapsed: bool) -> Option<String> {
    if decisions.is_empty() {
        return None;
    }

    let mut lines = vec![format!("DECISIONS SO FAR ({})", decisions.len())];
    let shown = if collapsed && decisions.len() > DECISIONS_KEPT {
        DECISIONS_KEPT
    } else {
        decisions.len()
    };
    for decision in &decisions[..shown] {
        let gist = decision
            .gist
            .as_ref()
            .map_or(String::new(), |gist| format!("  → {gist}"));
        lines.push(format!("  {} {}{gist}", decision.id, decision.name));
    }
    if shown < decisions.len() {
        lines.push(format!(
            "  … {} more · zoom with ticket_zoom(id)",
            decisions.len() - shown
        ));
    }
    Some(lines.join("\n"))
}

/// Renders the `FRONTIER` block. Never compressed (task unit `D3`, step
/// 3): every takeable ticket appears, in full.
fn frontier_block(frontier: &[FrontierTicket]) -> String {
    let mut lines = vec![format!("FRONTIER ({} takeable now)", frontier.len())];
    for ticket in frontier {
        let presence = if ticket.hitl { "HITL" } else { "AFK" };
        lines.push(format!(
            "  {} [{}·{presence}]  {}",
            ticket.id,
            ticket.ticket_type.as_str(),
            ticket.name
        ));
    }
    lines.join("\n")
}

/// Renders the `BLOCKED` block, when any ticket is blocked.
fn blocked_block(blocked: &[Blocked], level: BlockedLevel) -> Option<String> {
    if blocked.is_empty() {
        return None;
    }

    let header = format!("BLOCKED ({})", blocked.len());
    let body = match level {
        BlockedLevel::Full => {
            let mut lines = vec![header];
            for entry in blocked {
                lines.push(format!(
                    "  {} {}  ← blocked by {}",
                    entry.id,
                    entry.name,
                    entry.blockers.join(", ")
                ));
            }
            lines.join("\n")
        }
        BlockedLevel::EdgeNotation => {
            let fragments: Vec<String> = blocked
                .iter()
                .map(|entry| format!("{} ← {}", entry.id, entry.blockers.join(",")))
                .collect();
            format!("{header}\n  {}", fragments.join(" · "))
        }
        BlockedLevel::CountOnly => header,
    };
    Some(body)
}

/// Renders the `NOT YET SPECIFIED (fog)` block, when the map has any fog
/// left.
fn fog_block(fog: &[Fog], truncated: bool) -> Option<String> {
    if fog.is_empty() {
        return None;
    }

    let mut lines = vec!["NOT YET SPECIFIED (fog)".to_owned()];
    for patch in fog {
        let text = if truncated {
            first_sentence(&patch.patch)
        } else {
            patch.patch.as_str()
        };
        lines.push(format!("  · {text}"));
    }
    Some(lines.join("\n"))
}

/// Renders the `OUT OF SCOPE` block, when the map excludes anything.
/// Never compressed: task unit `D3` names no compression step for scope
/// exclusions, and this section is already the shortest per entry.
fn scope_exclusions_block(exclusions: &[ScopeExclusion]) -> Option<String> {
    if exclusions.is_empty() {
        return None;
    }

    let mut lines = vec![format!("OUT OF SCOPE ({})", exclusions.len())];
    for exclusion in exclusions {
        let ticket = exclusion
            .ticket_id
            .as_ref()
            .map_or(String::new(), |id| format!(" ({id})"));
        lines.push(format!(
            "  · {} — {}{ticket}",
            exclusion.gist, exclusion.reason
        ));
    }
    Some(lines.join("\n"))
}

/// Returns the first sentence of `text`: everything up to and including
/// the first `.`, `!`, or `?`, or the whole text when it has none.
fn first_sentence(text: &str) -> &str {
    let end = text.find(['.', '!', '?']).map_or(text.len(), |idx| idx + 1);
    text[..end].trim_end()
}

/// Renders a [`MapStatus`] the way the digest header shows it: matching
/// [`MapStatus::as_str`], since that string is already the short,
/// lowercase form the header line wants.
fn status_str(status: MapStatus) -> &'static str {
    status.as_str()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::journal::TicketType;

    fn empty_snapshot() -> MapSnapshot {
        MapSnapshot {
            name: "Test map".to_owned(),
            destination: "A tested destination".to_owned(),
            notes: None,
            status: MapStatus::Active,
            ticket_count: 0,
            resolved_count: 0,
            decisions: Vec::new(),
            blocked: Vec::new(),
            fog: Vec::new(),
            scope_exclusions: Vec::new(),
            frontier: Vec::new(),
        }
    }

    fn frontier_ticket(id: &str, name: &str, hitl: bool) -> FrontierTicket {
        FrontierTicket {
            id: id.to_owned(),
            name: name.to_owned(),
            question: format!("What does {id} answer?"),
            ticket_type: TicketType::Task,
            hitl,
            ordinal: 0,
        }
    }

    #[test]
    fn tier_none_renders_nothing() {
        assert_eq!(render(&empty_snapshot(), Tier::None), "");
    }

    #[test]
    fn tier_frontier_only_omits_the_header_and_decisions() {
        let mut snapshot = empty_snapshot();
        snapshot.decisions.push(Decision {
            id: "T1".to_owned(),
            name: "A decision".to_owned(),
            gist: None,
        });
        let text = render(&snapshot, Tier::FrontierOnly);
        assert!(text.contains("DESTINATION"));
        assert!(text.contains("FRONTIER"));
        assert!(!text.contains("MAP:"));
        assert!(!text.contains("DECISIONS"));
    }

    #[test]
    fn full_tier_shows_the_header_destination_and_frontier() {
        let mut snapshot = empty_snapshot();
        snapshot
            .frontier
            .push(frontier_ticket("T-018", "Staleness policy", true));
        let text = render(&snapshot, Tier::Full);
        assert!(text.starts_with("MAP: Test map"));
        assert!(text.contains("[active · 0 tickets · 0 resolved]"));
        assert!(text.contains("DESTINATION"));
        assert!(text.contains("T-018 [task·HITL]  Staleness policy"));
    }

    #[test]
    fn a_ticket_name_appears_before_a_bare_identifier_would_read_alone() {
        // D3 step 5: show the name, not a bare number list.
        let mut snapshot = empty_snapshot();
        snapshot.frontier.push(frontier_ticket(
            "T-021",
            "Generate the fixture corpus",
            false,
        ));
        let text = render(&snapshot, Tier::Full);
        assert!(text.contains("Generate the fixture corpus"));
    }

    #[test]
    fn decisions_collapse_to_the_ten_most_recent_plus_a_count() {
        let decisions: Vec<String> = decisions_block(
            &(0..15)
                .map(|i| Decision {
                    id: format!("T{i}"),
                    name: format!("Decision {i}"),
                    gist: None,
                })
                .collect::<Vec<_>>(),
            true,
        )
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect();
        assert_eq!(decisions[0], "DECISIONS SO FAR (15)");
        // Header + 10 kept + one collapsed-count line.
        assert_eq!(decisions.len(), 12);
        assert_eq!(decisions[11], "  … 5 more · zoom with ticket_zoom(id)");
    }

    #[test]
    fn decisions_uncollapsed_shows_every_one() {
        let decisions: Vec<Decision> = (0..15)
            .map(|i| Decision {
                id: format!("T{i}"),
                name: format!("Decision {i}"),
                gist: None,
            })
            .collect();
        let text = decisions_block(&decisions, false).unwrap();
        assert_eq!(text.lines().count(), 16, "header plus all 15, uncompressed");
    }

    #[test]
    fn blocked_full_names_every_blocker() {
        let blocked = vec![Blocked {
            id: "T-020".to_owned(),
            name: "Some ticket".to_owned(),
            blockers: vec!["T-018".to_owned()],
        }];
        let text = blocked_block(&blocked, BlockedLevel::Full).unwrap();
        assert!(text.contains("T-020 Some ticket  ← blocked by T-018"));
    }

    #[test]
    fn blocked_edge_notation_joins_every_ticket_onto_one_line() {
        let blocked = vec![
            Blocked {
                id: "T-020".to_owned(),
                name: "A".to_owned(),
                blockers: vec!["T-018".to_owned()],
            },
            Blocked {
                id: "T-022".to_owned(),
                name: "B".to_owned(),
                blockers: vec!["T-019".to_owned(), "T-021".to_owned()],
            },
        ];
        let text = blocked_block(&blocked, BlockedLevel::EdgeNotation).unwrap();
        assert_eq!(text, "BLOCKED (2)\n  T-020 ← T-018 · T-022 ← T-019,T-021");
    }

    #[test]
    fn blocked_count_only_names_no_ticket() {
        let blocked = vec![Blocked {
            id: "T-020".to_owned(),
            name: "A".to_owned(),
            blockers: vec!["T-018".to_owned()],
        }];
        let text = blocked_block(&blocked, BlockedLevel::CountOnly).unwrap();
        assert_eq!(text, "BLOCKED (1)");
    }

    #[test]
    fn fog_truncates_to_the_first_sentence_when_asked() {
        let fog = vec![Fog {
            patch: "How packs are distributed. This is a second sentence.".to_owned(),
        }];
        let text = fog_block(&fog, true).unwrap();
        assert_eq!(
            text,
            "NOT YET SPECIFIED (fog)\n  · How packs are distributed."
        );
    }

    #[test]
    fn fog_stays_whole_when_not_truncated() {
        let fog = vec![Fog {
            patch: "How packs are distributed. This is a second sentence.".to_owned(),
        }];
        let text = fog_block(&fog, false).unwrap();
        assert!(text.contains("This is a second sentence."));
    }

    #[test]
    fn first_sentence_returns_the_whole_text_when_there_is_no_terminator() {
        assert_eq!(first_sentence("no terminator here"), "no terminator here");
    }

    #[test]
    fn scope_exclusions_name_the_ticket_when_one_raised_it() {
        let exclusions = vec![ScopeExclusion {
            gist: "Pack signing and trust chain".to_owned(),
            reason: "separate effort".to_owned(),
            ticket_id: Some("T-009".to_owned()),
        }];
        let text = scope_exclusions_block(&exclusions).unwrap();
        assert!(text.contains("Pack signing and trust chain — separate effort (T-009)"));
    }

    #[test]
    fn full_tier_escalates_compression_until_the_estimate_fits() {
        let mut snapshot = empty_snapshot();
        // Enough resolved tickets, each with a long gist, to push the
        // uncompressed render over the estimated budget.
        for i in 0..40 {
            snapshot.decisions.push(Decision {
                id: format!("T-{i:03}"),
                name: "A moderately long decision statement about the pack format".to_owned(),
                gist: Some(
                    "a fairly long summary of the resolution that took several words".to_owned(),
                ),
            });
        }
        let text = render_full(&snapshot);
        assert!(estimate_tokens(&text) <= ESTIMATED_BUDGET);
        assert!(text.contains("zoom with ticket_zoom(id)"));
    }

    #[test]
    fn full_tier_never_compresses_the_frontier_even_far_over_budget() {
        let mut snapshot = empty_snapshot();
        for i in 0..400 {
            snapshot.frontier.push(frontier_ticket(
                &format!("T-{i:04}"),
                "A frontier ticket with a reasonably descriptive name",
                i % 2 == 0,
            ));
        }
        let text = render_full(&snapshot);
        assert_eq!(
            text.matches("FRONTIER (400 takeable now)").count(),
            1,
            "the frontier count must be unchanged"
        );
        for ticket in &snapshot.frontier {
            assert!(
                text.contains(&ticket.id),
                "every frontier ticket must still appear: missing {}",
                ticket.id
            );
        }
    }

    #[test]
    fn render_is_pure() {
        let mut snapshot = empty_snapshot();
        snapshot.frontier.push(frontier_ticket("T-018", "A", true));
        assert_eq!(render(&snapshot, Tier::Full), render(&snapshot, Tier::Full));
    }
}
