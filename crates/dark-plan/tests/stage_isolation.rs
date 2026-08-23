//! Task unit `E1`'s "Done when": "Stage N's prompt contains no text from
//! stage N-1's transcript."
//!
//! This test runs the whole seven-stage pipeline and inspects every request
//! the engine actually received (`FakeEngine::seen_requests`). Two things
//! prove isolation:
//!
//! - Every request carries exactly the two messages its stage builds fresh
//!   (a system message and a user message) — never more, which would mean a
//!   later stage's request grew by appending an earlier stage's messages.
//! - Each stage's system-prompt text, chosen to be unique to that stage, is
//!   findable in that stage's own request and in no other.
//!
//! Stages 4 to 7 are trait objects (`crate::chart::stages`) that task units
//! `E3` to `E6` implement for real; this test plugs in minimal stand-ins
//! that each make exactly one engine call with their own fresh, marked
//! prompt, so the isolation property is checked across the full pipeline,
//! not only across the two stages (`destination`, `axes`) this task unit
//! implements outright.

use dark_contract::{Engine, Message, Result, Role, RoleClass};
use dark_engine_fake::script::Turn;
use dark_engine_fake::{FakeEngine, Script};
use dark_plan::axes::{AxisSet, AxisSets};
use dark_plan::chart::sampling::BoxFuture;
use dark_plan::chart::{
    Candidate, ChartConfig, ChartPipeline, ChartRun, DestinationInput, ExtractOutput, Extractor,
    FileCheckpointStore, MicroSampling, SeedReport, SharpenOutcome, Sharpener, SizeOutcome, Sizer,
    StageImpls, StageSampling, TicketKind, WireAnswer, Wirer, build_request, run_generation,
};

/// Builds a two-message request whose system prompt is `marker`, so a test
/// can find it (or fail to) in another stage's request.
fn marked_request(class: RoleClass, marker: &str, body: &str) -> dark_contract::Request {
    let messages = vec![
        Message::text(Role::System, marker),
        Message::text(Role::User, body),
    ];
    build_request(class, messages, MicroSampling::classify())
}

struct MarkedExtractor;
impl Extractor for MarkedExtractor {
    fn extract<'a>(
        &'a self,
        engine: &'a dyn Engine,
        class: RoleClass,
        _sampling: MicroSampling,
        _destination: &'a str,
        answers: &'a [dark_plan::axes::AxisAnswer],
    ) -> BoxFuture<'a, Result<ExtractOutput>> {
        Box::pin(async move {
            let request = marked_request(class, "STAGE4-EXTRACT-MARKER", "extract candidates");
            run_generation(engine, request).await?;
            assert_eq!(
                answers.len(),
                1,
                "the fixture axis set has exactly one open axis"
            );
            Ok(ExtractOutput {
                candidates: vec![
                    Candidate {
                        name: "retry cap".to_owned(),
                        question: "What is the retry cap?".to_owned(),
                        axis: answers[0].axis.clone(),
                        kind: TicketKind::Task,
                    },
                    Candidate {
                        name: "staleness policy".to_owned(),
                        question: "How is staleness declared?".to_owned(),
                        axis: answers[0].axis.clone(),
                        kind: TicketKind::Grilling,
                    },
                ],
                out_of_scope: vec![],
            })
        })
    }
}

struct MarkedSharpener;
impl Sharpener for MarkedSharpener {
    fn sharpen<'a>(
        &'a self,
        engine: &'a dyn Engine,
        class: RoleClass,
        _sampling: MicroSampling,
        candidate: &'a Candidate,
    ) -> BoxFuture<'a, Result<SharpenOutcome>> {
        Box::pin(async move {
            let request = marked_request(class, "STAGE5-SHARPEN-MARKER", &candidate.question);
            run_generation(engine, request).await?;
            let outcome = if candidate.name == "staleness policy" {
                SharpenOutcome::Fog
            } else {
                SharpenOutcome::Ticket
            };
            Ok(outcome)
        })
    }
}

struct MarkedSizer;
impl Sizer for MarkedSizer {
    fn size<'a>(
        &'a self,
        engine: &'a dyn Engine,
        class: RoleClass,
        _sampling: MicroSampling,
        candidate: &'a Candidate,
        _budget_tokens: usize,
    ) -> BoxFuture<'a, Result<SizeOutcome>> {
        Box::pin(async move {
            let request = marked_request(class, "STAGE6-SIZE-MARKER", &candidate.question);
            run_generation(engine, request).await?;
            Ok(SizeOutcome::Ok)
        })
    }
}

struct MarkedWirer;
impl Wirer for MarkedWirer {
    fn wire<'a>(
        &'a self,
        engine: &'a dyn Engine,
        class: RoleClass,
        _sampling: MicroSampling,
        ticket: &'a dark_plan::chart::ChartedTicket,
        _other_names: &'a [String],
    ) -> BoxFuture<'a, Result<WireAnswer>> {
        Box::pin(async move {
            let request = marked_request(class, "STAGE7-WIRE-MARKER", &ticket.question);
            run_generation(engine, request).await?;
            Ok(WireAnswer::default())
        })
    }
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn no_stage_prompt_carries_text_from_another_stage() {
    let engine = FakeEngine::new(Script {
        turns: vec![
            // Stage 1: destination.
            Turn {
                text: "DESTINATION: A retry policy for the pack fetcher.\nTYPE: decision\n"
                    .to_owned(),
                ..Default::default()
            },
            // Stage 3: one axis.
            Turn {
                text: "Retries need a cap.".to_owned(),
                ..Default::default()
            },
            // Stage 4: extract (MarkedExtractor's single call).
            Turn {
                text: "extracted".to_owned(),
                ..Default::default()
            },
            // Stage 5: sharpen, once per candidate (two candidates).
            Turn {
                text: "TICKET".to_owned(),
                ..Default::default()
            },
            Turn {
                text: "FOG".to_owned(),
                ..Default::default()
            },
            // Stage 6: size, once per sharp candidate (one candidate).
            Turn {
                text: "OK".to_owned(),
                ..Default::default()
            },
            // Stage 7: wire, once per ticket (one ticket).
            Turn {
                text: "NONE".to_owned(),
                ..Default::default()
            },
        ],
        ..Default::default()
    });

    let mut axis_sets = AxisSets::builtin();
    axis_sets.decision = AxisSet {
        axes: vec!["failure modes and error handling".to_owned()],
    };

    let store = FileCheckpointStore::new(std::env::temp_dir().join(format!(
        "dark-plan-stage-isolation-{}.jsonl",
        ulid::Ulid::new()
    )));

    let config = ChartConfig {
        role_class: RoleClass::Architect,
        sampling: StageSampling::default(),
        model_id: "fake/qwen3-32b".to_owned(),
        allow_charting: true,
        ticket_budget_tokens: 18_000,
    };

    let pipeline = ChartPipeline::new(&engine, config, &axis_sets, &store);
    let stages = StageImpls {
        extractor: &MarkedExtractor,
        sharpener: &MarkedSharpener,
        sizer: &MarkedSizer,
        wirer: &MarkedWirer,
    };

    let run = pipeline
        .chart(
            "map-isolation",
            DestinationInput {
                idea: "we need a retry policy",
                agents_md: "",
                repo_summary: "",
            },
            SeedReport::default(),
            &stages,
        )
        .await
        .expect("charting succeeds");

    assert!(
        matches!(run, ChartRun::Charted(_)),
        "the fixture has fog, so a map is charted"
    );

    let seen = engine.seen_requests();
    assert_eq!(
        seen.len(),
        7,
        "one request for each of the seven turns scripted above"
    );

    let markers = [
        "You settle the destination for a decision map.", // stage 1's real system prompt
        "You chart one axis of a decision map.",          // stage 3's real system prompt
        "STAGE4-EXTRACT-MARKER",
        "STAGE5-SHARPEN-MARKER",
        "STAGE6-SIZE-MARKER",
        "STAGE7-WIRE-MARKER",
    ];

    for (index, request) in seen.iter().enumerate() {
        // No stage's request grows: every request is exactly one system
        // message and one user message, never a carried-over transcript.
        assert_eq!(
            request.messages.len(),
            2,
            "request {index} carries {} messages; a growing count means a stage appended \
             an earlier stage's transcript instead of building fresh messages",
            request.messages.len()
        );

        let text: String = request
            .messages
            .iter()
            .map(dark_contract::Message::text_content)
            .collect::<Vec<_>>()
            .join("\n");

        let owning_markers: Vec<&str> = markers
            .iter()
            .filter(|marker| text.contains(*marker))
            .copied()
            .collect();
        assert!(
            owning_markers.len() <= 1,
            "request {index} contains more than one stage's marker text: {owning_markers:?}"
        );
    }

    // Each stage-specific marker appears in exactly the requests that stage
    // issued, and nowhere else.
    let stage5_marker_count = seen
        .iter()
        .filter(|request| {
            request
                .messages
                .iter()
                .any(|message| message.text_content().contains("STAGE5-SHARPEN-MARKER"))
        })
        .count();
    assert_eq!(
        stage5_marker_count, 2,
        "stage 5 ran once for each of the two candidates"
    );

    for (marker, expected_count) in [
        ("You settle the destination for a decision map.", 1),
        ("You chart one axis of a decision map.", 1),
        ("STAGE4-EXTRACT-MARKER", 1),
        ("STAGE6-SIZE-MARKER", 1),
        ("STAGE7-WIRE-MARKER", 1),
    ] {
        let count = seen
            .iter()
            .filter(|request| {
                request
                    .messages
                    .iter()
                    .any(|message| message.text_content().contains(marker))
            })
            .count();
        assert_eq!(
            count, expected_count,
            "unexpected number of requests carrying {marker:?}"
        );
    }
}
