//! Renders an [`ExportSnapshot`] as a Mermaid flowchart.

use std::fmt::Write as _;

use crate::journal::TicketStatus;

use super::query::ExportSnapshot;

/// Renders `snapshot` as a Mermaid `flowchart TD`: one node per ticket,
/// one arrow per blocking edge, and a `classDef` per status so a viewer
/// sees the frontier and the closed tickets at a glance.
///
/// A pure function of `snapshot`: the same snapshot always renders the
/// same bytes. See the module documentation on [`super`].
pub(super) fn render(snapshot: &ExportSnapshot) -> String {
    let mut out = String::from("flowchart TD\n");

    for ticket in &snapshot.tickets {
        let node = node_id(&ticket.id);
        let label = escape_label(&format!("{}: {}", ticket.id, ticket.name));
        let class = status_class(ticket.status);
        let _ = writeln!(out, "    {node}[\"{label}\"]:::{class}");
    }

    for (blocker, blocked) in &snapshot.edges {
        let _ = writeln!(out, "    {} --> {}", node_id(blocker), node_id(blocked));
    }

    out.push('\n');
    out.push_str("    classDef open fill:#7DD3FC,stroke:#0369A1,color:#0C4A6E\n");
    out.push_str("    classDef claimed fill:#FDE68A,stroke:#B45309,color:#78350F\n");
    out.push_str("    classDef resolved fill:#86EFAC,stroke:#15803D,color:#14532D\n");
    out.push_str("    classDef out_of_scope fill:#E5E7EB,stroke:#6B7280,color:#374151\n");
    out.push_str("    classDef invalidated fill:#E5E7EB,stroke:#6B7280,color:#374151\n");

    out
}

/// Returns the Mermaid node identifier for `ticket_id`.
///
/// Mermaid node identifiers cannot safely carry every character a ticket
/// identifier might: this replaces anything but an ASCII letter, digit,
/// or underscore with `_`, and prefixes the result with `n` so an
/// identifier that starts with a digit still parses as a node name, not
/// a number.
fn node_id(ticket_id: &str) -> String {
    let sanitised: String = ticket_id
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    format!("n{sanitised}")
}

/// Escapes a label for Mermaid's `["..."]` node syntax: a double quote
/// would close the label early, and a newline would break the line the
/// node declaration sits on.
fn escape_label(label: &str) -> String {
    label.replace('"', "'").replace('\n', " ")
}

/// Returns the `classDef` name for `status`.
fn status_class(status: TicketStatus) -> &'static str {
    status.as_str()
}
