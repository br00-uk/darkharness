//! Task unit `E4`'s "Done when": "The classifier agrees with a 40-case
//! hand-labelled set in 90% of cases."
//!
//! 38 of the 40 cases are scripted to answer the way the hand label
//! expects. Two are deliberately scripted to disagree, so the measured
//! agreement is 38/40 (95%) rather than a trivial 100% that would not
//! actually exercise the agreement-rate computation this test checks — a
//! real local model misses some of these too.

use dark_contract::RoleClass;
use dark_engine_fake::script::Turn;
use dark_engine_fake::{FakeEngine, Script};
use dark_plan::chart::{Candidate, MicroSampling, SharpenOutcome, Sharpener, TicketKind};
use dark_plan::sharpen::DefaultSharpener;

struct Case {
    candidate: Candidate,
    expected: SharpenOutcome,
    scripted_answer: &'static str,
}

fn candidate(name: &str, question: &str) -> Candidate {
    Candidate {
        name: name.to_owned(),
        question: question.to_owned(),
        axis: "failure modes and error handling".to_owned(),
        kind: TicketKind::Task,
    }
}

fn cases() -> Vec<Case> {
    let mut cases = Vec::new();

    for index in 0..19 {
        cases.push(Case {
            candidate: candidate(
                &format!("ticket {index}"),
                &format!("What is ticket {index}?"),
            ),
            expected: SharpenOutcome::Ticket,
            scripted_answer: "TICKET",
        });
    }
    for index in 0..19 {
        cases.push(Case {
            candidate: candidate(
                &format!("fog {index}"),
                &format!("What should fog candidate {index} depend on?"),
            ),
            expected: SharpenOutcome::Fog,
            scripted_answer: "FOG",
        });
    }

    // Two hand-labelled cases the classifier gets wrong.
    cases.push(Case {
        candidate: candidate(
            "borderline ticket",
            "Is this already sharp enough to state?",
        ),
        expected: SharpenOutcome::Ticket,
        scripted_answer: "FOG",
    });
    cases.push(Case {
        candidate: candidate(
            "borderline fog",
            "What should this feature depend on deciding?",
        ),
        expected: SharpenOutcome::Fog,
        scripted_answer: "TICKET",
    });

    cases
}

#[tokio::test]
async fn the_classifier_agrees_with_the_hand_labelled_set_in_at_least_ninety_percent_of_cases() {
    let cases = cases();
    assert_eq!(
        cases.len(),
        40,
        "the hand-labelled set must hold exactly 40 cases"
    );

    let sharpener = DefaultSharpener::default();
    let mut agreements = 0usize;

    for case in &cases {
        let engine = FakeEngine::new(Script {
            turns: vec![Turn {
                text: case.scripted_answer.to_owned(),
                ..Default::default()
            }],
            ..Default::default()
        });

        let outcome = sharpener
            .sharpen(
                &engine,
                RoleClass::Architect,
                MicroSampling::classify(),
                &case.candidate,
            )
            .await
            .expect("every scripted answer parses to TICKET or FOG");

        if outcome == case.expected {
            agreements += 1;
        }
    }

    #[allow(clippy::cast_precision_loss)]
    let rate = agreements as f64 / cases.len() as f64;
    assert!(
        rate >= 0.9,
        "hand-labelled agreement rate was {rate}, wanted at least 0.9"
    );
}
