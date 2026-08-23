//! Stage 3 of the charting pipeline: the axis sweep.
//!
//! Task unit `E2`. Replaces wide thinking — a 32B model's weak spot (see
//! the weakness table in the `E` section preamble of `PRD.md`) — with
//! enumeration against a fixed list: one narrow question per axis, one
//! generation each, each in its own fresh sub-session (task unit `E1`, Do
//! step 2). This module is the reason `E2`'s task brief says "Needs: `Z1`,
//! `E1`": it calls `crate::chart::sampling::run_generation`, the
//! conversation driver `E1` built, once per axis.

use std::fmt::Write as _;

use dark_contract::{Engine, Message, Result, Role, RoleClass};
use serde::{Deserialize, Serialize};

use crate::axes::set::AxisSet;
use crate::chart::sampling::{MicroSampling, build_request, run_generation};

/// What one axis produced.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AxisOutcome {
    /// The axis raised open decisions. Carries the model's raw answer, for
    /// stage 4 (extract) to turn into candidates.
    Open(String),
    /// The axis raised nothing. Task unit `E2`, Do step 6: "Accept 'nothing
    /// here' as a valid answer for an axis" — this is that acceptance,
    /// recorded rather than discarded, so a caller can show that every
    /// axis was actually asked about.
    NothingHere,
}

impl AxisOutcome {
    /// Returns the model's raw text for an [`AxisOutcome::Open`] answer.
    #[must_use]
    pub fn open_text(&self) -> Option<&str> {
        match self {
            Self::Open(text) => Some(text.as_str()),
            Self::NothingHere => None,
        }
    }
}

/// One axis, answered.
///
/// Task unit `E2`, Do step 5: "Record which axis produced each candidate.
/// Store it in `tickets.axis` and `fog.axis`." This is the record that
/// downstream stages read the axis name off of; see
/// `crate::chart::stages::Candidate::axis`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AxisAnswer {
    /// The axis this answer is for.
    pub axis: String,
    /// What the axis produced.
    pub outcome: AxisOutcome,
}

/// Normalises a model's raw answer into an [`AxisOutcome`].
///
/// The comparison is case-insensitive and ignores trailing punctuation and
/// surrounding whitespace, because the `deliberate` micro-role runs with no
/// grammar constraint (`MicroSampling::deliberate().grammar` is `false`):
/// the model's exact phrasing is not guaranteed, so accepting "Nothing
/// here.", "nothing here", and "NOTHING HERE" alike is what makes Do step 6
/// hold in practice, not only on the one exact string the example prompt
/// shows.
fn classify_answer(text: &str) -> AxisOutcome {
    let normalised = text
        .trim()
        .trim_end_matches(['.', '!'])
        .trim()
        .to_lowercase();
    if normalised == "nothing here" {
        AxisOutcome::NothingHere
    } else {
        AxisOutcome::Open(text.trim().to_owned())
    }
}

/// Builds the one question stage 3 asks for one axis.
fn axis_prompt(destination: &str, seed_text: Option<&str>, axis: &str) -> Vec<Message> {
    let mut context = format!("Destination:\n{destination}\n");
    if let Some(seed) = seed_text {
        context.push_str("\nWhat the repository already shows:\n");
        context.push_str(seed);
        context.push('\n');
    }
    let _ = write!(
        context,
        "\nAxis: {axis}\n\nWhat decisions are still open on this axis, for this destination? \
         List each one plainly. If this axis raises nothing for this destination, answer \
         exactly \"nothing here\"."
    );

    vec![
        Message::text(
            Role::System,
            "You chart one axis of a decision map. See only this axis, the destination, and \
             what the repository already shows. You have no memory of any other axis.",
        ),
        Message::text(Role::User, context),
    ]
}

/// Runs stage 3 (axis sweep): one turn for each axis in `axis_set`, in
/// order.
///
/// Each axis gets its own fresh call — `axis_prompt` builds a two-message
/// conversation from nothing, never appending to a running transcript — so
/// no axis's prompt carries text from another axis's answer. That is what
/// task unit `E1`'s "Done when" calls "Stage N's prompt contains no text
/// from stage N-1's transcript," applied within stage 3 itself, axis by
/// axis.
///
/// `seed_text` is stage 2's hand-off (task unit `E2`, Do step 4): "The seam
/// report answers 'blast radius' and much of 'current shape' with computed
/// numbers. Give the model those numbers. Do not ask it to guess them." A
/// caller charting a destination with no repository behind it yet passes
/// `None`.
///
/// # Errors
///
/// Returns an error when the engine fails on any axis. The axes already
/// answered are lost on that path; the caller re-runs stage 3 from a
/// checkpoint rather than this function retrying internally, matching task
/// unit `E1`'s checkpoint-per-stage design.
pub async fn run_axis_sweep(
    engine: &dyn Engine,
    class: RoleClass,
    sampling: MicroSampling,
    destination: &str,
    axis_set: &AxisSet,
    seed_text: Option<&str>,
) -> Result<Vec<AxisAnswer>> {
    let mut answers = Vec::with_capacity(axis_set.axes.len());
    for axis in &axis_set.axes {
        let messages = axis_prompt(destination, seed_text, axis);
        let request = build_request(class, messages, sampling);
        let generation = run_generation(engine, request).await?;
        answers.push(AxisAnswer {
            axis: axis.clone(),
            outcome: classify_answer(&generation.text),
        });
    }
    Ok(answers)
}

#[cfg(test)]
mod tests {
    use super::*;
    use dark_engine_fake::script::Turn;
    use dark_engine_fake::{FakeEngine, Script};

    fn set_of(axes: &[&str]) -> AxisSet {
        AxisSet {
            axes: axes.iter().map(|a| (*a).to_owned()).collect(),
        }
    }

    #[test]
    fn nothing_here_is_recognised_regardless_of_case_or_punctuation() {
        assert_eq!(classify_answer("nothing here"), AxisOutcome::NothingHere);
        assert_eq!(classify_answer("Nothing here."), AxisOutcome::NothingHere);
        assert_eq!(
            classify_answer("  NOTHING HERE  "),
            AxisOutcome::NothingHere
        );
    }

    #[test]
    fn an_open_answer_keeps_its_text() {
        let outcome = classify_answer("The retry policy is undecided.");
        assert_eq!(
            outcome,
            AxisOutcome::Open("The retry policy is undecided.".to_owned())
        );
    }

    #[tokio::test]
    async fn the_sweep_asks_one_turn_for_each_axis_in_order() {
        let engine = FakeEngine::new(Script {
            turns: vec![
                Turn {
                    text: "Retry policy is undecided.".to_owned(),
                    ..Default::default()
                },
                Turn {
                    text: "nothing here".to_owned(),
                    ..Default::default()
                },
            ],
            ..Default::default()
        });

        let axis_set = set_of(&["failure modes and error handling", "observability"]);
        let answers = run_axis_sweep(
            &engine,
            RoleClass::Architect,
            MicroSampling::deliberate(),
            "A retry policy for the pack fetcher",
            &axis_set,
            None,
        )
        .await
        .expect("sweep succeeds");

        assert_eq!(answers.len(), 2);
        assert_eq!(answers[0].axis, "failure modes and error handling");
        assert_eq!(
            answers[0].outcome,
            AxisOutcome::Open("Retry policy is undecided.".to_owned())
        );
        assert_eq!(answers[1].axis, "observability");
        assert_eq!(answers[1].outcome, AxisOutcome::NothingHere);
    }

    #[tokio::test]
    async fn no_axis_prompt_carries_text_from_another_axis_answer() {
        let engine = FakeEngine::new(Script {
            turns: vec![
                Turn {
                    text: "UNIQUE-ANSWER-ONE about retries".to_owned(),
                    ..Default::default()
                },
                Turn {
                    text: "nothing here".to_owned(),
                    ..Default::default()
                },
            ],
            ..Default::default()
        });

        let axis_set = set_of(&["failure modes and error handling", "observability"]);
        run_axis_sweep(
            &engine,
            RoleClass::Architect,
            MicroSampling::deliberate(),
            "A retry policy for the pack fetcher",
            &axis_set,
            None,
        )
        .await
        .expect("sweep succeeds");

        let seen = engine.seen_requests();
        assert_eq!(seen.len(), 2);
        let second_axis_text: String = seen[1]
            .messages
            .iter()
            .map(dark_contract::Message::text_content)
            .collect();
        assert!(
            !second_axis_text.contains("UNIQUE-ANSWER-ONE"),
            "the observability axis prompt must not carry the failure-modes answer"
        );
    }

    #[tokio::test]
    async fn seed_text_is_included_when_given() {
        let engine = FakeEngine::new(Script {
            turns: vec![Turn {
                text: "nothing here".to_owned(),
                ..Default::default()
            }],
            ..Default::default()
        });

        let axis_set = set_of(&["blast radius"]);
        run_axis_sweep(
            &engine,
            RoleClass::Architect,
            MicroSampling::deliberate(),
            "Some destination",
            &axis_set,
            Some("blast radius: 40 files reachable, 6 within a bounding seam"),
        )
        .await
        .expect("sweep succeeds");

        let seen = engine.seen_requests();
        let text: String = seen[0]
            .messages
            .iter()
            .map(dark_contract::Message::text_content)
            .collect();
        assert!(text.contains("40 files reachable"));
    }
}
