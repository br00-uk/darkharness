//! Renders an [`ExportSnapshot`] as a Markdown document.

use std::fmt::Write as _;

use crate::journal::TicketStatus;

use super::query::ExportSnapshot;

/// Renders `snapshot` as Markdown: a title, the destination, notes, one
/// checklist item per ticket with its blockers, fog, and scope
/// exclusions.
///
/// A pure function of `snapshot`: the same snapshot always renders the
/// same bytes. See the module documentation on [`super`].
pub(super) fn render(snapshot: &ExportSnapshot) -> String {
    let mut out = String::new();

    let _ = writeln!(out, "# {}\n", snapshot.name);
    let _ = writeln!(out, "**Map:** {}\n", snapshot.map_id);
    let _ = writeln!(out, "**Status:** {}\n", snapshot.status.as_str());
    let _ = writeln!(out, "## Destination\n");
    let _ = writeln!(out, "{}\n", snapshot.destination);

    if let Some(notes) = &snapshot.notes {
        let _ = writeln!(out, "## Notes\n");
        let _ = writeln!(out, "{notes}\n");
    }

    let _ = writeln!(out, "## Tickets\n");
    if snapshot.tickets.is_empty() {
        let _ = writeln!(out, "(none)\n");
    }
    for ticket in &snapshot.tickets {
        let checked = if matches!(ticket.status, TicketStatus::Resolved) {
            'x'
        } else {
            ' '
        };
        let _ = writeln!(
            out,
            "- [{checked}] **{}** {} ({}, {})",
            ticket.id,
            ticket.name,
            ticket.ticket_type.as_str(),
            ticket.status.as_str(),
        );

        let blockers = blockers_of(snapshot, &ticket.id);
        if blockers.is_empty() {
            let _ = writeln!(out, "  - Blocked by: none");
        } else {
            let _ = writeln!(out, "  - Blocked by: {}", blockers.join(", "));
        }

        if let Some(gist) = &ticket.gist {
            let _ = writeln!(out, "  - Resolution: {gist}");
        }
    }
    out.push('\n');

    if !snapshot.fog.is_empty() {
        let _ = writeln!(out, "## Fog\n");
        for fog in &snapshot.fog {
            match &fog.axis {
                Some(axis) => {
                    let _ = writeln!(out, "- {} (axis: {axis})", fog.patch);
                }
                None => {
                    let _ = writeln!(out, "- {}", fog.patch);
                }
            }
        }
        out.push('\n');
    }

    if !snapshot.scope_exclusions.is_empty() {
        let _ = writeln!(out, "## Out of scope\n");
        for exclusion in &snapshot.scope_exclusions {
            match &exclusion.ticket_id {
                Some(ticket_id) => {
                    let _ = writeln!(
                        out,
                        "- {} — {} ({ticket_id})",
                        exclusion.gist, exclusion.reason
                    );
                }
                None => {
                    let _ = writeln!(out, "- {} — {}", exclusion.gist, exclusion.reason);
                }
            }
        }
    }

    out
}

/// Returns the identifiers that block `ticket_id`, in the order
/// `snapshot.edges` already holds them.
fn blockers_of<'a>(snapshot: &'a ExportSnapshot, ticket_id: &str) -> Vec<&'a str> {
    snapshot
        .edges
        .iter()
        .filter(|(_, blocked)| blocked == ticket_id)
        .map(|(blocker, _)| blocker.as_str())
        .collect()
}
