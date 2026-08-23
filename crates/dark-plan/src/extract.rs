//! Stage 4 of the charting pipeline: turn stage 3's axis answers into
//! structured candidates. Task unit `E3`.
//!
//! [`DefaultExtractor`] implements [`Extractor`](crate::chart::stages::Extractor)
//! (task unit `E1`, `crate::chart::stages`): one grammar-constrained
//! generation over the open axis answers, at the `extract` micro-role
//! (`MicroSampling::extract`, thinking off, temperature 0.2, 1200 tokens).
//! The build specification's Do step 3 names six deterministic checks the
//! response must pass before extraction accepts it; a response that fails
//! one gets a repair message describing the failure and a fresh generation,
//! up to [`MAX_ATTEMPTS`] tries in the one stage-4 sub-session (task unit
//! `E1`'s "fresh sub-session" rule is about isolation *between* stages, not
//! about a single stage's own retry loop, so a repair round-trip stays
//! inside this one call rather than starting a whole new stage).
//!
//! **One check this module cannot run.** Task unit `E3`, Do step 3, lists
//! "no question restates the destination" among the deterministic checks.
//! [`Extractor::extract`](crate::chart::stages::Extractor::extract) — the
//! seam task unit `E1` defined and this task unit must implement rather
//! than replace — takes only `answers: &[AxisAnswer]`; it carries no
//! destination text, and stage 3's [`AxisAnswer`] does not restate the
//! destination it was asked against either. There is no text in this
//! function's own inputs to compare a question to. This module runs the
//! other five checks and skips this one; see the task report for the same
//! note, flagged rather than worked around by reaching for a value the
//! trait does not offer.

use std::fmt::Write as _;

use dark_contract::{Engine, ErrCode, Error, Grammar, Message, Result, Role, RoleClass};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::axes::AxisAnswer;
use crate::chart::sampling::{BoxFuture, MicroSampling, build_request, run_generation};
use crate::chart::stages::{Candidate, ExtractOutput, Extractor, OutOfScopeCandidate};
use crate::chart::ticket::TicketKind;

/// How many generations one [`DefaultExtractor::extract`] call spends
/// before it gives up.
///
/// Task unit `E3`'s "Done when" asks for schema-valid output on the first
/// attempt in 90% of fixture cases, which leaves room for the other 10% to
/// need a repair round or two; three tries (one first attempt, two
/// repairs) is enough headroom for that without letting one bad candidate
/// list spin forever.
const MAX_ATTEMPTS: u8 = 3;

/// The JSON schema stage 4's grammar constrains the response to.
///
/// Mirrors task unit `E3`, Do step 2, field for field. `type` is the wire
/// name; [`RawCandidate`] renames it to [`Candidate::kind`] on the way in.
fn extract_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "candidates": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "name": { "type": "string" },
                        "question": { "type": "string" },
                        "axis": { "type": "string" },
                        "type": {
                            "type": "string",
                            "enum": ["research", "prototype", "grilling", "task"]
                        }
                    },
                    "required": ["name", "question", "axis", "type"],
                    "additionalProperties": false
                }
            },
            "out_of_scope": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "gist": { "type": "string" },
                        "reason": { "type": "string" }
                    },
                    "required": ["gist", "reason"],
                    "additionalProperties": false
                }
            }
        },
        "required": ["candidates", "out_of_scope"],
        "additionalProperties": false
    })
}

/// Builds the fresh two-message stage-4 prompt from `answers`.
///
/// Every open axis answer goes in; an axis stage 3 recorded as "nothing
/// here" never reaches this function (`ChartPipeline::stage_extract`
/// filters to [`crate::chart::AxisOutcome::Open`] before calling
/// [`Extractor::extract`]).
fn extract_prompt(answers: &[AxisAnswer]) -> Vec<Message> {
    let mut body = String::from(
        "Turn these open decisions from an axis sweep into structured candidates.\n\n",
    );
    for answer in answers {
        if let Some(text) = answer.outcome.open_text() {
            let _ = writeln!(body, "Axis: {}\nOpen decisions: {}\n", answer.axis, text);
        }
    }
    body.push_str(
        "For each distinct decision, name one candidate:\n\
         - name: 12 words or fewer, unique among the candidates\n\
         - question: the exact question this candidate raises, ending with a question mark\n\
         - axis: the axis the decision came from, copied from above\n\
         - type: one of research, prototype, grilling, task\n\n\
         List anything above that sits outside this map's scope as out_of_scope instead, with \
         a short gist and the reason it does not belong.\n\n\
         Name at least one candidate whose type is not research.\n\n\
         Answer with exactly one JSON object matching the schema you were given. Write nothing \
         else.",
    );

    vec![
        Message::text(
            Role::System,
            "You turn a decision map's open axis answers into structured candidates. See only \
             the axis answers you were given below. You have no memory of how they were \
             produced, and no memory of any earlier attempt at this task.",
        ),
        Message::text(Role::User, body),
    ]
}

/// Builds the repair message a failed check sends back for a retry.
///
/// Task unit `E3`, Do step 3: "Reject and retry with a repair message when
/// a check fails." `reason` names the one check that failed, so the retry
/// carries a concrete correction instead of a bare "try again."
fn repair_message(reason: &str) -> String {
    format!(
        "That answer did not pass a schema check: {reason}\n\n\
         Answer again with exactly one JSON object matching the schema you were given. Fix the \
         problem named above. Write nothing else."
    )
}

/// One candidate as the wire schema spells it, before task unit `E1`'s
/// [`Candidate`] renames `type` to [`Candidate::kind`].
#[derive(Debug, Deserialize)]
struct RawCandidate {
    /// See [`Candidate::name`].
    name: String,
    /// See [`Candidate::question`].
    question: String,
    /// See [`Candidate::axis`].
    axis: String,
    /// See [`Candidate::kind`]. Deserialising this field rejects anything
    /// other than the four values [`extract_schema`] enumerates, which is
    /// what carries out Do step 3's "the type is one of the four values"
    /// check: an unrecognised value fails to parse at all, and
    /// [`parse_extract_response`] turns that parse failure into the same
    /// kind of repairable error the other checks produce.
    #[serde(rename = "type")]
    kind: TicketKind,
}

/// [`ExtractOutput`] as the wire schema spells it.
#[derive(Debug, Deserialize)]
struct RawExtractOutput {
    /// See [`ExtractOutput::candidates`].
    candidates: Vec<RawCandidate>,
    /// See [`ExtractOutput::out_of_scope`].
    out_of_scope: Vec<OutOfScopeCandidate>,
}

/// Runs the deterministic checks task unit `E3`, Do step 3, names, minus
/// the one [`DefaultExtractor`]'s own module documentation explains this
/// module cannot run.
///
/// Returns the first violation found, worded so [`repair_message`] can
/// hand it straight back to the model.
fn validate(output: &ExtractOutput) -> std::result::Result<(), String> {
    let mut seen_names: Vec<String> = Vec::with_capacity(output.candidates.len());
    for candidate in &output.candidates {
        let normalised = candidate.name.trim().to_lowercase();
        if seen_names.contains(&normalised) {
            return Err(format!(
                "the candidate name {:?} is not unique",
                candidate.name
            ));
        }
        seen_names.push(normalised);

        if !candidate.question.trim_end().ends_with('?') {
            return Err(format!(
                "the question for {:?} does not end with a question mark: {:?}",
                candidate.name, candidate.question
            ));
        }

        if candidate.name.split_whitespace().count() > 12 {
            return Err(format!(
                "the candidate name {:?} is longer than 12 words",
                candidate.name
            ));
        }
    }

    if !output
        .candidates
        .iter()
        .any(|candidate| candidate.kind != TicketKind::Research)
    {
        return Err(
            "every candidate has type research; at least one candidate must be a different type"
                .to_owned(),
        );
    }

    Ok(())
}

/// Parses and validates one generation's raw text against the extraction
/// schema.
///
/// Returns the failure reason as a `String`, not a [`dark_contract::Error`]:
/// the caller feeds it to [`repair_message`] for a retry, and only turns it
/// into a real [`dark_contract::Error`] once [`MAX_ATTEMPTS`] is spent.
fn parse_extract_response(text: &str) -> std::result::Result<ExtractOutput, String> {
    let raw: RawExtractOutput = serde_json::from_str(text.trim()).map_err(|err| {
        format!("the response is not valid JSON for the extraction schema: {err}")
    })?;

    let output = ExtractOutput {
        candidates: raw
            .candidates
            .into_iter()
            .map(|candidate| Candidate {
                name: candidate.name,
                question: candidate.question,
                axis: candidate.axis,
                kind: candidate.kind,
            })
            .collect(),
        out_of_scope: raw.out_of_scope,
    };

    validate(&output)?;
    Ok(output)
}

/// The build specification's extraction stage.
///
/// See the module documentation for the one Do-step-3 check this
/// implementation cannot run, and why.
#[derive(Debug, Default, Clone, Copy)]
pub struct DefaultExtractor;

impl Extractor for DefaultExtractor {
    fn extract<'a>(
        &'a self,
        engine: &'a dyn Engine,
        class: RoleClass,
        sampling: MicroSampling,
        answers: &'a [AxisAnswer],
    ) -> BoxFuture<'a, Result<ExtractOutput>> {
        Box::pin(async move {
            let mut messages = extract_prompt(answers);
            let schema = extract_schema();
            let mut last_reason = String::new();

            for attempt in 1..=MAX_ATTEMPTS {
                let mut request = build_request(class, messages.clone(), sampling);
                if sampling.grammar {
                    request.grammar = Some(Grammar::JsonSchema(schema.clone()));
                }

                let generation = run_generation(engine, request).await?;

                match parse_extract_response(&generation.text) {
                    Ok(output) => return Ok(output),
                    Err(reason) => {
                        if attempt < MAX_ATTEMPTS {
                            messages.push(Message::text(Role::Assistant, generation.text));
                            messages.push(Message::text(Role::User, repair_message(&reason)));
                        }
                        last_reason = reason;
                    }
                }
            }

            Err(Error::new(
                ErrCode::EngineGenerate,
                format!(
                    "stage 4 (extract) did not pass its schema checks in {MAX_ATTEMPTS} \
                     attempts: {last_reason}"
                ),
            )
            .with_remedy("Retry stage 4. See dark map chart --resume --from-stage 4."))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::axes::AxisOutcome;
    use dark_engine_fake::script::Turn;
    use dark_engine_fake::{FakeEngine, Script};

    fn one_open_answer(axis: &str, text: &str) -> Vec<AxisAnswer> {
        vec![AxisAnswer {
            axis: axis.to_owned(),
            outcome: AxisOutcome::Open(text.to_owned()),
        }]
    }

    fn valid_json() -> &'static str {
        r#"{"candidates":[{"name":"retry cap","question":"What is the retry cap?",
        "axis":"failure modes and error handling","type":"task"}],"out_of_scope":[]}"#
    }

    #[tokio::test]
    async fn a_schema_valid_response_is_accepted_on_the_first_attempt() {
        let engine = FakeEngine::new(Script {
            turns: vec![Turn {
                text: valid_json().to_owned(),
                ..Default::default()
            }],
            ..Default::default()
        });
        let extractor = DefaultExtractor;
        let answers = one_open_answer("failure modes and error handling", "Retries are undecided.");

        let output = extractor
            .extract(
                &engine,
                RoleClass::Architect,
                MicroSampling::extract(),
                &answers,
            )
            .await
            .expect("a schema-valid response is accepted");

        assert_eq!(output.candidates.len(), 1);
        assert_eq!(output.candidates[0].name, "retry cap");
        assert_eq!(output.candidates[0].kind, TicketKind::Task);
        assert_eq!(engine.turns_played(), 1, "no retry was needed");
    }

    #[tokio::test]
    async fn the_prompt_carries_the_axis_answers_and_a_grammar() {
        let engine = FakeEngine::new(Script {
            turns: vec![Turn {
                text: valid_json().to_owned(),
                ..Default::default()
            }],
            ..Default::default()
        });
        let extractor = DefaultExtractor;
        let answers = one_open_answer(
            "failure modes and error handling",
            "UNIQUE-MARKER-Retries are undecided.",
        );

        extractor
            .extract(
                &engine,
                RoleClass::Architect,
                MicroSampling::extract(),
                &answers,
            )
            .await
            .expect("extraction succeeds");

        let seen = engine.seen_requests();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].messages.len(), 2, "a fresh two-message prompt");
        let text: String = seen[0]
            .messages
            .iter()
            .map(dark_contract::Message::text_content)
            .collect();
        assert!(text.contains("UNIQUE-MARKER-Retries are undecided."));
        assert!(matches!(seen[0].grammar, Some(Grammar::JsonSchema(_))));
    }

    #[tokio::test]
    async fn a_duplicate_name_is_rejected_and_repaired() {
        let bad = r#"{"candidates":[
            {"name":"retry cap","question":"What is the retry cap?","axis":"a","type":"task"},
            {"name":"retry cap","question":"What else?","axis":"a","type":"task"}
        ],"out_of_scope":[]}"#;
        let engine = FakeEngine::new(Script {
            turns: vec![
                Turn {
                    text: bad.to_owned(),
                    ..Default::default()
                },
                Turn {
                    text: valid_json().to_owned(),
                    ..Default::default()
                },
            ],
            ..Default::default()
        });
        let extractor = DefaultExtractor;
        let answers = one_open_answer("failure modes and error handling", "Retries are undecided.");

        let output = extractor
            .extract(
                &engine,
                RoleClass::Architect,
                MicroSampling::extract(),
                &answers,
            )
            .await
            .expect("the second attempt is schema-valid");

        assert_eq!(output.candidates.len(), 1);
        assert_eq!(engine.turns_played(), 2, "one repair round happened");

        let seen = engine.seen_requests();
        let repair_text: String = seen[1]
            .messages
            .iter()
            .map(dark_contract::Message::text_content)
            .collect();
        assert!(repair_text.contains("not unique"));
        // The repair conversation carries the model's own bad answer plus
        // the repair note, growing the two-message prompt by two.
        assert_eq!(seen[1].messages.len(), 4);
    }

    #[tokio::test]
    async fn a_question_without_a_question_mark_is_rejected() {
        let bad = r#"{"candidates":[
            {"name":"retry cap","question":"The retry cap.","axis":"a","type":"task"}
        ],"out_of_scope":[]}"#;
        let engine = FakeEngine::new(Script {
            turns: vec![
                Turn {
                    text: bad.to_owned(),
                    ..Default::default()
                },
                Turn {
                    text: valid_json().to_owned(),
                    ..Default::default()
                },
            ],
            ..Default::default()
        });
        let extractor = DefaultExtractor;
        let answers = one_open_answer("a", "x");

        extractor
            .extract(
                &engine,
                RoleClass::Architect,
                MicroSampling::extract(),
                &answers,
            )
            .await
            .expect("the second attempt is schema-valid");

        let seen = engine.seen_requests();
        let repair_text: String = seen[1]
            .messages
            .iter()
            .map(dark_contract::Message::text_content)
            .collect();
        assert!(repair_text.contains("question mark"));
    }

    #[tokio::test]
    async fn a_name_over_twelve_words_is_rejected() {
        let long_name = "one two three four five six seven eight nine ten eleven twelve thirteen";
        let bad = format!(
            r#"{{"candidates":[{{"name":"{long_name}","question":"What?","axis":"a","type":"task"}}],"out_of_scope":[]}}"#
        );
        let engine = FakeEngine::new(Script {
            turns: vec![
                Turn {
                    text: bad,
                    ..Default::default()
                },
                Turn {
                    text: valid_json().to_owned(),
                    ..Default::default()
                },
            ],
            ..Default::default()
        });
        let extractor = DefaultExtractor;
        let answers = one_open_answer("a", "x");

        extractor
            .extract(
                &engine,
                RoleClass::Architect,
                MicroSampling::extract(),
                &answers,
            )
            .await
            .expect("the second attempt is schema-valid");

        let seen = engine.seen_requests();
        let repair_text: String = seen[1]
            .messages
            .iter()
            .map(dark_contract::Message::text_content)
            .collect();
        assert!(repair_text.contains("12 words"));
    }

    #[tokio::test]
    async fn an_all_research_candidate_list_is_rejected() {
        let bad = r#"{"candidates":[
            {"name":"a","question":"a?","axis":"x","type":"research"}
        ],"out_of_scope":[]}"#;
        let engine = FakeEngine::new(Script {
            turns: vec![
                Turn {
                    text: bad.to_owned(),
                    ..Default::default()
                },
                Turn {
                    text: valid_json().to_owned(),
                    ..Default::default()
                },
            ],
            ..Default::default()
        });
        let extractor = DefaultExtractor;
        let answers = one_open_answer("a", "x");

        extractor
            .extract(
                &engine,
                RoleClass::Architect,
                MicroSampling::extract(),
                &answers,
            )
            .await
            .expect("the second attempt is schema-valid");

        let seen = engine.seen_requests();
        let repair_text: String = seen[1]
            .messages
            .iter()
            .map(dark_contract::Message::text_content)
            .collect();
        assert!(repair_text.contains("research"));
    }

    #[tokio::test]
    async fn malformed_json_is_rejected_and_repaired() {
        let engine = FakeEngine::new(Script {
            turns: vec![
                Turn {
                    text: "not json at all".to_owned(),
                    ..Default::default()
                },
                Turn {
                    text: valid_json().to_owned(),
                    ..Default::default()
                },
            ],
            ..Default::default()
        });
        let extractor = DefaultExtractor;
        let answers = one_open_answer("a", "x");

        let output = extractor
            .extract(
                &engine,
                RoleClass::Architect,
                MicroSampling::extract(),
                &answers,
            )
            .await
            .expect("the second attempt is schema-valid");
        assert_eq!(output.candidates.len(), 1);
    }

    #[tokio::test]
    async fn an_unrecognised_type_value_fails_to_parse_and_is_repaired() {
        let bad = r#"{"candidates":[
            {"name":"a","question":"a?","axis":"x","type":"epic"}
        ],"out_of_scope":[]}"#;
        let engine = FakeEngine::new(Script {
            turns: vec![
                Turn {
                    text: bad.to_owned(),
                    ..Default::default()
                },
                Turn {
                    text: valid_json().to_owned(),
                    ..Default::default()
                },
            ],
            ..Default::default()
        });
        let extractor = DefaultExtractor;
        let answers = one_open_answer("a", "x");

        extractor
            .extract(
                &engine,
                RoleClass::Architect,
                MicroSampling::extract(),
                &answers,
            )
            .await
            .expect("the second attempt is schema-valid");
        assert_eq!(engine.turns_played(), 2);
    }

    #[tokio::test]
    async fn exhausting_every_attempt_fails_with_a_named_remedy() {
        let bad = r#"{"candidates":[
            {"name":"a","question":"not a question","axis":"x","type":"task"}
        ],"out_of_scope":[]}"#;
        let engine = FakeEngine::new(Script {
            turns: vec![
                Turn {
                    text: bad.to_owned(),
                    ..Default::default()
                },
                Turn {
                    text: bad.to_owned(),
                    ..Default::default()
                },
                Turn {
                    text: bad.to_owned(),
                    ..Default::default()
                },
            ],
            ..Default::default()
        });
        let extractor = DefaultExtractor;
        let answers = one_open_answer("a", "x");

        let err = extractor
            .extract(
                &engine,
                RoleClass::Architect,
                MicroSampling::extract(),
                &answers,
            )
            .await
            .expect_err("every attempt fails the same check");

        assert_eq!(err.code, ErrCode::EngineGenerate);
        assert!(err.remedy.is_some());
        assert_eq!(engine.turns_played(), MAX_ATTEMPTS as usize);
    }
}
