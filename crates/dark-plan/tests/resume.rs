//! Task unit `E1`'s "Done when": "A charting run killed at stage 5 resumes
//! and produces the same map as an uninterrupted run with the same seed."
//!
//! This test drives [`ChartPipeline`] through the public API (unlike the
//! resume test inside `chart::pipeline`'s own unit tests, which reaches
//! into the module directly), across three separate `FakeEngine`
//! instances sharing one on-disk [`FileCheckpointStore`]:
//!
//! 1. A run that fails partway through stage 5 (sharpen) — a scripted
//!    engine error stands in for a crash — checkpointing stages 1 to 4
//!    first.
//! 2. A `resume` call, on a fresh engine holding only the turns stage 5
//!    onward still needs, using the same checkpoint file.
//! 3. An independent, uninterrupted run of the same fixture, for
//!    comparison.
//!
//! Ticket identifiers are `ulid::Ulid` values minted fresh wherever stage 6
//! (size) actually runs, and stage 6 has not run yet when this fixture's
//! "crash" happens, so the resumed run and the uninterrupted run each mint
//! their own. The comparison below is therefore over decisions — the same
//! destination, the same tickets by name and content, the same fog — not
//! over bit-identical identifiers; see the note in
//! `dark_plan::chart::pipeline::ChartPipeline::run` (private) for why
//! identifier assignment sits at stage 6 rather than after stage 7.

use dark_contract::{Engine, ErrCode, Message, Result, Role, RoleClass};
use dark_engine_fake::script::{ScriptedError, Turn};
use dark_engine_fake::{FakeEngine, Script};
use dark_plan::axes::{AxisAnswer, AxisSet, AxisSets};
use dark_plan::chart::sampling::BoxFuture;
use dark_plan::chart::{
    Candidate, ChartConfig, ChartOutput, ChartPipeline, ChartRun, ChartedEdge, ChartedTicket,
    CheckpointStore, DestinationInput, ExtractOutput, Extractor, FileCheckpointStore,
    MicroSampling, SeedReport, SharpenOutcome, Sharpener, SizeOutcome, Sizer, Stage, StageImpls,
    StageSampling, TicketKind, WireAnswer, Wirer, build_request, run_generation,
};

fn one_call_request(class: RoleClass, body: &str) -> dark_contract::Request {
    let messages = vec![
        Message::text(Role::System, "test stage"),
        Message::text(Role::User, body),
    ];
    build_request(class, messages, MicroSampling::classify())
}

/// A fixed extractor: always the same two candidates, regardless of what
/// the axis answers actually say. Still makes one engine call, so it
/// consumes the script turn a real extractor would.
struct FixedExtractor;
impl Extractor for FixedExtractor {
    fn extract<'a>(
        &'a self,
        engine: &'a dyn Engine,
        class: RoleClass,
        _sampling: MicroSampling,
        _destination: &'a str,
        _answers: &'a [AxisAnswer],
    ) -> BoxFuture<'a, Result<ExtractOutput>> {
        Box::pin(async move {
            run_generation(engine, one_call_request(class, "extract")).await?;
            Ok(ExtractOutput {
                candidates: vec![
                    Candidate {
                        name: "retry cap".to_owned(),
                        question: "What is the retry cap?".to_owned(),
                        axis: "failure modes and error handling".to_owned(),
                        kind: TicketKind::Task,
                    },
                    Candidate {
                        name: "staleness policy".to_owned(),
                        question: "How is staleness declared?".to_owned(),
                        axis: "failure modes and error handling".to_owned(),
                        kind: TicketKind::Grilling,
                    },
                ],
                out_of_scope: vec![],
            })
        })
    }
}

/// Classifies by a fixed name: "staleness policy" is fog, everything else
/// is a ticket. One engine call per candidate.
struct FixedSharpener;
impl Sharpener for FixedSharpener {
    fn sharpen<'a>(
        &'a self,
        engine: &'a dyn Engine,
        class: RoleClass,
        _sampling: MicroSampling,
        candidate: &'a Candidate,
    ) -> BoxFuture<'a, Result<SharpenOutcome>> {
        Box::pin(async move {
            run_generation(engine, one_call_request(class, &candidate.question)).await?;
            Ok(if candidate.name == "staleness policy" {
                SharpenOutcome::Fog
            } else {
                SharpenOutcome::Ticket
            })
        })
    }
}

/// Always accepts. One engine call per candidate.
struct AlwaysOkSizer;
impl Sizer for AlwaysOkSizer {
    fn size<'a>(
        &'a self,
        engine: &'a dyn Engine,
        class: RoleClass,
        _sampling: MicroSampling,
        candidate: &'a Candidate,
        _budget_tokens: usize,
    ) -> BoxFuture<'a, Result<SizeOutcome>> {
        Box::pin(async move {
            run_generation(engine, one_call_request(class, &candidate.question)).await?;
            Ok(SizeOutcome::Ok)
        })
    }
}

/// Always answers `NONE`. One engine call per ticket.
struct NoBlockersWirer;
impl Wirer for NoBlockersWirer {
    fn wire<'a>(
        &'a self,
        engine: &'a dyn Engine,
        class: RoleClass,
        _sampling: MicroSampling,
        ticket: &'a ChartedTicket,
        _other_names: &'a [String],
    ) -> BoxFuture<'a, Result<WireAnswer>> {
        Box::pin(async move {
            run_generation(engine, one_call_request(class, &ticket.question)).await?;
            Ok(WireAnswer::default())
        })
    }
}

fn axis_sets_fixture() -> AxisSets {
    let mut axis_sets = AxisSets::builtin();
    axis_sets.decision = AxisSet {
        axes: vec!["failure modes and error handling".to_owned()],
    };
    axis_sets
}

fn config_fixture() -> ChartConfig {
    ChartConfig {
        role_class: RoleClass::Architect,
        sampling: StageSampling::default(),
        model_id: "fake/qwen3-32b".to_owned(),
        allow_charting: true,
        ticket_budget_tokens: 18_000,
    }
}

fn destination_turn() -> Turn {
    Turn {
        text: "DESTINATION: A retry policy for the pack fetcher.\nTYPE: decision\n".to_owned(),
        ..Default::default()
    }
}

fn axis_turn() -> Turn {
    Turn {
        text: "Retries are undecided; staleness is also undecided.".to_owned(),
        ..Default::default()
    }
}

fn stage_impls() -> StageImpls<'static> {
    StageImpls {
        extractor: &FixedExtractor,
        sharpener: &FixedSharpener,
        sizer: &AlwaysOkSizer,
        wirer: &NoBlockersWirer,
    }
}

/// Projects a ticket list to the fields that must match once identifiers
/// (necessarily fresh per run) are set aside.
fn ticket_content(
    tickets: &[ChartedTicket],
) -> Vec<(String, String, TicketKind, bool, i64, Vec<String>)> {
    tickets
        .iter()
        .map(|ticket| {
            (
                ticket.name.clone(),
                ticket.question.clone(),
                ticket.ticket_type,
                ticket.hitl,
                ticket.ordinal,
                ticket.axis.clone(),
            )
        })
        .collect()
}

fn edges_by_name(tickets: &[ChartedTicket], edges: &[ChartedEdge]) -> Vec<(String, String)> {
    let name_of = |id: &str| -> String {
        tickets
            .iter()
            .find(|ticket| ticket.id == id)
            .map_or_else(String::new, |ticket| ticket.name.clone())
    };
    let mut pairs: Vec<(String, String)> = edges
        .iter()
        .map(|edge| (name_of(&edge.blocker), name_of(&edge.blocked)))
        .collect();
    pairs.sort();
    pairs
}

fn assert_same_decisions(left: &ChartOutput, right: &ChartOutput) {
    assert_eq!(left.destination, right.destination);
    assert_eq!(left.seed, right.seed);
    assert_eq!(left.axis_answers, right.axis_answers);
    assert_eq!(left.out_of_scope, right.out_of_scope);
    assert_eq!(left.fog, right.fog);
    assert_eq!(
        ticket_content(&left.tickets),
        ticket_content(&right.tickets)
    );
    assert_eq!(
        edges_by_name(&left.tickets, &left.edges),
        edges_by_name(&right.tickets, &right.edges)
    );
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn a_run_killed_partway_through_stage_5_resumes_to_the_same_map() {
    let axis_sets = axis_sets_fixture();
    let config = config_fixture();
    let map_id = format!("map-resume-{}", ulid::Ulid::new());

    // Phase 1: run until a scripted error simulates a crash partway
    // through stage 5 (sharpen) — the first sharpen call fails.
    let killed_store_path =
        std::env::temp_dir().join(format!("dark-plan-resume-test-{}.jsonl", ulid::Ulid::new()));
    let killed_store = FileCheckpointStore::new(&killed_store_path);
    let killed_engine = FakeEngine::new(Script {
        turns: vec![
            destination_turn(), // stage 1
            axis_turn(),        // stage 3
            Turn {
                text: "extract".to_owned(),
                ..Default::default()
            }, // stage 4
            Turn {
                text: String::new(),
                error: Some(ScriptedError {
                    code: "E_ENGINE_GENERATE".to_owned(),
                    message: "simulated crash".to_owned(),
                    after_chunks: 0,
                }),
                ..Default::default()
            }, // stage 5, first call: fails
        ],
        ..Default::default()
    });
    let killed_pipeline =
        ChartPipeline::new(&killed_engine, config.clone(), &axis_sets, &killed_store);
    let stages = stage_impls();

    let killed_result = killed_pipeline
        .chart(
            &map_id,
            DestinationInput {
                idea: "we need a retry policy",
                agents_md: "",
                repo_summary: "",
            },
            SeedReport::default(),
            &stages,
        )
        .await;
    let err = killed_result.expect_err("the scripted error must surface");
    assert_eq!(err.code, ErrCode::EngineGenerate);

    // Stages 1 to 4 checkpointed; stage 5 did not.
    let recorded = killed_store.load(&map_id).unwrap();
    let recorded_stages: Vec<Stage> = recorded.iter().map(|checkpoint| checkpoint.stage).collect();
    assert!(recorded_stages.contains(&Stage::Destination));
    assert!(recorded_stages.contains(&Stage::Seed));
    assert!(recorded_stages.contains(&Stage::AxisSweep));
    assert!(recorded_stages.contains(&Stage::Extract));
    assert!(!recorded_stages.contains(&Stage::Sharpen));

    // Phase 2: resume, on a fresh engine holding only the turns stage 5
    // onward needs: two sharpen calls, one size call, one wire call.
    let resume_engine = FakeEngine::new(Script {
        turns: vec![
            Turn {
                text: "TICKET".to_owned(),
                ..Default::default()
            }, // sharpen candidate 1
            Turn {
                text: "FOG".to_owned(),
                ..Default::default()
            }, // sharpen candidate 2
            Turn {
                text: "OK".to_owned(),
                ..Default::default()
            }, // size
            Turn {
                text: "NONE".to_owned(),
                ..Default::default()
            }, // wire
        ],
        ..Default::default()
    });
    let resume_store = FileCheckpointStore::new(&killed_store_path); // same file
    let resume_pipeline =
        ChartPipeline::new(&resume_engine, config.clone(), &axis_sets, &resume_store);
    let resumed_run = resume_pipeline
        .resume(&map_id, SeedReport::default(), &stages)
        .await
        .expect("resume completes using only the remaining turns");

    assert_eq!(
        resume_engine.turns_played(),
        4,
        "resume must not re-run stages 1 to 4, which already have checkpoints"
    );

    let ChartRun::Charted(resumed_output) = resumed_run else {
        panic!("expected a charted map");
    };

    // Phase 3: an independent, uninterrupted run of the same fixture.
    let full_store = FileCheckpointStore::new(std::env::temp_dir().join(format!(
        "dark-plan-resume-test-full-{}.jsonl",
        ulid::Ulid::new()
    )));
    let full_engine = FakeEngine::new(Script {
        turns: vec![
            destination_turn(),
            axis_turn(),
            Turn {
                text: "extract".to_owned(),
                ..Default::default()
            },
            Turn {
                text: "TICKET".to_owned(),
                ..Default::default()
            },
            Turn {
                text: "FOG".to_owned(),
                ..Default::default()
            },
            Turn {
                text: "OK".to_owned(),
                ..Default::default()
            },
            Turn {
                text: "NONE".to_owned(),
                ..Default::default()
            },
        ],
        ..Default::default()
    });
    let full_pipeline = ChartPipeline::new(&full_engine, config, &axis_sets, &full_store);
    let full_run = full_pipeline
        .chart(
            &format!("map-full-{}", ulid::Ulid::new()),
            DestinationInput {
                idea: "we need a retry policy",
                agents_md: "",
                repo_summary: "",
            },
            SeedReport::default(),
            &stages,
        )
        .await
        .expect("the uninterrupted run charts");
    let ChartRun::Charted(full_output) = full_run else {
        panic!("expected a charted map");
    };

    assert_same_decisions(&resumed_output, &full_output);
}
