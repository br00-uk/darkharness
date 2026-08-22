//! The context prefix: assembly, and change detection.
//!
//! Rule 5 says the harness assembles the prefix at the start of a turn and
//! does not change it during the turn. This module builds the prefix
//! ([`assemble_prefix`]) and detects when the assembled prefix differs from
//! the one the previous turn used ([`PrefixTracker`]), so the caller can emit
//! a notice that names the cause before the engine pays for a full prefill.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use dark_contract::{EventTx, Message, Role};

use super::budget::Part;

/// The five sections that make up the prefix, in build order.
///
/// This list is [`Part::is_prefix`] restricted to the parts that always
/// carry text, in the same fixed order the build specification names.
pub const PREFIX_PARTS: [Part; 5] = [
    Part::SystemPrompt,
    Part::AgentsChain,
    Part::Environment,
    Part::MapDigest,
    Part::Ticket,
];

/// The text a caller supplies for each prefix section.
///
/// `dark-core` does not depend on `dark-agentsmd` or `dark-cartograph` (see
/// `CLAUDE.md`), so this module never renders the `AGENTS.md` chain or the
/// map digest itself. The caller renders that text and passes it in as a
/// borrowed string, which keeps this module a pure assembler.
#[derive(Debug, Clone, Copy)]
pub struct PrefixInputs<'a> {
    /// The system prompt, verbatim.
    pub system_prompt: &'a str,
    /// The rendered `AGENTS.md` chain, nearest file last, verbatim.
    pub agents_chain: &'a str,
    /// The date, in `YYYY-MM-DD` form.
    ///
    /// Rule 6 says the prefix carries the date and never the time. Passing a
    /// date-only string in, rather than reading a clock in this module,
    /// keeps that rule true by construction: this module never reads a
    /// clock at all.
    pub environment_date: &'a str,
    /// The rendered map digest, when a map is loaded.
    pub map_digest: Option<&'a str>,
    /// The claimed ticket body, when a ticket is claimed.
    pub ticket_body: Option<&'a str>,
}

/// Formats the environment block from a date and nothing else.
///
/// Building the block here, from a date-only string, is what keeps Rule 6
/// true: nothing upstream of this function can smuggle a time in through the
/// `environment_date` field, because the field is a plain date and this
/// function reads only that field.
fn format_environment_block(date: &str) -> String {
    format!("Today's date is {date}.")
}

/// One section of the assembled prefix.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrefixSection {
    /// Which of the five parts this section fills.
    pub part: Part,
    /// The section content, rendered to a single [`Message`].
    pub message: Message,
}

/// The assembled prefix: the sections that are present, in build order.
///
/// A caller with no map loaded and no claimed ticket sees three sections,
/// not five. Absent sections never appear; this module never renders an
/// empty placeholder for them.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AssembledPrefix {
    /// The present sections, in the order [`PREFIX_PARTS`] names.
    pub sections: Vec<PrefixSection>,
}

impl AssembledPrefix {
    /// Returns the prefix as one pinned [`Message`] per present section.
    ///
    /// A turn loop appends the tail messages after these. See Rule 8.
    pub fn messages(&self) -> Vec<Message> {
        self.sections
            .iter()
            .map(|section| section.message.clone())
            .collect()
    }
}

/// Builds the prefix in the fixed order the build specification names.
///
/// This function is pure: the same `inputs` always produce byte-identical
/// output. Call it once at the start of a turn and reuse the result; do not
/// call it again mid-turn, or a different set of inputs could change the
/// prefix and force a full prefill (Rule 5).
pub fn assemble_prefix(inputs: &PrefixInputs<'_>) -> AssembledPrefix {
    let mut sections = Vec::with_capacity(5);

    sections.push(PrefixSection {
        part: Part::SystemPrompt,
        message: Message::text(Role::System, inputs.system_prompt).pinned(),
    });
    sections.push(PrefixSection {
        part: Part::AgentsChain,
        message: Message::text(Role::System, inputs.agents_chain).pinned(),
    });
    sections.push(PrefixSection {
        part: Part::Environment,
        message: Message::text(
            Role::System,
            format_environment_block(inputs.environment_date),
        )
        .pinned(),
    });
    if let Some(digest) = inputs.map_digest {
        sections.push(PrefixSection {
            part: Part::MapDigest,
            message: Message::text(Role::System, digest).pinned(),
        });
    }
    if let Some(ticket) = inputs.ticket_body {
        sections.push(PrefixSection {
            part: Part::Ticket,
            message: Message::text(Role::System, ticket).pinned(),
        });
    }

    AssembledPrefix { sections }
}

/// A hash of the text in each present prefix section.
///
/// Hashing `None` and hashing `Some("")` produce different values, so a map
/// that unloads to empty text is still a detectable change from a map that
/// was never loaded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrefixHash {
    /// One hash per part named in [`PREFIX_PARTS`], in that order.
    sections: [u64; 5],
}

fn hash_text(text: Option<&str>) -> u64 {
    let mut hasher = DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
}

impl PrefixHash {
    /// Computes the hash for `inputs`.
    ///
    /// This reads only the five section fields of `inputs`. It never reads a
    /// clock, so computing the hash cannot itself change the prefix.
    pub fn compute(inputs: &PrefixInputs<'_>) -> Self {
        Self {
            sections: [
                hash_text(Some(inputs.system_prompt)),
                hash_text(Some(inputs.agents_chain)),
                hash_text(Some(inputs.environment_date)),
                hash_text(inputs.map_digest),
                hash_text(inputs.ticket_body),
            ],
        }
    }

    /// Combines the five section hashes into one value.
    ///
    /// `crates/dark-core/src/session/mod.rs` stores exactly this shape
    /// (`Session::prefix_hash` is a `u64`, set once per turn through
    /// `Session::set_prefix_hash`). Use this method to produce the value
    /// that call expects; keep comparing sections with
    /// [`PrefixHash::changed_since`] when the goal is naming the cause of a
    /// change, since a combined `u64` alone cannot do that.
    pub fn combined(&self) -> u64 {
        let mut hasher = DefaultHasher::new();
        self.sections.hash(&mut hasher);
        hasher.finish()
    }

    /// Returns the parts whose text differs between `self` and `other`, in
    /// build order.
    ///
    /// An empty result means the two hashes cover byte-identical prefixes.
    pub fn changed_since(&self, other: &Self) -> Vec<Part> {
        PREFIX_PARTS
            .iter()
            .copied()
            .zip(self.sections)
            .zip(other.sections)
            .filter_map(|((part, mine), theirs)| (mine != theirs).then_some(part))
            .collect()
    }
}

/// Tracks the prefix hash across turns and names the cause when it changes.
///
/// A turn loop holds one `PrefixTracker` for the life of a session and calls
/// [`PrefixTracker::observe`] once at the start of every turn (Do step 3).
#[derive(Debug, Default)]
pub struct PrefixTracker {
    previous: Option<PrefixHash>,
}

impl PrefixTracker {
    /// Creates a tracker with no prior turn to compare against.
    pub fn new() -> Self {
        Self::default()
    }

    /// Computes the hash for `inputs` and compares it against the hash the
    /// previous call observed.
    ///
    /// The first call on a fresh tracker never emits a notice: there is no
    /// previous turn to have changed from. Every later call emits one
    /// [`dark_contract::Event::Notice`] on `events`, naming every part that
    /// changed, whenever the prefix differs from the previous turn's.
    pub fn observe(&mut self, inputs: &PrefixInputs<'_>, events: &EventTx) -> PrefixHash {
        let hash = PrefixHash::compute(inputs);

        if let Some(previous) = &self.previous {
            let changed = hash.changed_since(previous);
            if !changed.is_empty() {
                let names = changed
                    .iter()
                    .copied()
                    .map(Part::as_str)
                    .collect::<Vec<_>>()
                    .join(", ");
                events.notice(format!(
                    "the context prefix changed ({names}); this turn pays for a full prefill"
                ));
            }
        }

        self.previous = Some(hash.clone());
        hash
    }
}

#[cfg(test)]
mod tests {
    use dark_contract::{EventBus, Received};

    use super::*;

    fn inputs<'a>(map_digest: Option<&'a str>, ticket: Option<&'a str>) -> PrefixInputs<'a> {
        PrefixInputs {
            system_prompt: "you are dark",
            agents_chain: "root rules",
            environment_date: "2026-08-22",
            map_digest,
            ticket_body: ticket,
        }
    }

    #[test]
    fn assemble_prefix_builds_sections_in_order() {
        let assembled = assemble_prefix(&inputs(Some("digest text"), Some("ticket text")));
        let parts: Vec<Part> = assembled.sections.iter().map(|s| s.part).collect();
        assert_eq!(parts, PREFIX_PARTS.to_vec());
    }

    #[test]
    fn assemble_prefix_skips_absent_sections() {
        let assembled = assemble_prefix(&inputs(None, None));
        let parts: Vec<Part> = assembled.sections.iter().map(|s| s.part).collect();
        assert_eq!(
            parts,
            vec![Part::SystemPrompt, Part::AgentsChain, Part::Environment]
        );
    }

    #[test]
    fn every_prefix_message_is_pinned() {
        let assembled = assemble_prefix(&inputs(Some("d"), Some("t")));
        for section in &assembled.sections {
            assert!(section.message.pinned, "{:?} was not pinned", section.part);
        }
    }

    #[test]
    fn the_environment_block_carries_no_time() {
        let assembled = assemble_prefix(&inputs(None, None));
        let env = &assembled.sections[2].message;
        let text = env.text_content();
        assert!(text.contains("2026-08-22"));
        // A time carries a colon between digits. Guard against a caller (or
        // a future edit) sneaking a clock into the block. See Rule 6.
        assert!(
            !text.contains(':'),
            "the environment block must never carry a time: {text:?}"
        );
    }

    #[test]
    fn assemble_prefix_is_pure() {
        let a = assemble_prefix(&inputs(Some("d"), Some("t")));
        let b = assemble_prefix(&inputs(Some("d"), Some("t")));
        assert_eq!(a, b);
    }

    #[test]
    fn identical_inputs_hash_identically() {
        let hash_a = PrefixHash::compute(&inputs(Some("d"), Some("t")));
        let hash_b = PrefixHash::compute(&inputs(Some("d"), Some("t")));
        assert_eq!(hash_a, hash_b);
        assert!(hash_a.changed_since(&hash_b).is_empty());
        assert_eq!(hash_a.combined(), hash_b.combined());
    }

    #[test]
    fn a_changed_section_changes_the_combined_hash() {
        let before = PrefixHash::compute(&inputs(Some("old"), Some("t")));
        let after = PrefixHash::compute(&inputs(Some("new"), Some("t")));
        assert_ne!(before.combined(), after.combined());
    }

    #[test]
    fn a_changed_digest_is_named_as_the_cause() {
        let before = PrefixHash::compute(&inputs(Some("old digest"), Some("t")));
        let after = PrefixHash::compute(&inputs(Some("new digest"), Some("t")));
        assert_eq!(after.changed_since(&before), vec![Part::MapDigest]);
    }

    #[test]
    fn loading_a_digest_that_was_absent_counts_as_a_change() {
        let before = PrefixHash::compute(&inputs(None, Some("t")));
        let after = PrefixHash::compute(&inputs(Some("digest"), Some("t")));
        assert_eq!(after.changed_since(&before), vec![Part::MapDigest]);
    }

    #[test]
    fn two_sections_changing_at_once_names_both() {
        let before = PrefixHash::compute(&inputs(Some("old"), Some("old ticket")));
        let after = PrefixHash::compute(&inputs(Some("new"), Some("new ticket")));
        assert_eq!(
            after.changed_since(&before),
            vec![Part::MapDigest, Part::Ticket]
        );
    }

    /// Waits briefly for an event and returns `None` when none arrives.
    ///
    /// The bus never closes in these tests, so [`dark_contract::EventRx::recv`]
    /// would hang forever on an empty channel; a short timeout is the only
    /// way to assert "no notice fired".
    async fn recv_soon(rx: &mut dark_contract::EventRx) -> Option<Received> {
        tokio::time::timeout(std::time::Duration::from_millis(50), rx.recv())
            .await
            .unwrap_or(None)
    }

    #[tokio::test]
    async fn tracker_emits_no_notice_on_the_first_turn() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        let mut tracker = PrefixTracker::new();

        tracker.observe(&inputs(Some("d"), Some("t")), &bus.tx());

        assert!(recv_soon(&mut rx).await.is_none());
    }

    #[tokio::test]
    async fn tracker_stays_silent_when_the_prefix_does_not_change() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        let mut tracker = PrefixTracker::new();

        tracker.observe(&inputs(Some("d"), Some("t")), &bus.tx());
        let _ = recv_soon(&mut rx).await; // drain the (absent) first-turn notice
        tracker.observe(&inputs(Some("d"), Some("t")), &bus.tx());

        assert!(recv_soon(&mut rx).await.is_none());
    }

    #[tokio::test]
    async fn tracker_names_the_cause_when_the_ticket_changes() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        let mut tracker = PrefixTracker::new();

        tracker.observe(&inputs(Some("d"), Some("first ticket")), &bus.tx());
        tracker.observe(&inputs(Some("d"), Some("second ticket")), &bus.tx());

        let Some(Received::Event(dark_contract::Event::Notice(text))) = recv_soon(&mut rx).await
        else {
            panic!("expected a Notice event");
        };
        assert!(
            text.contains("claimed ticket body"),
            "notice did not name the cause: {text}"
        );
        assert!(
            !text.contains("map digest"),
            "notice named a part that did not change: {text}"
        );
    }
}
