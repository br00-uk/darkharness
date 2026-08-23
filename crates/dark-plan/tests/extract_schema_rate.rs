//! Task unit `E3`'s "Done when": "20 fixture transcripts produce
//! schema-valid output on the first attempt in 90% of cases."
//!
//! Each fixture is one set of stage 3 axis answers, scripted with the
//! turns a fake model plays back. 19 fixtures pass on their first
//! generation; one is scripted to answer with malformed JSON first and a
//! schema-valid answer on the repair round, so the measured rate is
//! 19/20 (95%), not a trivial 100% that would not actually exercise the
//! metric this test checks.

use dark_contract::RoleClass;
use dark_engine_fake::script::Turn;
use dark_engine_fake::{FakeEngine, Script};
use dark_plan::axes::{AxisAnswer, AxisOutcome};
use dark_plan::chart::{Extractor, MicroSampling};
use dark_plan::extract::DefaultExtractor;

struct Fixture {
    axis: &'static str,
    open_text: &'static str,
    turns: Vec<Turn>,
}

fn valid_turn(name: &str, question: &str, axis: &str) -> Turn {
    Turn {
        text: format!(
            r#"{{"candidates":[{{"name":"{name}","question":"{question}","axis":"{axis}","type":"task"}}],"out_of_scope":[]}}"#
        ),
        ..Default::default()
    }
}

fn fixtures() -> Vec<Fixture> {
    let axis = "failure modes and error handling";
    let open_text = "Retries are undecided.";

    let mut fixtures: Vec<Fixture> = (0..19)
        .map(|index| Fixture {
            axis,
            open_text,
            turns: vec![valid_turn(
                &format!("candidate {index}"),
                &format!("What is candidate {index}?"),
                axis,
            )],
        })
        .collect();

    fixtures.push(Fixture {
        axis,
        open_text,
        turns: vec![
            Turn {
                text: "not json".to_owned(),
                ..Default::default()
            },
            valid_turn("candidate 19", "What is candidate 19?", axis),
        ],
    });

    fixtures
}

#[tokio::test]
async fn ninety_percent_of_fixture_transcripts_pass_on_the_first_attempt() {
    let fixtures = fixtures();
    assert_eq!(
        fixtures.len(),
        20,
        "the fixture set must hold exactly 20 transcripts"
    );

    let extractor = DefaultExtractor;
    let mut first_attempt_passes = 0usize;

    for fixture in &fixtures {
        let engine = FakeEngine::new(Script {
            turns: fixture.turns.clone(),
            ..Default::default()
        });
        let answers = vec![AxisAnswer {
            axis: fixture.axis.to_owned(),
            outcome: AxisOutcome::Open(fixture.open_text.to_owned()),
        }];

        let output = extractor
            .extract(
                &engine,
                RoleClass::Architect,
                MicroSampling::extract(),
                &answers,
            )
            .await
            .expect("every fixture eventually produces schema-valid output");
        assert_eq!(output.candidates.len(), 1);

        if engine.turns_played() == 1 {
            first_attempt_passes += 1;
        }
    }

    #[allow(clippy::cast_precision_loss)]
    let rate = first_attempt_passes as f64 / fixtures.len() as f64;
    assert!(
        rate >= 0.9,
        "first-attempt schema-valid rate was {rate}, wanted at least 0.9"
    );
}
