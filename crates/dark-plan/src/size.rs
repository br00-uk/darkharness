//! Stage 6 of the charting pipeline: make one ticket fit inside the
//! session budget. Task unit `E5`.
//!
//! [`DefaultSizer`] implements [`Sizer`](crate::chart::stages::Sizer) (task
//! unit `E1`, `crate::chart::stages`): one grammar-constrained `classify`
//! call per candidate, answering the three questions task unit `E5`, Do
//! step 2, names, followed by a code-side decision that uses
//! [`Sizer::size`](crate::chart::stages::Sizer::size)'s `budget_tokens`
//! argument the way its own doc comment describes — "tests whether one
//! candidate fits inside the ticket budget."
//!
//! **How a split gets its content.** [`crate::chart::stages::SizeOutcome::Split`]
//! carries `Vec<Candidate>` — full replacement candidates, each with its
//! own name and question — but the build specification never says how
//! that content is produced. The stage table (task unit `E1`, Do step 1)
//! gives stage 6 a `classify` micro-role and an `ok`/`split` output, the
//! same shape as stage 5's `ticket`/`fog`; Do step 9's cost estimate
//! ("size and wire ~2N generations, single token") confirms this is meant
//! to stay one short generation per ticket, not a second `extract`-sized
//! call for split content. So [`split_candidate`] builds the replacement
//! candidates deterministically, from the original candidate's own text,
//! with no further generation. This is a genuine gap in the build
//! specification, not a design this module was told to make — flagged in
//! the task report rather than invented a second seam to paper over it.

use dark_contract::{Engine, ErrCode, Error, Grammar, Message, Result, Role, RoleClass};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::chart::sampling::{BoxFuture, MicroSampling, build_request, run_generation};
use crate::chart::stages::{Candidate, SizeOutcome, Sizer};

/// The estimator's starting tokens-per-file rate, before task unit `E5`,
/// Do step 4's calibration has run once.
///
/// A rough middle ground between a one-line change and a change that reads
/// several files' worth of surrounding context: generous enough that a
/// ticket touching a handful of files trips the size check before it
/// actually blows the budget in practice.
pub const DEFAULT_TOKENS_PER_FILE: usize = 900;

/// The flat token surcharge a research ticket adds to the estimate, on top
/// of its file count: reading around a codebase for an unanswered question
/// costs more context than editing a known file.
pub const DEFAULT_RESEARCH_OVERHEAD_TOKENS: usize = 3000;

/// Computes the token budget one ticket must fit.
///
/// Task unit `E5`, Do step 1: `ticket_budget = granted_context * 0.55`.
/// Integer arithmetic (`* 55 / 100`) avoids a float-to-integer cast:
/// `32_768 * 55 / 100 = 18_022`, matching the specification's "about 18000
/// tokens at a 32k grant."
#[must_use]
pub fn ticket_budget_tokens(granted_context: usize) -> usize {
    granted_context.saturating_mul(55) / 100
}

/// Estimates how many tokens resolving a ticket touching `files_touched`
/// files will use, at `tokens_per_file` tokens per file plus
/// `research_overhead_tokens` more when the ticket needs research.
#[must_use]
pub fn estimate_ticket_tokens(
    files_touched: usize,
    needs_research: bool,
    tokens_per_file: usize,
    research_overhead_tokens: usize,
) -> usize {
    let files_cost = files_touched.saturating_mul(tokens_per_file);
    if needs_research {
        files_cost.saturating_add(research_overhead_tokens)
    } else {
        files_cost
    }
}

/// One resolved ticket's actual token cost, for calibrating the estimator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SizeSample {
    /// How many files sizing predicted this ticket would touch.
    pub files_touched: usize,
    /// `tickets.tokens_used`, once the ticket's resolution finished.
    pub tokens_used: usize,
}

/// Recomputes tokens-per-file from resolved-ticket telemetry.
///
/// Task unit `E5`, Do step 4: "Read `tickets.tokens_used` after
/// resolutions accumulate. Calibrate the estimator." `dark-plan` does not
/// store `tickets.tokens_used` itself — that column lives in
/// `dark-cartograph`'s map store, which `dark-plan` does not depend on
/// (see `crate::chart::ticket`'s module documentation) — so this function
/// takes the samples as already read from wherever a caller keeps them,
/// and returns the rate a caller should pass as [`DefaultSizer::new`]'s
/// `tokens_per_file` argument on the next chart.
///
/// Returns `None` when every sample's `files_touched` is zero, since a
/// per-file average is undefined in that case.
#[must_use]
pub fn calibrate_tokens_per_file(samples: &[SizeSample]) -> Option<usize> {
    let total_files: usize = samples.iter().map(|sample| sample.files_touched).sum();
    if total_files == 0 {
        return None;
    }
    let total_tokens: usize = samples.iter().map(|sample| sample.tokens_used).sum();
    Some(total_tokens / total_files)
}

/// The JSON schema stage 6's grammar constrains the response to: the three
/// questions task unit `E5`, Do step 2, names, packed into one short
/// generation.
fn size_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "files": { "type": "integer", "minimum": 0 },
            "research": { "type": "boolean" },
            "multi": { "type": "boolean" }
        },
        "required": ["files", "research", "multi"],
        "additionalProperties": false
    })
}

/// Builds the fresh stage-6 prompt for one ticket.
fn size_prompt(question: &str) -> Vec<Message> {
    let body = format!(
        "Ticket: {question:?}\n\n\
         Answer three questions about this ticket, briefly and honestly:\n\
         files: how many files does resolving it touch? A small integer.\n\
         research: does it need research before anyone can act on it?\n\
         multi: does it contain more than one decision, not one?\n\n\
         Answer with exactly one JSON object: {{\"files\": <integer>, \"research\": <bool>, \
         \"multi\": <bool>}}. Write nothing else."
    );

    vec![
        Message::text(
            Role::System,
            "You size one ticket at a time for a decision map. See only this one ticket. You \
             have no memory of any other ticket.",
        ),
        Message::text(Role::User, body),
    ]
}

/// One parsed stage-6 answer.
#[derive(Debug, Deserialize)]
struct SizeAnswer {
    /// How many files the model believes resolving the ticket touches.
    files: usize,
    /// Whether the model believes the ticket needs research.
    research: bool,
    /// Whether the model believes the ticket holds more than one decision.
    ///
    /// Task unit `E5`, Do step 3: "This signal is reliable. A model
    /// detects it more accurately than it estimates tokens" — so this
    /// field, not the token estimate built from `files` and `research`
    /// alone, is what [`DefaultSizer::size`] trusts first.
    multi: bool,
}

/// Parses one stage-6 response.
///
/// # Errors
///
/// Returns [`ErrCode::EngineGenerate`] when the text does not match the
/// schema [`size_schema`] asked for. A size answer that cannot be read
/// must not silently pass as `ok`.
fn parse_size_answer(text: &str) -> Result<SizeAnswer> {
    serde_json::from_str(text.trim()).map_err(|err| {
        Error::new(
            ErrCode::EngineGenerate,
            format!("stage 6 (size) produced an answer that does not match its schema: {err}"),
        )
        .with_remedy("Retry stage 6. See dark map chart --resume --from-stage 6.")
    })
}

/// Splits a question's text into two clauses at its first conjunction, or
/// duplicates it with a part marker when no conjunction is present.
///
/// See the module documentation for why this is a deterministic text
/// split rather than a second generation.
fn split_question_text(question: &str) -> Vec<String> {
    let trimmed = question.trim();
    let body = trimmed.trim_end_matches('?').trim();

    for separator in [" and ", "; ", " or "] {
        if let Some((first, rest)) = body.split_once(separator) {
            let first = first.trim();
            let rest = rest.trim();
            if !first.is_empty() && !rest.is_empty() {
                return vec![format!("{first}?"), format!("{rest}?")];
            }
        }
    }

    vec![
        format!("{body}, part 1 of 2?"),
        format!("{body}, part 2 of 2?"),
    ]
}

/// Splits one candidate into its replacement candidates.
///
/// See the module documentation for why this is deterministic rather than
/// model-generated.
fn split_candidate(candidate: &Candidate) -> Vec<Candidate> {
    let questions = split_question_text(&candidate.question);
    let total = questions.len();
    questions
        .into_iter()
        .enumerate()
        .map(|(index, question)| Candidate {
            name: format!("{} ({}/{total})", candidate.name, index + 1),
            question,
            axis: candidate.axis.clone(),
            kind: candidate.kind,
        })
        .collect()
}

/// The build specification's ticket sizer.
///
/// `tokens_per_file` starts at [`DEFAULT_TOKENS_PER_FILE`]; a caller who
/// has run [`calibrate_tokens_per_file`] against resolved-ticket telemetry
/// passes the recalibrated rate to [`DefaultSizer::new`] instead.
#[derive(Debug, Clone, Copy)]
pub struct DefaultSizer {
    tokens_per_file: usize,
    research_overhead_tokens: usize,
}

impl DefaultSizer {
    /// Builds a sizer with an explicit tokens-per-file rate and research
    /// overhead.
    #[must_use]
    pub fn new(tokens_per_file: usize, research_overhead_tokens: usize) -> Self {
        Self {
            tokens_per_file,
            research_overhead_tokens,
        }
    }
}

impl Default for DefaultSizer {
    fn default() -> Self {
        Self::new(DEFAULT_TOKENS_PER_FILE, DEFAULT_RESEARCH_OVERHEAD_TOKENS)
    }
}

impl Sizer for DefaultSizer {
    fn size<'a>(
        &'a self,
        engine: &'a dyn Engine,
        class: RoleClass,
        sampling: MicroSampling,
        candidate: &'a Candidate,
        budget_tokens: usize,
    ) -> BoxFuture<'a, Result<SizeOutcome>> {
        Box::pin(async move {
            let messages = size_prompt(&candidate.question);
            let mut request = build_request(class, messages, sampling);
            if sampling.grammar {
                request.grammar = Some(Grammar::JsonSchema(size_schema()));
            }

            let generation = run_generation(engine, request).await?;
            let answer = parse_size_answer(&generation.text)?;

            let estimated = estimate_ticket_tokens(
                answer.files,
                answer.research,
                self.tokens_per_file,
                self.research_overhead_tokens,
            );

            // Do step 3: the multi-decision signal is reliable and always
            // forces a split. An oversized single-decision ticket also
            // splits, because `budget_tokens` (`Sizer::size`'s own
            // contract) must fit for the outcome to be `Ok`.
            if answer.multi || estimated > budget_tokens {
                Ok(SizeOutcome::Split(split_candidate(candidate)))
            } else {
                Ok(SizeOutcome::Ok)
            }
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

    #[test]
    fn ticket_budget_matches_the_build_specification_formula() {
        assert_eq!(ticket_budget_tokens(32_768), 18_022);
        assert_eq!(ticket_budget_tokens(0), 0);
    }

    #[test]
    fn the_token_estimate_adds_research_overhead_only_when_needed() {
        let plain = estimate_ticket_tokens(3, false, 900, 3000);
        assert_eq!(plain, 2700);
        let research = estimate_ticket_tokens(3, true, 900, 3000);
        assert_eq!(research, 5700);
    }

    #[test]
    fn calibration_averages_tokens_used_per_file_touched() {
        let samples = [
            SizeSample {
                files_touched: 2,
                tokens_used: 2000,
            },
            SizeSample {
                files_touched: 2,
                tokens_used: 4000,
            },
        ];
        assert_eq!(calibrate_tokens_per_file(&samples), Some(1500));
    }

    #[test]
    fn calibration_is_undefined_when_no_sample_touches_a_file() {
        let samples = [SizeSample {
            files_touched: 0,
            tokens_used: 500,
        }];
        assert_eq!(calibrate_tokens_per_file(&samples), None);
    }

    #[tokio::test]
    async fn a_small_single_decision_ticket_fits_the_budget() {
        let engine = FakeEngine::new(Script {
            turns: vec![Turn {
                text: r#"{"files":1,"research":false,"multi":false}"#.to_owned(),
                ..Default::default()
            }],
            ..Default::default()
        });
        let sizer = DefaultSizer::default();
        let candidate = candidate("retry cap", "What is the retry cap?");

        let outcome = sizer
            .size(
                &engine,
                RoleClass::Architect,
                MicroSampling::classify(),
                &candidate,
                18_000,
            )
            .await
            .expect("sizing succeeds");

        assert_eq!(outcome, SizeOutcome::Ok);
    }

    /// Task unit `E5`'s "Done when": "A multi-decision fixture ticket is
    /// split."
    #[tokio::test]
    async fn a_multi_decision_fixture_ticket_is_split() {
        let engine = FakeEngine::new(Script {
            turns: vec![Turn {
                text: r#"{"files":1,"research":false,"multi":true}"#.to_owned(),
                ..Default::default()
            }],
            ..Default::default()
        });
        let sizer = DefaultSizer::default();
        let candidate = candidate(
            "pack lifecycle",
            "How does a pack declare staleness and how does it get evicted?",
        );

        let outcome = sizer
            .size(
                &engine,
                RoleClass::Architect,
                MicroSampling::classify(),
                &candidate,
                18_000,
            )
            .await
            .expect("sizing succeeds");

        let SizeOutcome::Split(parts) = outcome else {
            panic!("a multi-decision ticket must split");
        };
        assert_eq!(parts.len(), 2);
        for part in &parts {
            assert!(part.question.ends_with('?'));
            assert_eq!(part.axis, candidate.axis);
            assert_eq!(part.kind, candidate.kind);
        }
        assert!(parts[0].question.contains("declare staleness"));
        assert!(parts[1].question.contains("get evicted"));
    }

    #[tokio::test]
    async fn an_oversized_single_decision_ticket_still_splits_on_budget_alone() {
        let engine = FakeEngine::new(Script {
            turns: vec![Turn {
                text: r#"{"files":50,"research":false,"multi":false}"#.to_owned(),
                ..Default::default()
            }],
            ..Default::default()
        });
        let sizer = DefaultSizer::default();
        let candidate = candidate("giant ticket", "Does this ticket touch fifty files?");

        let outcome = sizer
            .size(
                &engine,
                RoleClass::Architect,
                MicroSampling::classify(),
                &candidate,
                18_000,
            )
            .await
            .expect("sizing succeeds");

        assert!(matches!(outcome, SizeOutcome::Split(_)));
    }

    #[tokio::test]
    async fn a_research_ticket_that_only_just_fits_still_fits() {
        let engine = FakeEngine::new(Script {
            turns: vec![Turn {
                text: r#"{"files":1,"research":true,"multi":false}"#.to_owned(),
                ..Default::default()
            }],
            ..Default::default()
        });
        let sizer = DefaultSizer::default();
        let candidate = candidate("research spike", "What does the registry return?");

        let outcome = sizer
            .size(
                &engine,
                RoleClass::Architect,
                MicroSampling::classify(),
                &candidate,
                18_000,
            )
            .await
            .expect("sizing succeeds");

        // 1 file * 900 + 3000 research overhead = 3900, well inside budget.
        assert_eq!(outcome, SizeOutcome::Ok);
    }

    #[tokio::test]
    async fn a_malformed_answer_fails_explicitly_instead_of_silently_passing() {
        let engine = FakeEngine::new(Script {
            turns: vec![Turn {
                text: "not json".to_owned(),
                ..Default::default()
            }],
            ..Default::default()
        });
        let sizer = DefaultSizer::default();
        let candidate = candidate("retry cap", "What is the retry cap?");

        let err = sizer
            .size(
                &engine,
                RoleClass::Architect,
                MicroSampling::classify(),
                &candidate,
                18_000,
            )
            .await
            .expect_err("a schema-invalid answer must not silently pass as ok");

        assert_eq!(err.code, ErrCode::EngineGenerate);
        assert!(err.remedy.is_some());
    }

    #[tokio::test]
    async fn the_request_carries_a_json_schema_grammar_when_asked_for_one() {
        let engine = FakeEngine::new(Script {
            turns: vec![Turn {
                text: r#"{"files":1,"research":false,"multi":false}"#.to_owned(),
                ..Default::default()
            }],
            ..Default::default()
        });
        let sizer = DefaultSizer::default();
        let candidate = candidate("retry cap", "What is the retry cap?");

        sizer
            .size(
                &engine,
                RoleClass::Architect,
                MicroSampling::classify(),
                &candidate,
                18_000,
            )
            .await
            .expect("sizing succeeds");

        let seen = engine.seen_requests();
        assert!(matches!(seen[0].grammar, Some(Grammar::JsonSchema(_))));
    }
}
