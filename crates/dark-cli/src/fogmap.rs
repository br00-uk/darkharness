//! Turns a stored map into the fog map the shell draws.
//!
//! This join lives in the composition root for the same reason
//! `crate::scrape` and `crate::pack` do: it is the only crate that sees
//! both sides. `dark-cartograph` owns the tickets and knows nothing about
//! a terminal; `dark-tui` draws the map and depends on `dark-contract`
//! alone (Rule 14), so it cannot open a store. Neither can be given the
//! other's type, and the conversion has to happen somewhere.
//!
//! Every read here is local: an `SQLite` file under the repository root.
//!
//! # What `Fog` means here
//!
//! [`dark_tui::theme::TicketState`] has six states and
//! [`dark_cartograph::journal::TicketStatus`] has five, and they are not
//! the same five. The map's `Blocked` and `Frontier` are one status —
//! `Open` — split by whether anything still blocks the ticket, which is
//! why this needs the blocking edges and not only the rows. `Fog` has no
//! status at all: it is the map's word for a question not yet turned into
//! a ticket, which the store records separately, so nothing here ever
//! produces it.

use dark_cartograph::journal::TicketStatus;
use dark_cartograph::snapshot::{MapTicket, map_destination, map_ids, map_snapshot};
use dark_cartograph::store::Store;
use dark_tui::theme::TicketState;
use dark_tui::views::fogmap::{FogMapData, Layout, Ticket, compute_layout};

use std::collections::BTreeSet;
use std::path::Path;

/// Returns the state a ticket should draw in.
///
/// `unresolved` holds the identifiers of every ticket that has not
/// finished: an open ticket blocked by one of them is `Blocked`, otherwise
/// it is on the frontier. That is the same rule
/// [`dark_cartograph::frontier::frontier`] applies when it decides what is
/// takeable, restated here because this needs the answer for every ticket
/// rather than only the takeable ones.
fn state_of(ticket: &MapTicket, unresolved: &BTreeSet<&str>) -> TicketState {
    match ticket.status {
        TicketStatus::Claimed => TicketState::Claimed,
        TicketStatus::Resolved => TicketState::Resolved,
        // A map excludes an out-of-scope ticket, and an invalidated one is
        // a decision the map has moved past. Both sit outside the disk.
        TicketStatus::OutOfScope | TicketStatus::Invalidated => TicketState::OutOfScope,
        TicketStatus::Open => {
            if ticket
                .blocked_by
                .iter()
                .any(|blocker| unresolved.contains(blocker.as_str()))
            {
                TicketState::Blocked
            } else {
                TicketState::Frontier
            }
        }
    }
}

/// Converts a stored map into the shape the fog map draws.
///
/// `destination` names the ticket at the centre. A destination naming no
/// ticket in `tickets` leaves the map centred on nothing, which
/// [`compute_layout`] handles by placing every ticket in the fog ring —
/// an honest drawing of "this map has no destination in it" rather than a
/// panic or an empty pane.
#[must_use]
pub(crate) fn to_map_data(tickets: &[MapTicket], destination: &str) -> FogMapData {
    let unresolved: BTreeSet<&str> = tickets
        .iter()
        .filter(|t| {
            !matches!(
                t.status,
                TicketStatus::Resolved | TicketStatus::OutOfScope | TicketStatus::Invalidated
            )
        })
        .map(|t| t.id.as_str())
        .collect();

    FogMapData {
        destination: destination.to_owned(),
        tickets: tickets
            .iter()
            .map(|ticket| Ticket {
                id: ticket.id.clone(),
                name: ticket.name.clone(),
                state: state_of(ticket, &unresolved),
                blocked_by: ticket.blocked_by.clone(),
            })
            .collect(),
    }
}

/// Loads one map from the repository's store and lays it out.
///
/// Returns `None` when the store cannot be opened, the map is not in it,
/// or it holds no tickets. Each of those is "there is no map to draw",
/// which the pane already has a sentence for; none is an error worth
/// interrupting a session over.
#[must_use]
pub(crate) fn load(repo_root: &Path, map_id: &str) -> Option<Layout> {
    let store = Store::open(repo_root).ok()?;
    let destination = map_destination(&store, map_id).ok().flatten()?;
    let tickets = map_snapshot(&store, map_id).ok()?;
    if tickets.is_empty() {
        return None;
    }
    Some(compute_layout(&to_map_data(&tickets, &destination)))
}

/// Loads the repository's map when it holds exactly one, for a session
/// that has not been told which map to draw.
///
/// Returns `None` when the store holds no map or more than one: with two
/// maps there is no answer to "the map", and drawing an arbitrary one is
/// worse than drawing none and saying so.
#[must_use]
pub(crate) fn sole_map(repo_root: &Path) -> Option<Layout> {
    let store = Store::open(repo_root).ok()?;
    let ids = map_ids(&store).ok()?;
    let [only] = &ids[..] else {
        return None;
    };
    load(repo_root, only)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ticket(id: &str, status: TicketStatus, blocked_by: &[&str]) -> MapTicket {
        MapTicket {
            id: id.to_owned(),
            name: format!("ticket {id}"),
            status,
            blocked_by: blocked_by.iter().map(|s| (*s).to_owned()).collect(),
        }
    }

    fn state_for(data: &FogMapData, id: &str) -> TicketState {
        data.tickets
            .iter()
            .find(|t| t.id == id)
            .unwrap_or_else(|| panic!("no ticket {id}"))
            .state
    }

    #[test]
    fn an_open_ticket_with_no_blocker_is_on_the_frontier() {
        let data = to_map_data(&[ticket("T1", TicketStatus::Open, &[])], "T1");
        assert_eq!(state_for(&data, "T1"), TicketState::Frontier);
    }

    #[test]
    fn an_open_ticket_behind_an_unresolved_blocker_is_blocked() {
        let tickets = [
            ticket("T1", TicketStatus::Open, &[]),
            ticket("T2", TicketStatus::Open, &["T1"]),
        ];
        let data = to_map_data(&tickets, "T2");
        assert_eq!(state_for(&data, "T2"), TicketState::Blocked);
    }

    #[test]
    fn a_resolved_blocker_stops_blocking() {
        // This is the whole point of the distinction: the same ticket, the
        // same edge, and the state turns over when its blocker resolves.
        let tickets = [
            ticket("T1", TicketStatus::Resolved, &[]),
            ticket("T2", TicketStatus::Open, &["T1"]),
        ];
        let data = to_map_data(&tickets, "T2");
        assert_eq!(state_for(&data, "T2"), TicketState::Frontier);
    }

    #[test]
    fn an_out_of_scope_blocker_stops_blocking_too() {
        let tickets = [
            ticket("T1", TicketStatus::OutOfScope, &[]),
            ticket("T2", TicketStatus::Open, &["T1"]),
        ];
        assert_eq!(
            state_for(&to_map_data(&tickets, "T2"), "T2"),
            TicketState::Frontier
        );
    }

    #[test]
    fn each_remaining_status_maps_to_its_own_state() {
        let tickets = [
            ticket("T1", TicketStatus::Claimed, &[]),
            ticket("T2", TicketStatus::Resolved, &[]),
            ticket("T3", TicketStatus::OutOfScope, &[]),
            ticket("T4", TicketStatus::Invalidated, &[]),
        ];
        let data = to_map_data(&tickets, "T1");
        assert_eq!(state_for(&data, "T1"), TicketState::Claimed);
        assert_eq!(state_for(&data, "T2"), TicketState::Resolved);
        assert_eq!(state_for(&data, "T3"), TicketState::OutOfScope);
        assert_eq!(state_for(&data, "T4"), TicketState::OutOfScope);
    }

    #[test]
    fn the_destination_is_carried_through() {
        let data = to_map_data(&[ticket("T1", TicketStatus::Open, &[])], "T1");
        assert_eq!(data.destination, "T1");
    }

    #[test]
    fn a_map_laid_out_twice_is_identical() {
        // The map pane redraws every frame; a layout that moved between
        // two identical snapshots would shimmer.
        let tickets = [
            ticket("T1", TicketStatus::Open, &[]),
            ticket("T2", TicketStatus::Open, &["T1"]),
            ticket("T3", TicketStatus::Claimed, &["T2"]),
        ];
        let data = to_map_data(&tickets, "T3");
        assert_eq!(compute_layout(&data), compute_layout(&data));
    }

    #[test]
    fn loading_a_map_from_a_directory_with_no_store_is_none() {
        let dir = tempfile::tempdir().unwrap();
        assert!(load(dir.path(), "M1").is_none());
    }
}
