//! Compaction: folding old history into one summary message.
//!
//! Rule 7 says the harness compacts only at a turn boundary, never mid-turn.
//! This module gives a turn loop the three pieces it needs to honour that
//! rule: a threshold check ([`should_compact`]), a pure selection of which
//! messages to fold ([`select_fold_range`]), and a pure replacement of that
//! selection with one summary message ([`apply_summary`]).
//!
//! This module never calls [`dark_contract::Engine::stream`] itself. Do step
//! 5 says compaction uses the scout micro-role, so [`build_summary_request`]
//! builds the [`dark_contract::Request`] that asks a scout model for the
//! summary text; the turn loop sends it and passes the resulting text to
//! [`apply_summary`]. Keeping the send outside this module keeps `context/`
//! free of a streaming dependency it does not otherwise need.

use dark_contract::{EventTx, Message, Request, Role, RoleClass};

/// The scout instructions for a compaction summary.
///
/// ASD-STE100: one instruction a sentence, and the same word for the same
/// thing every time the build specification names it (files, decisions,
/// errors, work).
const SCOUT_INSTRUCTIONS: &str = "Summarise the messages that follow into one paragraph. \
Keep the files that the session changed. \
Keep the decisions that the session made. \
Keep the errors that the session met. \
Keep the work that remains. \
Drop everything else.";

/// Returns the token count at which the harness must compact.
///
/// Do step 4: compact at 75% of `granted_context`. Budget against
/// [`dark_contract::Caps::granted_context`], never
/// [`dark_contract::Caps::max_context`] (Rule 4).
pub fn compaction_threshold(granted_context: usize) -> usize {
    granted_context * 3 / 4
}

/// Returns `true` when `used_tokens` has reached the compaction threshold.
///
/// Call this only at a turn boundary (Rule 7). A `true` result means the
/// turn loop must fold history before it starts the next turn, not during
/// the turn that is already running.
pub fn should_compact(used_tokens: usize, granted_context: usize) -> bool {
    used_tokens >= compaction_threshold(granted_context)
}

/// Which messages in a history slice compaction will fold.
///
/// The indices are positions into the `history` slice that produced them,
/// sorted ascending. Every index names an unpinned message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FoldSelection {
    /// The selected indices, oldest first.
    indices: Vec<usize>,
}

impl FoldSelection {
    /// Returns the selected indices, oldest first.
    pub fn indices(&self) -> &[usize] {
        &self.indices
    }
}

/// Selects the oldest third of the unpinned messages in `history` to fold.
///
/// Never selects a message where [`dark_contract::Message::pinned`] is
/// `true` (Do step 5, "never compact pinned messages"). Returns `None` when
/// `history` holds fewer than three unpinned messages: folding one or two
/// messages into "a summary" buys nothing worth the model call.
pub fn select_fold_range(history: &[Message]) -> Option<FoldSelection> {
    let unpinned: Vec<usize> = history
        .iter()
        .enumerate()
        .filter(|(_, message)| !message.pinned)
        .map(|(index, _)| index)
        .collect();

    if unpinned.len() < 3 {
        return None;
    }

    let fold_count = unpinned.len() / 3;
    Some(FoldSelection {
        indices: unpinned[..fold_count].to_vec(),
    })
}

/// Builds the [`dark_contract::Request`] that asks the scout role to
/// summarise the selected messages.
///
/// The turn loop sends this request and passes the model's reply text to
/// [`apply_summary`]. This function never sends it: see the module
/// documentation for why.
pub fn build_summary_request(history: &[Message], selection: &FoldSelection) -> Request {
    let mut messages = vec![Message::text(Role::System, SCOUT_INSTRUCTIONS)];
    messages.extend(
        selection
            .indices
            .iter()
            .map(|&index| history[index].clone()),
    );
    Request::new(RoleClass::Scout, messages)
}

/// Replaces the selected messages with one summary message.
///
/// Every message outside `selection` keeps its original position relative
/// to the others. The summary message takes the position of the earliest
/// selected message. `events` receives one
/// [`dark_contract::Event::Notice`] naming how many messages folded:
/// Do step 6 says silent compaction destroys trust.
pub fn apply_summary(
    history: &[Message],
    selection: &FoldSelection,
    summary_text: &str,
    events: &EventTx,
) -> Vec<Message> {
    let fold_set: std::collections::HashSet<usize> = selection.indices.iter().copied().collect();
    let mut result = Vec::with_capacity(history.len() - fold_set.len() + 1);
    let mut folded_in = false;

    for (index, message) in history.iter().enumerate() {
        if fold_set.contains(&index) {
            if !folded_in {
                result.push(Message::text(Role::System, summary_text));
                folded_in = true;
            }
            continue;
        }
        result.push(message.clone());
    }

    events.notice(format!(
        "the harness folded {} message(s) into one summary to stay inside the context budget",
        fold_set.len()
    ));

    result
}

#[cfg(test)]
mod tests {
    use dark_contract::{Event, EventBus, Received};

    use super::*;

    fn pinned(text: &str) -> Message {
        Message::text(Role::User, text).pinned()
    }

    fn unpinned(text: &str) -> Message {
        Message::text(Role::User, text)
    }

    #[test]
    fn compaction_threshold_is_75_percent() {
        assert_eq!(compaction_threshold(32_000), 24_000);
        assert_eq!(compaction_threshold(1000), 750);
    }

    #[test]
    fn should_compact_fires_at_the_threshold_and_not_before() {
        assert!(!should_compact(23_999, 32_000));
        assert!(should_compact(24_000, 32_000));
        assert!(should_compact(30_000, 32_000));
    }

    #[test]
    fn select_fold_range_is_none_below_three_unpinned_messages() {
        let history = vec![unpinned("a"), unpinned("b"), pinned("c")];
        assert!(select_fold_range(&history).is_none());
    }

    #[test]
    fn select_fold_range_takes_the_oldest_third_of_unpinned_messages() {
        let history: Vec<Message> = (0..9).map(|n| unpinned(&n.to_string())).collect();
        let selection = select_fold_range(&history).unwrap();
        assert_eq!(selection.indices(), [0, 1, 2]);
    }

    /// Nine unpinned messages, with a pinned message before and after the
    /// oldest three. Nine is divisible by three, so "the oldest third"
    /// (indices 1, 2, 3) is unambiguous, and the pinned messages at 0 and 4
    /// prove selection skips them rather than just avoiding position zero.
    fn history_with_pinned_messages_around_the_fold() -> Vec<Message> {
        vec![
            pinned("keep-pinned-1"),
            unpinned("fold-1"),
            unpinned("fold-2"),
            unpinned("fold-3"),
            pinned("keep-pinned-2"),
            unpinned("mid-1"),
            unpinned("mid-2"),
            unpinned("mid-3"),
            unpinned("mid-4"),
            unpinned("mid-5"),
            unpinned("mid-6"),
        ]
    }

    #[test]
    fn select_fold_range_never_selects_a_pinned_message() {
        let history = history_with_pinned_messages_around_the_fold();
        let selection = select_fold_range(&history).unwrap();
        for &index in selection.indices() {
            assert!(!history[index].pinned, "selected a pinned message");
        }
        assert_eq!(selection.indices(), [1, 2, 3]);
    }

    #[test]
    fn build_summary_request_uses_the_scout_role() {
        let history = vec![unpinned("a"), unpinned("b"), unpinned("c")];
        let selection = select_fold_range(&history).unwrap();
        let request = build_summary_request(&history, &selection);
        assert_eq!(request.class, RoleClass::Scout);
    }

    #[test]
    fn build_summary_request_names_every_preserved_category() {
        let history = vec![unpinned("a"), unpinned("b"), unpinned("c")];
        let selection = select_fold_range(&history).unwrap();
        let request = build_summary_request(&history, &selection);
        let instructions = request.messages[0].text_content();
        for word in ["files", "decisions", "errors", "work that remains"] {
            assert!(
                instructions.contains(word),
                "instructions dropped {word:?}: {instructions}"
            );
        }
    }

    #[test]
    fn build_summary_request_carries_only_the_selected_messages() {
        let history = vec![
            unpinned("fold-1"),
            unpinned("fold-2"),
            unpinned("fold-3"),
            unpinned("keep"),
        ];
        let selection = select_fold_range(&history).unwrap();
        let request = build_summary_request(&history, &selection);
        // messages[0] is the instructions; the rest are the selected slice.
        assert_eq!(request.messages.len(), 1 + selection.indices().len());
        assert!(!request.messages.iter().any(|m| m.text_content() == "keep"));
    }

    #[test]
    fn apply_summary_replaces_the_selection_with_one_message() {
        let bus = EventBus::new();
        // Six unpinned messages: the oldest third is the first two.
        let history = vec![
            unpinned("fold-1"),
            unpinned("fold-2"),
            unpinned("keep-1"),
            unpinned("keep-2"),
            unpinned("keep-3"),
            unpinned("keep-4"),
        ];
        let selection = select_fold_range(&history).unwrap();
        assert_eq!(selection.indices(), [0, 1]);

        let result = apply_summary(&history, &selection, "the summary", &bus.tx());

        assert_eq!(result.len(), 5);
        assert_eq!(result[0].text_content(), "the summary");
        assert_eq!(result[1].text_content(), "keep-1");
        assert_eq!(result[4].text_content(), "keep-4");
    }

    #[test]
    fn apply_summary_leaves_pinned_messages_exactly_where_they_were() {
        let bus = EventBus::new();
        let history = history_with_pinned_messages_around_the_fold();
        let selection = select_fold_range(&history).unwrap();

        let result = apply_summary(&history, &selection, "the summary", &bus.tx());

        // The pinned message that led the folded range moves up by one slot
        // (three messages became one), but it is untouched: same text, still
        // pinned. The pinned message that followed the range never moves.
        assert_eq!(result[0].text_content(), "keep-pinned-1");
        assert!(result[0].pinned);
        assert_eq!(result[1].text_content(), "the summary");
        assert_eq!(result[2].text_content(), "keep-pinned-2");
        assert!(result[2].pinned);
        assert_eq!(result[3].text_content(), "mid-1");
    }

    #[tokio::test]
    async fn apply_summary_emits_a_notice() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        let history = history_with_pinned_messages_around_the_fold();
        let selection = select_fold_range(&history).unwrap();

        apply_summary(&history, &selection, "summary text", &bus.tx());

        let received = tokio::time::timeout(std::time::Duration::from_millis(50), rx.recv())
            .await
            .expect("a notice must arrive")
            .expect("bus is open");
        let Received::Event(Event::Notice(text)) = received else {
            panic!("expected a Notice event");
        };
        assert!(text.contains('3'), "notice did not name the count: {text}");
    }
}
