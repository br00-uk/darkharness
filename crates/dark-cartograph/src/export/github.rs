//! Builds a plan for exporting a map to GitHub issues.
//!
//! This module never talks to GitHub. Rule 13 (`CLAUDE.md`) lets only
//! `dark-airlock` construct an HTTP client, and this crate's own
//! dependency allowlist (see `crate`'s module documentation and task
//! unit `D5`'s report) names no HTTP crate at all — `cargo-deny` would
//! catch one arriving even by accident. [`Plan`] is therefore inert data:
//! a parent issue, one child issue per ticket, and the blocking relation
//! task unit `D4`'s step 2 gives as "native blocking relations". A layer
//! that does hold an `dark-airlock` client — outside this task unit —
//! creates the parent, creates each child, reads back the issue numbers
//! GitHub assigns, and only then can call the "blocked by" API, because
//! that API needs numbers this plan cannot know yet.

use dark_contract::ErrCode;
use serde::Serialize;

use crate::journal::TicketStatus;

use super::query::ExportSnapshot;

/// The label every parent (map) issue in a `dark map export --format=github`
/// plan carries. Task unit `D4`, step 2.
pub const MAP_LABEL: &str = "wayfinder:map";

/// The label every child (ticket) issue carries.
pub const TICKET_LABEL: &str = "wayfinder:ticket";

/// One issue this plan would create.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct IssueDraft {
    /// The issue title.
    pub title: String,
    /// The issue body, in Markdown.
    pub body: String,
    /// The labels to apply.
    pub labels: Vec<String>,
}

/// One ticket's issue, plus what this plan cannot resolve until GitHub
/// has assigned every issue a number.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TicketIssue {
    /// The ticket this issue represents.
    pub ticket_id: String,
    /// The issue to create.
    pub issue: IssueDraft,
    /// The ticket identifiers that must become "blocked by" relations on
    /// this issue, once every child issue in [`Plan::children`] has a
    /// number.
    pub blocked_by: Vec<String>,
    /// `true` when the executing layer should close this issue right
    /// after creating it — the ticket already resolved, left scope, or
    /// was invalidated before this export ran.
    pub close: bool,
}

/// A complete GitHub export plan for one map: one parent issue, and one
/// child issue for every ticket. Export is one way in version 1 (task
/// unit `D5`, step 3): nothing here reads GitHub state back into the
/// map, so re-running an export creates a second, independent set of
/// issues rather than updating the first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Plan {
    /// The issue that represents the map as a whole.
    pub parent: IssueDraft,
    /// One issue for every ticket, in the same order
    /// [`ExportSnapshot::tickets`] holds them.
    pub children: Vec<TicketIssue>,
}

/// Builds the export plan for `snapshot`.
///
/// A pure function of `snapshot`: the same snapshot always builds the
/// same plan. See the module documentation on [`super`].
pub(super) fn build(snapshot: &ExportSnapshot) -> Plan {
    let parent = IssueDraft {
        title: snapshot.name.clone(),
        body: parent_body(snapshot),
        labels: vec![MAP_LABEL.to_owned()],
    };

    let children = snapshot
        .tickets
        .iter()
        .map(|ticket| TicketIssue {
            ticket_id: ticket.id.clone(),
            issue: IssueDraft {
                title: format!("{}: {}", ticket.id, ticket.name),
                body: ticket_body(ticket),
                labels: vec![
                    TICKET_LABEL.to_owned(),
                    format!("wayfinder:type:{}", ticket.ticket_type.as_str()),
                ],
            },
            blocked_by: blockers_of(snapshot, &ticket.id),
            close: matches!(
                ticket.status,
                TicketStatus::Resolved | TicketStatus::OutOfScope | TicketStatus::Invalidated
            ),
        })
        .collect();

    Plan { parent, children }
}

/// Renders `plan` as stable, deterministic JSON.
///
/// # Errors
///
/// Returns an error only when serialisation itself fails, which
/// [`Plan`]'s all-owned, non-cyclic shape never triggers in practice;
/// the `Result` exists because `serde_json::to_string_pretty` returns
/// one.
pub(super) fn render(plan: &Plan) -> dark_contract::Result<String> {
    serde_json::to_string_pretty(plan).map_err(|err| {
        dark_contract::Error::new(
            ErrCode::ToolFailed,
            format!("cannot render the GitHub export plan: {err}"),
        )
    })
}

/// Builds the parent issue's body.
fn parent_body(snapshot: &ExportSnapshot) -> String {
    let mut body = format!(
        "{}\n\n**Status:** {}\n",
        snapshot.destination,
        snapshot.status.as_str()
    );
    if let Some(notes) = &snapshot.notes {
        body.push_str("\n**Notes:** ");
        body.push_str(notes);
        body.push('\n');
    }
    body
}

/// Builds one ticket's issue body.
fn ticket_body(ticket: &super::query::ExportTicket) -> String {
    let mut body = ticket.question.clone();
    if let Some(resolution) = &ticket.resolution {
        body.push_str("\n\n**Resolution:** ");
        body.push_str(resolution);
    }
    if let Some(axis) = &ticket.axis {
        body.push_str("\n\n**Axis:** ");
        body.push_str(axis);
    }
    body
}

/// Returns the ticket identifiers that block `ticket_id`, in the order
/// `snapshot.edges` already holds them.
fn blockers_of(snapshot: &ExportSnapshot, ticket_id: &str) -> Vec<String> {
    snapshot
        .edges
        .iter()
        .filter(|(_, blocked)| blocked == ticket_id)
        .map(|(blocker, _)| blocker.clone())
        .collect()
}
