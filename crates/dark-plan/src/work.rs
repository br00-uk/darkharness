//! The `work` stage: resolve one ticket from an already-charted map.
//!
//! Task unit `E7`. `dark-cartograph` (task unit `D4`) stores the map and
//! owns every tool that mutates it: `map_read`, `ticket_claim`,
//! `ticket_zoom`, `ticket_resolve`, `fog_graduate`, `scope_exclude`,
//! `ticket_invalidate`, `ticket_block`. `dark-plan` must not depend on
//! `dark-cartograph` (the module documentation on `crate::chart::ticket`
//! explains why: the map store is downstream of charting, not upstream of
//! it). So this module is not a second implementation of those tools. It
//! is the routing table task unit `E7`'s Do step 5 names, plus the guards
//! Rules 19 to 21 (`CLAUDE.md`) require, expressed as pure functions and
//! small session state that hold regardless of which store answers the
//! actual `ticket_claim` or `ticket_resolve` call. A caller that holds
//! both `dark-plan` and `dark-cartograph` — `dark-core` — sequences the
//! two: read the digest, call [`select_ticket`], claim the ticket through
//! `dark-cartograph`, call [`route`] to learn the method, do the work, then
//! resolve through `dark-cartograph` after [`WorkSession::record_resolution`]
//! agrees a resolution is still allowed.
//!
//! # The eleven Do steps, and where each one lives
//!
//! 1. **Load the digest. Do not load every ticket body.** A caller calls
//!    `map_read` with `tier: "frontier_only"` to build the
//!    [`WorkTicket`] slice this module's functions take; `tier: "full"` (or
//!    `ticket_zoom`) is for the one ticket [`select_ticket`] picks, not
//!    for the whole map. `dark-plan` never sees the digest text itself —
//!    it is `dark-cartograph`'s rendered string (see
//!    `dark_cartograph::digest`) — so this module works from the
//!    already-parsed [`WorkTicket`] list a caller extracts from it.
//! 2. **Select the ticket.** [`select_ticket`].
//! 3. **Claim the ticket before any work starts.** `ticket_claim` is a
//!    mutating call into `dark-cartograph`; this module has no seam for it
//!    (see the module documentation on why one is not worth adding) — a
//!    caller claims between [`select_ticket`] and [`route`].
//! 4. **Resolve it. Call `ticket_zoom` on a related ticket only when
//!    needed.** `ticket_zoom` is `dark-cartograph`'s; [`route`] only says
//!    which method the ticket needs.
//! 5. **Route by ticket type.** [`route`] and [`WorkMethod`]. This is the
//!    point of this task unit.
//! 6. **Record the resolution in one transaction.** `dark-cartograph`'s
//!    `ticket_resolve` already does this (task unit `D4`, Do step 2).
//!    [`WorkSession::record_resolution`] is the session-side twin: it
//!    enforces Rule 20 from inside `dark-plan`, so the limit is provable
//!    without a `dark-cartograph` dependency, the same way `ticket_resolve`
//!    proves it from inside `dark-cartograph`.
//! 7. **Graduate the fog that the answer made specifiable.** [`graduate_fog`]
//!    assigns each graduated candidate an identifier and an ordinal, the
//!    same shape stage 6 of charting assigns
//!    (`crate::chart::pipeline::ChartPipeline::stage_size`). Wiring the new
//!    tickets reuses `crate::wire::repair_wiring` unchanged; clearing the
//!    patch is `dark-cartograph`'s `fog_graduate`.
//! 8. **Close an out-of-scope ticket.** [`close_out_of_scope`] builds the
//!    [`crate::chart::ScopeExclusion`] record; recording it and refusing
//!    to resolve the ticket is a caller decision — read literally, "do not
//!    resolve it" means [`WorkSession::record_resolution`] is never called
//!    for this ticket at all, so no code here needs to refuse it a second
//!    time.
//! 9. **Update or delete an invalidated ticket.** [`TicketInvalidation`]
//!    carries the `ticket_invalidate(ticket_id, reason)` shape D4's tool
//!    table names; issuing it is `dark-cartograph`'s job.
//! 10. **Stop after one ticket. Research tickets are exempt.**
//!     [`WorkSession::record_resolution`].
//! 11. **Limit parallel sub-agents. The default is 2.**
//!     [`research_parallelism`].

use dark_contract::{ErrCode, Error, ResidencySnapshot, Result};
use serde::{Deserialize, Serialize};
use ulid::Ulid;

use crate::chart::{Candidate, ChartedTicket, ScopeExclusion, TicketKind};

/// A ticket as `work.rs` sees it: enough to select it, claim it, and route
/// it.
///
/// Mirrors `dark_cartograph::journal::event::TicketCreated` by field name —
/// `id`, `name`, `question`, `ticket_type`, `hitl`, `ordinal` — minus the
/// fields a router never reads (`map_id`, `status`, `created_at`, `axis`,
/// `tokens_used`). A caller builds one of these from a row `map_read`'s
/// digest describes, or from `ticket_zoom`; this module never constructs
/// one from a live map itself, since it has no seam into `dark-cartograph`
/// to read one with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkTicket {
    /// The ticket's identifier.
    pub id: String,
    /// The ticket's short name.
    pub name: String,
    /// The question the ticket answers.
    pub question: String,
    /// The kind of ticket.
    pub ticket_type: TicketKind,
    /// `true` when the ticket needs a person present to resolve it.
    pub hitl: bool,
    /// The ticket's position among its siblings on the frontier. Lower
    /// sorts first.
    pub ordinal: i64,
}

/// Selects the ticket to work.
///
/// Task unit `E7`, Do step 2: "Use the named ticket. Otherwise use the
/// first frontier ticket by ordinal." `frontier` is the workable set the
/// caller computed — `dark-cartograph` computes the real frontier (a
/// ticket blocked by an unresolved one is not ready), and this module
/// takes that computation as an input rather than reaching for the map's
/// edges itself, since it has none to reach for. See the module
/// documentation on why `dark-plan` cannot compute the frontier on its
/// own.
///
/// `named` matches a [`WorkTicket::id`] or a [`WorkTicket::name`], so a
/// caller may pass either the identifier `/plan work` was given or the
/// exact name a person typed.
///
/// # Errors
///
/// Returns [`ErrCode::MapNotFound`] when `named` is `Some` and matches no
/// ticket in `frontier` — the ticket does not exist, or something still
/// blocks it, and either way `frontier` is all this function was given to
/// look in. Returns [`ErrCode::MapEmptyFrontier`] when `named` is `None`
/// and `frontier` is empty.
pub fn select_ticket<'a>(
    named: Option<&str>,
    frontier: &'a [WorkTicket],
) -> Result<&'a WorkTicket> {
    if let Some(needle) = named {
        return frontier
            .iter()
            .find(|ticket| ticket.id == needle || ticket.name == needle)
            .ok_or_else(|| {
                Error::new(
                    ErrCode::MapNotFound,
                    format!(
                        "{needle:?} is not on the frontier: it does not exist, or a blocker \
                         has not resolved yet"
                    ),
                )
                .with_remedy("Read the digest again. Work an unblocked ticket instead.")
            });
    }

    frontier
        .iter()
        .min_by_key(|ticket| ticket.ordinal)
        .ok_or_else(|| Error::new(ErrCode::MapEmptyFrontier, "the frontier is empty"))
}

/// Returns whether `ticket` needs a person present to resolve it.
///
/// Task unit `E7`, Do step 5's routing table: "`grilling` and anything
/// with `hitl` set needs a person." This checks both conditions rather
/// than trusting [`WorkTicket::hitl`] alone: [`TicketKind::default_hitl`]
/// sets `hitl` for a fresh `grilling` ticket, but nothing stops a caller
/// handing this function a [`WorkTicket`] built by hand with `hitl: false`
/// and `ticket_type: TicketKind::Grilling` — the kind still means a
/// person must decide, whatever the flag says.
#[must_use]
pub fn requires_human(ticket: &WorkTicket) -> bool {
    ticket.hitl || ticket.ticket_type == TicketKind::Grilling
}

/// The method a ticket's kind routes to.
///
/// Task unit `E7`, Do step 5's routing table, minus the "Human present"
/// column — that is [`requires_human`] and the guard [`route`] applies
/// before it ever returns one of these.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WorkMethod {
    /// A sub-session with read-only tools: [`RESEARCH_TOOLS`]. No person
    /// present. Bounded — it needs retrieval, not long reasoning. The
    /// build specification's preferred method: "It is bounded and needs
    /// retrieval, not long reasoning."
    Research,
    /// A cheap, rough artefact, linked as an asset, discussed with the
    /// person once it exists.
    Prototype,
    /// Conversation with the person present, using the `deliberate`
    /// micro-role.
    Grilling,
    /// The manual work that unblocks the decision. Either a person or the
    /// model may do it; record what was done and the facts later tickets
    /// need.
    Task,
}

impl WorkMethod {
    /// Maps a ticket's kind to its method. A one-to-one correspondence:
    /// the routing table names exactly one method for each of the four
    /// [`TicketKind`] values.
    #[must_use]
    pub fn for_kind(kind: TicketKind) -> Self {
        match kind {
            TicketKind::Research => Self::Research,
            TicketKind::Prototype => Self::Prototype,
            TicketKind::Grilling => Self::Grilling,
            TicketKind::Task => Self::Task,
        }
    }
}

/// The read-only tools a [`WorkMethod::Research`] sub-session gets.
///
/// Task unit `E7`, Do step 5's routing table, `research` row: "A
/// sub-session with read-only tools: `docs_*`, `grep`, `read_file`,
/// `explore`."
pub const RESEARCH_TOOLS: [&str; 4] = ["docs_*", "grep", "read_file", "explore"];

/// Routes `ticket` to the method that resolves it.
///
/// Task unit `E7`, Do step 5: the routing table, this task unit's whole
/// point. **A ticket that needs a person must not be worked by a model:**
/// this function is the one place that decision gets made, and it makes
/// it before anything downstream sees a [`WorkMethod`] at all — a caller
/// that only ever routes through this function can never receive
/// [`WorkMethod::Grilling`] (or any method at all, for a `hitl` `task`
/// ticket) for headless work.
///
/// # Errors
///
/// Returns [`ErrCode::HitlRequiresHuman`] when [`requires_human`] returns
/// `true` for `ticket` and `human_present` is `false`. Matches Rule 19
/// (`CLAUDE.md`): "`ticket_resolve` on a human-in-the-loop ticket fails
/// with `E_HITL_REQUIRES_HUMAN`. It succeeds only when the session holds a
/// human-present token."
pub fn route(ticket: &WorkTicket, human_present: bool) -> Result<WorkMethod> {
    if requires_human(ticket) && !human_present {
        return Err(Error::new(
            ErrCode::HitlRequiresHuman,
            format!(
                "ticket {:?} needs a person; no human-present token",
                ticket.id
            ),
        ));
    }
    Ok(WorkMethod::for_kind(ticket.ticket_type))
}

/// Tracks what one harness session has already resolved.
///
/// Enforces Rule 20 (`CLAUDE.md`): "A session resolves one ticket. A
/// second resolution fails with `E_SESSION_RESOLUTION_LIMIT`. Research
/// tickets are exempt." `dark-cartograph`'s `ticket_resolve` enforces the
/// same rule from its own session state (task unit `D4`); this is the
/// `dark-plan`-side twin, so a caller can prove the limit holds — and a
/// test can exercise it — without a `dark-cartograph` dependency.
///
/// A fresh [`WorkSession`] starts with nothing resolved.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct WorkSession {
    resolved_non_research: bool,
}

impl WorkSession {
    /// Creates a session that has resolved nothing yet.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns whether this session has already resolved a non-research
    /// ticket.
    #[must_use]
    pub fn has_resolved_non_research(self) -> bool {
        self.resolved_non_research
    }

    /// Checks whether resolving a ticket of `kind` is still allowed, and
    /// records it when it is.
    ///
    /// Call this immediately before resolving — not before selecting,
    /// claiming, or routing a ticket, all of which a session may still do
    /// a second time. Task unit `E7`, Do step 10: "Do not resolve a
    /// second non-research ticket in one session. Research tickets are
    /// exempt."
    ///
    /// # Errors
    ///
    /// Returns [`ErrCode::SessionResolutionLimit`] when `kind` is not
    /// [`TicketKind::Research`] and this session already resolved a
    /// non-research ticket. Leaves the session unchanged on this path — a
    /// refused resolution is not a resolution.
    pub fn record_resolution(&mut self, kind: TicketKind) -> Result<()> {
        if kind == TicketKind::Research {
            return Ok(());
        }
        if self.resolved_non_research {
            return Err(Error::new(
                ErrCode::SessionResolutionLimit,
                "this session already resolved a ticket",
            ));
        }
        self.resolved_non_research = true;
        Ok(())
    }
}

/// The default limit on parallel research sub-agents, when the resident
/// set has ample headroom.
///
/// Task unit `E7`, Do step 11: "Limit parallel sub-agents. Read the
/// headroom from the resident set. The default is 2."
pub const DEFAULT_SUBAGENT_LIMIT: usize = 2;

/// Returns how many [`WorkMethod::Research`] sub-agents may run in
/// parallel right now.
///
/// Task unit `E7`, Do step 11: "Each sub-agent holds a key-value cache,"
/// so the resident set's headroom caps the count directly:
/// `headroom_bytes / per_agent_bytes`, never more than
/// [`DEFAULT_SUBAGENT_LIMIT`] — Do not step: "Do not start eight research
/// sub-agents. That exhausts memory." `dark-plan` does not estimate
/// `per_agent_bytes` itself; that estimate is the resident set manager's
/// own (task unit `B3`, Rule 4: "The resident set manager estimates
/// memory before a load"), so the caller passes it in, already computed
/// from `Caps::granted_context` for the class a sub-agent's engine calls
/// would use.
///
/// Returns `0` when the headroom will not fit even one sub-agent: a
/// caller reading `0` works the ticket without a sub-agent, rather than
/// starting one that the estimate says will not fit. Returns
/// [`DEFAULT_SUBAGENT_LIMIT`] when `per_agent_bytes` is `0` — nothing to
/// divide by, so headroom imposes no limit and the built-in default
/// stands alone.
#[must_use]
pub fn research_parallelism(residency: &ResidencySnapshot, per_agent_bytes: u64) -> usize {
    if per_agent_bytes == 0 {
        return DEFAULT_SUBAGENT_LIMIT;
    }
    let headroom = residency.budget_bytes.saturating_sub(residency.used_bytes);
    let by_headroom = headroom / per_agent_bytes;
    let capped = by_headroom.min(u64_from_usize(DEFAULT_SUBAGENT_LIMIT));
    // `capped` never exceeds `DEFAULT_SUBAGENT_LIMIT`, so it always fits
    // back into a `usize`; the fallback only satisfies clippy's
    // `cast_possible_truncation` lint on a plain `as` cast, and is never
    // reached.
    usize::try_from(capped).unwrap_or(DEFAULT_SUBAGENT_LIMIT)
}

/// Converts a `usize` to `u64` for the one comparison
/// [`research_parallelism`] needs. Infallible on every platform this
/// workspace targets (`usize` is 64 bits wide on all three build targets
/// in Section 4.5), but written as a checked conversion rather than an
/// `as` cast so clippy's pedantic lints have nothing to flag.
#[allow(clippy::missing_panics_doc)]
fn u64_from_usize(value: usize) -> u64 {
    u64::try_from(value).expect("usize fits in u64 on every supported target")
}

/// Turns fog that a resolution made specifiable into new tickets.
///
/// Task unit `E7`, Do step 7: "Graduate the fog that the answer made
/// specifiable. Create the tickets." Assigns each candidate a fresh
/// identifier and an ordinal starting at `next_ordinal`, the same shape
/// `crate::chart::pipeline::ChartPipeline::stage_size` assigns during
/// charting — a graduated ticket is not structurally different from a
/// charted one, it simply arrives later. Wiring the returned tickets'
/// blocking edges reuses `crate::wire::repair_wiring` unchanged; clearing
/// the fog patch that graduated is `dark-cartograph`'s `fog_graduate`
/// tool (task unit `D4`), not this function's job.
#[must_use]
pub fn graduate_fog(candidates: Vec<Candidate>, next_ordinal: i64) -> Vec<ChartedTicket> {
    candidates
        .into_iter()
        .enumerate()
        .map(|(offset, candidate)| ChartedTicket {
            id: Ulid::new().to_string(),
            name: candidate.name,
            question: candidate.question,
            ticket_type: candidate.kind,
            hitl: candidate.kind.default_hitl(),
            ordinal: next_ordinal.saturating_add(offset_as_i64(offset)),
            axis: vec![candidate.axis],
        })
        .collect()
}

/// Converts a zero-based position into the `i64` offset [`graduate_fog`]
/// adds to `next_ordinal`.
#[allow(clippy::cast_possible_wrap)]
fn offset_as_i64(offset: usize) -> i64 {
    offset as i64
}

/// Builds the record for a ticket a resolution showed to sit past the
/// destination.
///
/// Task unit `E7`, Do step 8: "When the answer shows that a ticket sits
/// past the destination, close that ticket. Add one line to the
/// out-of-scope section. Do not resolve it. A scope boundary is not a
/// step on the route." Unlike [`crate::chart::ScopeExclusion`] values
/// charting itself produces — which never carry a `ticket_id`, since
/// charting resolves nothing (see the field's own documentation) — this
/// one always does: it exists because resolving *this* ticket is exactly
/// what found the boundary. "Do not resolve it" is read literally: no
/// function in this module accepts a [`ScopeExclusion`] as proof that a
/// resolution happened, so a caller that builds one here and never calls
/// [`WorkSession::record_resolution`] for the same ticket has already
/// done everything this step asks.
#[must_use]
pub fn close_out_of_scope(
    ticket_id: impl Into<String>,
    gist: impl Into<String>,
    reason: impl Into<String>,
) -> ScopeExclusion {
    ScopeExclusion {
        gist: gist.into(),
        reason: reason.into(),
        ticket_id: Some(ticket_id.into()),
    }
}

/// What one ticket-invalidation decision carries.
///
/// Task unit `E7`, Do step 9: "When the decision invalidates another
/// ticket, update it or delete it." Mirrors the `ticket_invalidate`
/// tool's own signature from `D4`'s tool table — `ticket_invalidate(ticket_id,
/// reason)` — so a caller that holds `dark-cartograph` passes these two
/// fields straight through. Issuing the call is `dark-cartograph`'s job;
/// this module only carries the pair.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TicketInvalidation {
    /// The ticket that a later decision made void.
    pub ticket_id: String,
    /// Why the ticket no longer applies.
    pub reason: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ticket(id: &str, kind: TicketKind, hitl: bool, ordinal: i64) -> WorkTicket {
        WorkTicket {
            id: id.to_owned(),
            name: format!("ticket {id}"),
            question: format!("What resolves {id}?"),
            ticket_type: kind,
            hitl,
            ordinal,
        }
    }

    // -- select_ticket ---------------------------------------------------

    #[test]
    fn no_name_picks_the_lowest_ordinal_on_the_frontier() {
        let frontier = vec![
            ticket("B", TicketKind::Task, false, 3),
            ticket("A", TicketKind::Task, false, 1),
            ticket("C", TicketKind::Task, false, 2),
        ];

        let picked = select_ticket(None, &frontier).expect("the frontier is not empty");
        assert_eq!(picked.id, "A");
    }

    #[test]
    fn a_name_matches_by_id_or_by_name() {
        let frontier = vec![ticket("T-018", TicketKind::Task, false, 0)];

        assert_eq!(select_ticket(Some("T-018"), &frontier).unwrap().id, "T-018");
        assert_eq!(
            select_ticket(Some("ticket T-018"), &frontier).unwrap().id,
            "T-018"
        );
    }

    #[test]
    fn a_name_not_on_the_frontier_is_map_not_found() {
        let frontier = vec![ticket("T-018", TicketKind::Task, false, 0)];

        let err = select_ticket(Some("T-099"), &frontier)
            .expect_err("T-099 is not on the frontier, whether it exists elsewhere or not");
        assert_eq!(err.code, ErrCode::MapNotFound);
    }

    #[test]
    fn an_empty_frontier_with_no_name_is_empty_frontier() {
        let err = select_ticket(None, &[]).expect_err("nothing is takeable");
        assert_eq!(err.code, ErrCode::MapEmptyFrontier);
    }

    // -- requires_human / route -------------------------------------------

    #[test]
    fn grilling_always_requires_a_human_even_with_hitl_unset() {
        let ticket = ticket("T-018", TicketKind::Grilling, false, 0);
        assert!(requires_human(&ticket));
    }

    #[test]
    fn an_explicit_hitl_flag_requires_a_human_regardless_of_kind() {
        let ticket = ticket("T-042", TicketKind::Task, true, 0);
        assert!(requires_human(&ticket));
    }

    #[test]
    fn research_and_task_need_no_human_by_default() {
        assert!(!requires_human(&ticket(
            "T-004",
            TicketKind::Research,
            false,
            0
        )));
        assert!(!requires_human(&ticket(
            "T-020",
            TicketKind::Task,
            false,
            0
        )));
    }

    /// The routing table's whole point: a ticket that needs a person must
    /// not be worked by a model.
    #[test]
    fn a_hitl_ticket_is_never_routed_without_a_human_present() {
        for kind in [
            TicketKind::Research,
            TicketKind::Prototype,
            TicketKind::Grilling,
            TicketKind::Task,
        ] {
            let hitl_ticket = ticket("T-hitl", kind, true, 0);
            let err = route(&hitl_ticket, false)
                .expect_err("a hitl ticket must never route headless, whatever its kind");
            assert_eq!(err.code, ErrCode::HitlRequiresHuman);
        }
    }

    #[test]
    fn a_grilling_ticket_routes_once_a_human_is_present() {
        let ticket = ticket("T-018", TicketKind::Grilling, true, 0);
        let method = route(&ticket, true).expect("a human is present");
        assert_eq!(method, WorkMethod::Grilling);
    }

    #[test]
    fn a_headless_research_or_task_ticket_still_routes() {
        let research = ticket("T-004", TicketKind::Research, false, 0);
        assert_eq!(route(&research, false).unwrap(), WorkMethod::Research);

        let task = ticket("T-020", TicketKind::Task, false, 0);
        assert_eq!(route(&task, false).unwrap(), WorkMethod::Task);
    }

    #[test]
    fn work_method_for_kind_matches_the_routing_table_one_to_one() {
        assert_eq!(
            WorkMethod::for_kind(TicketKind::Research),
            WorkMethod::Research
        );
        assert_eq!(
            WorkMethod::for_kind(TicketKind::Prototype),
            WorkMethod::Prototype
        );
        assert_eq!(
            WorkMethod::for_kind(TicketKind::Grilling),
            WorkMethod::Grilling
        );
        assert_eq!(WorkMethod::for_kind(TicketKind::Task), WorkMethod::Task);
    }

    // -- WorkSession -------------------------------------------------------

    #[test]
    fn a_fresh_session_allows_one_non_research_resolution() {
        let mut session = WorkSession::new();
        session
            .record_resolution(TicketKind::Task)
            .expect("the first resolution is always allowed");
        assert!(session.has_resolved_non_research());
    }

    #[test]
    fn a_second_non_research_resolution_hits_the_session_limit() {
        let mut session = WorkSession::new();
        session.record_resolution(TicketKind::Task).unwrap();

        let err = session
            .record_resolution(TicketKind::Grilling)
            .expect_err("a second non-research resolution must fail");
        assert_eq!(err.code, ErrCode::SessionResolutionLimit);
    }

    #[test]
    fn research_resolutions_are_exempt_from_the_session_limit() {
        let mut session = WorkSession::new();
        for _ in 0..5 {
            session
                .record_resolution(TicketKind::Research)
                .expect("research tickets are exempt from the one-ticket limit");
        }
        assert!(!session.has_resolved_non_research());

        // A research streak never blocks the session's one non-research
        // resolution.
        session
            .record_resolution(TicketKind::Task)
            .expect("research resolutions must not have consumed the limit");
    }

    // -- research_parallelism ----------------------------------------------

    fn residency(budget_bytes: u64, used_bytes: u64) -> ResidencySnapshot {
        ResidencySnapshot {
            budget_bytes,
            used_bytes,
            models: Vec::new(),
        }
    }

    #[test]
    fn ample_headroom_still_caps_at_the_default_limit() {
        // "Do not start eight research sub-agents." Even a headroom that
        // could fit eight must not report more than two.
        let snapshot = residency(64_000_000_000, 1_000_000_000);
        assert_eq!(research_parallelism(&snapshot, 1_000_000_000), 2);
    }

    #[test]
    fn a_synthetic_low_memory_state_reduces_the_limit_below_default() {
        let snapshot = residency(2_000_000_000, 1_500_000_000); // 500 MB headroom
        assert_eq!(research_parallelism(&snapshot, 1_000_000_000), 0);
    }

    #[test]
    fn exactly_one_agent_worth_of_headroom_allows_one() {
        let snapshot = residency(2_000_000_000, 1_000_000_000); // 1 GB headroom
        assert_eq!(research_parallelism(&snapshot, 1_000_000_000), 1);
    }

    #[test]
    fn a_zero_per_agent_estimate_leaves_the_default_unconstrained() {
        let snapshot = residency(0, 0);
        assert_eq!(research_parallelism(&snapshot, 0), DEFAULT_SUBAGENT_LIMIT);
    }

    // -- graduate_fog --------------------------------------------------------

    fn candidate(name: &str, kind: TicketKind) -> Candidate {
        Candidate {
            name: name.to_owned(),
            question: format!("{name}?"),
            axis: "lifecycle, migration and backfill".to_owned(),
            kind,
        }
    }

    #[test]
    fn graduated_tickets_get_fresh_unique_ids_and_sequential_ordinals() {
        let candidates = vec![
            candidate("pack staleness policy", TicketKind::Grilling),
            candidate("staleness backfill", TicketKind::Task),
        ];

        let tickets = graduate_fog(candidates, 5);

        assert_eq!(tickets.len(), 2);
        assert_ne!(tickets[0].id, tickets[1].id);
        assert_eq!(tickets[0].ordinal, 5);
        assert_eq!(tickets[1].ordinal, 6);
        assert!(
            tickets[0].hitl,
            "a graduated grilling ticket defaults to hitl"
        );
        assert!(
            !tickets[1].hitl,
            "a graduated task ticket defaults to no hitl"
        );
    }

    // -- close_out_of_scope --------------------------------------------------

    #[test]
    fn close_out_of_scope_always_names_the_ticket_that_raised_it() {
        let exclusion = close_out_of_scope("T-018", "pack signing", "separate effort");
        assert_eq!(exclusion.ticket_id.as_deref(), Some("T-018"));
        assert_eq!(exclusion.gist, "pack signing");
        assert_eq!(exclusion.reason, "separate effort");
    }
}
