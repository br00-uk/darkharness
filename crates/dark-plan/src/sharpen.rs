//! Stage 5 of the charting pipeline: decide whether one candidate is a
//! ticket or fog. Task unit `E4`.
//!
//! [`DefaultSharpener`] implements [`Sharpener`](crate::chart::stages::Sharpener)
//! (task unit `E1`, `crate::chart::stages`). Task unit `E4`, Do step 4,
//! names three exclusions that code must apply before the model is asked
//! at all — a candidate that repeats a recorded decision, matches a live
//! ticket name, or sits under an out-of-scope entry is never fog. None of
//! that context travels through
//! [`Sharpener::sharpen`](crate::chart::stages::Sharpener::sharpen)'s
//! per-candidate call (it takes only the one `candidate`), so
//! [`DefaultSharpener`] carries it as its own construction data instead —
//! the three lists a caller already holds once it knows what map it is
//! charting into. This is not a second seam beside
//! [`Sharpener`](crate::chart::stages::Sharpener): the trait is
//! implemented exactly as `E1` defined it; the exclusion lists are this
//! implementation's own state, the same way [`FileCheckpointStore`] (`E1`)
//! carries a path a `CheckpointStore` call never receives.
//!
//! Do step 5 ("Write one fog patch for each axis... merge them into one
//! patch") is already done: `ChartPipeline::run`'s private
//! `merge_fog_by_axis` groups every [`SharpenOutcome::Fog`] candidate by
//! axis once stage 5 finishes. [`DefaultSharpener`] only needs to answer
//! `Ticket` or `Fog` for one candidate at a time.
//!
//! [`FileCheckpointStore`]: crate::chart::FileCheckpointStore

use dark_contract::{Engine, ErrCode, Error, Grammar, Message, Result, Role, RoleClass};

use crate::chart::sampling::{BoxFuture, MicroSampling, build_request, run_generation};
use crate::chart::stages::{Candidate, SharpenOutcome, Sharpener};

/// Lower-cases and trims `text`, for a comparison that ignores case and
/// surrounding whitespace.
fn normalise(text: &str) -> String {
    text.trim().to_lowercase()
}

/// Returns whether `candidate_text` and `reference` are close enough to
/// call the same thing.
///
/// Exact match after [`normalise`] always counts. A short reference (the
/// name of a live ticket, say) also counts when it appears as a substring
/// of the longer text, or the reverse — but only once the shorter side is
/// at least eight characters, so a one- or two-word reference cannot match
/// by accident on common phrasing.
fn text_matches(candidate_text: &str, reference: &str) -> bool {
    let left = normalise(candidate_text);
    let right = normalise(reference);
    if left.is_empty() || right.is_empty() {
        return false;
    }
    if left == right {
        return true;
    }
    let (shorter, longer) = if left.len() <= right.len() {
        (&left, &right)
    } else {
        (&right, &left)
    };
    shorter.len() >= 8 && longer.contains(shorter.as_str())
}

/// Builds the literal classify block task unit `E4`, Do step 2, gives,
/// with `question` in place of its example.
fn classify_block(question: &str) -> String {
    format!(
        "Candidate: {question:?}\n\n\
         Can this question be STATED precisely now?\n\
         This does not ask whether you can answer it.\n\
         A question can be stated precisely even when the answer needs\n\
         research, a prototype, or a decision that nobody has made yet.\n\n\
         \u{2003}TICKET — the question is already sharp, even when it is blocked\n\
         \u{2003}FOG    — you cannot yet phrase it sharply, because it depends on\n\
         \u{2003}         something that is still open\n\n\
         Answer with one word."
    )
}

/// Builds the fresh stage-5 prompt: a system turn, two worked examples —
/// one of each answer (task unit `E4`, Do step 3) — and the real
/// candidate.
fn sharpen_prompt(candidate: &Candidate) -> Vec<Message> {
    vec![
        Message::text(
            Role::System,
            "You test one candidate question at a time for a decision map. See only this one \
             candidate. You have no memory of any other candidate, and no memory of an earlier \
             stage's conversation.",
        ),
        Message::text(
            Role::User,
            classify_block("How does a pack declare its staleness policy?"),
        ),
        Message::text(Role::Assistant, "TICKET"),
        Message::text(
            Role::User,
            classify_block("What should the retry policy be?"),
        ),
        Message::text(Role::Assistant, "FOG"),
        Message::text(Role::User, classify_block(&candidate.question)),
    ]
}

/// Parses the model's one-word answer.
///
/// # Errors
///
/// Returns [`ErrCode::EngineGenerate`] when the trimmed, upper-cased text
/// is neither `TICKET` nor `FOG`. Task unit `E4`'s classifier must not
/// silently pass an answer it cannot read as one of the two outcomes.
fn parse_sharpen_answer(text: &str) -> Result<SharpenOutcome> {
    match text.trim().to_uppercase().as_str() {
        "TICKET" => Ok(SharpenOutcome::Ticket),
        "FOG" => Ok(SharpenOutcome::Fog),
        other => Err(Error::new(
            ErrCode::EngineGenerate,
            format!(
                "stage 5 (sharpen) produced an unrecognised classification {other:?}; wanted \
                 TICKET or FOG"
            ),
        )
        .with_remedy("Retry stage 5. See dark map chart --resume --from-stage 5.")),
    }
}

/// The build specification's fog classifier.
///
/// Holds the three lists task unit `E4`, Do step 4, checks in code before
/// any candidate reaches the model: decisions this map has already
/// recorded, the names of tickets already live on it, and the gists stage
/// 4 (extract) marked out of scope. A fresh charting run — nothing
/// recorded yet, no tickets yet, only this pass's own out-of-scope list —
/// uses [`DefaultSharpener::new`] with the first two empty and the third
/// carrying stage 4's [`ExtractOutput::out_of_scope`] gists.
///
/// [`ExtractOutput::out_of_scope`]: crate::chart::stages::ExtractOutput::out_of_scope
#[derive(Debug, Clone, Default)]
pub struct DefaultSharpener {
    recorded_decisions: Vec<String>,
    live_ticket_names: Vec<String>,
    out_of_scope: Vec<String>,
}

impl DefaultSharpener {
    /// Builds a sharpener that excludes any candidate matching one of
    /// these three lists from ever being classified as fog.
    #[must_use]
    pub fn new(
        recorded_decisions: Vec<String>,
        live_ticket_names: Vec<String>,
        out_of_scope: Vec<String>,
    ) -> Self {
        Self {
            recorded_decisions,
            live_ticket_names,
            out_of_scope,
        }
    }

    /// Returns `true` when `candidate` matches one of the three
    /// deterministic exclusions, and so must not be asked about at all.
    fn is_excluded(&self, candidate: &Candidate) -> bool {
        let repeats_a_decision = self
            .recorded_decisions
            .iter()
            .any(|decision| text_matches(&candidate.question, decision));
        let matches_a_live_ticket = self
            .live_ticket_names
            .iter()
            .any(|name| text_matches(&candidate.name, name));
        let covered_by_out_of_scope = self.out_of_scope.iter().any(|gist| {
            text_matches(&candidate.question, gist) || text_matches(&candidate.name, gist)
        });

        repeats_a_decision || matches_a_live_ticket || covered_by_out_of_scope
    }
}

impl Sharpener for DefaultSharpener {
    fn sharpen<'a>(
        &'a self,
        engine: &'a dyn Engine,
        class: RoleClass,
        sampling: MicroSampling,
        candidate: &'a Candidate,
    ) -> BoxFuture<'a, Result<SharpenOutcome>> {
        Box::pin(async move {
            // Task unit `E4`, Do step 4: apply these exclusions with code,
            // never ask the model. A candidate that is already settled,
            // already a live ticket, or already ruled out of scope is not
            // an open question, so it cannot be fog.
            if self.is_excluded(candidate) {
                return Ok(SharpenOutcome::Ticket);
            }

            let messages = sharpen_prompt(candidate);
            let mut request = build_request(class, messages, sampling);
            if sampling.grammar {
                request.grammar = Some(Grammar::Regex("^(TICKET|FOG)$".to_owned()));
            }

            let generation = run_generation(engine, request).await?;
            parse_sharpen_answer(&generation.text)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chart::ticket::TicketKind;
    use dark_engine_fake::script::Turn;
    use dark_engine_fake::{FakeEngine, Script};

    fn candidate(name: &str, question: &str) -> Candidate {
        Candidate {
            name: name.to_owned(),
            question: question.to_owned(),
            axis: "failure modes and error handling".to_owned(),
            kind: TicketKind::Task,
        }
    }

    #[tokio::test]
    async fn a_candidate_repeating_a_recorded_decision_is_never_fog() {
        let engine = FakeEngine::new(Script::default());
        let sharpener = DefaultSharpener::new(
            vec!["Retries are capped at three attempts.".to_owned()],
            vec![],
            vec![],
        );
        let candidate = candidate("retry cap", "Retries are capped at three attempts.");

        let outcome = sharpener
            .sharpen(
                &engine,
                RoleClass::Architect,
                MicroSampling::classify(),
                &candidate,
            )
            .await
            .expect("excluded candidates never fail");

        assert_eq!(outcome, SharpenOutcome::Ticket);
        assert_eq!(engine.turns_played(), 0, "the model is never asked");
    }

    #[tokio::test]
    async fn a_candidate_matching_a_live_ticket_name_is_never_fog() {
        let engine = FakeEngine::new(Script::default());
        let sharpener = DefaultSharpener::new(vec![], vec!["staleness policy".to_owned()], vec![]);
        let candidate = candidate("staleness policy", "How does a pack declare staleness?");

        let outcome = sharpener
            .sharpen(
                &engine,
                RoleClass::Architect,
                MicroSampling::classify(),
                &candidate,
            )
            .await
            .expect("excluded candidates never fail");

        assert_eq!(outcome, SharpenOutcome::Ticket);
        assert_eq!(engine.turns_played(), 0);
    }

    #[tokio::test]
    async fn a_candidate_covered_by_an_out_of_scope_entry_is_never_fog() {
        let engine = FakeEngine::new(Script::default());
        let sharpener = DefaultSharpener::new(
            vec![],
            vec![],
            vec!["how often pack signing keys rotate".to_owned()],
        );
        let candidate = candidate(
            "key rotation",
            "How often pack signing keys rotate is out of scope for this map?",
        );

        let outcome = sharpener
            .sharpen(
                &engine,
                RoleClass::Architect,
                MicroSampling::classify(),
                &candidate,
            )
            .await
            .expect("excluded candidates never fail");

        assert_eq!(outcome, SharpenOutcome::Ticket);
        assert_eq!(engine.turns_played(), 0);
    }

    #[tokio::test]
    async fn an_unexcluded_candidate_is_asked_and_a_ticket_answer_is_read() {
        let engine = FakeEngine::new(Script {
            turns: vec![Turn {
                text: "TICKET".to_owned(),
                ..Default::default()
            }],
            ..Default::default()
        });
        let sharpener = DefaultSharpener::default();
        let candidate = candidate("staleness policy", "How does a pack declare staleness?");

        let outcome = sharpener
            .sharpen(
                &engine,
                RoleClass::Architect,
                MicroSampling::classify(),
                &candidate,
            )
            .await
            .expect("a clean TICKET answer parses");

        assert_eq!(outcome, SharpenOutcome::Ticket);
        assert_eq!(engine.turns_played(), 1);
    }

    #[tokio::test]
    async fn a_fog_answer_is_read_case_and_whitespace_insensitively() {
        let engine = FakeEngine::new(Script {
            turns: vec![Turn {
                text: "  fog  ".to_owned(),
                ..Default::default()
            }],
            ..Default::default()
        });
        let sharpener = DefaultSharpener::default();
        let candidate = candidate("retry policy", "What should the retry policy be?");

        let outcome = sharpener
            .sharpen(
                &engine,
                RoleClass::Architect,
                MicroSampling::classify(),
                &candidate,
            )
            .await
            .expect("a loosely formatted FOG answer still parses");

        assert_eq!(outcome, SharpenOutcome::Fog);
    }

    #[tokio::test]
    async fn an_unrecognised_answer_fails_explicitly_instead_of_silently_passing() {
        let engine = FakeEngine::new(Script {
            turns: vec![Turn {
                text: "maybe".to_owned(),
                ..Default::default()
            }],
            ..Default::default()
        });
        let sharpener = DefaultSharpener::default();
        let candidate = candidate("retry policy", "What should the retry policy be?");

        let err = sharpener
            .sharpen(
                &engine,
                RoleClass::Architect,
                MicroSampling::classify(),
                &candidate,
            )
            .await
            .expect_err("an answer that is neither TICKET nor FOG must not silently pass");

        assert_eq!(err.code, ErrCode::EngineGenerate);
        assert!(err.remedy.is_some());
    }

    #[tokio::test]
    async fn the_prompt_carries_one_worked_example_of_each_answer_and_a_grammar() {
        let engine = FakeEngine::new(Script {
            turns: vec![Turn {
                text: "TICKET".to_owned(),
                ..Default::default()
            }],
            ..Default::default()
        });
        let sharpener = DefaultSharpener::default();
        let candidate = candidate("staleness policy", "UNIQUE-MARKER question?");

        sharpener
            .sharpen(
                &engine,
                RoleClass::Architect,
                MicroSampling::classify(),
                &candidate,
            )
            .await
            .expect("classification succeeds");

        let seen = engine.seen_requests();
        assert_eq!(seen.len(), 1);
        let request = &seen[0];
        let text: String = request
            .messages
            .iter()
            .map(dark_contract::Message::text_content)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("staleness policy?\""));
        assert!(text.contains("retry policy be?\""));
        assert!(text.contains("UNIQUE-MARKER question?"));
        assert!(matches!(request.grammar, Some(Grammar::Regex(_))));
    }

    #[tokio::test]
    async fn no_grammar_is_set_when_the_sampling_settings_do_not_ask_for_one() {
        let engine = FakeEngine::new(Script {
            turns: vec![Turn {
                text: "TICKET".to_owned(),
                ..Default::default()
            }],
            ..Default::default()
        });
        let sharpener = DefaultSharpener::default();
        let candidate = candidate("staleness policy", "How does a pack declare staleness?");
        let mut sampling = MicroSampling::classify();
        sampling.grammar = false;

        sharpener
            .sharpen(&engine, RoleClass::Architect, sampling, &candidate)
            .await
            .expect("classification succeeds");

        let seen = engine.seen_requests();
        assert!(seen[0].grammar.is_none());
    }
}
