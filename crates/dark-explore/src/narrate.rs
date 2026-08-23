//! The narration stage. Task unit `F5`.
//!
//! [`narrate`] turns a [`Document`](crate::output::Document) — the JSON
//! [`crate::output`] already wrote — into a short prose explanation, and
//! [`lint`] is the check that keeps that prose honest: it scans the
//! narration for anything that reads as a symbol or a data figure and
//! flags any that the JSON does not actually contain. The linter is the
//! substance of this task unit; see its section below.
//!
//! # `&dyn Engine`, not `dark-plan`'s helpers
//!
//! `crates/dark-plan/src/chart/sampling.rs` already carries a
//! `run_generation` helper that does exactly what this module needs: build
//! a [`Request`], drain the [`ChunkStream`] it returns, fold the chunks
//! into text. `dark-explore` cannot depend on it — Rule 16 says `dark-explore`
//! "reaches for no other workspace crate," `dark-contract` alone excepted —
//! so this module carries its own copy of the same small pattern, including
//! the same `std::future::poll_fn` trick that drains a
//! [`futures_core::stream::BoxStream`] without a `futures-util` dependency:
//! `ChunkStream`'s own type already names `Stream`, so `.poll_next` resolves
//! by method syntax with no `use` needed for the trait.
//!
//! # What "mark the narration as model-generated in the transcript" means
//! here
//!
//! Do step 3 of this task unit's brief says to mark the narration
//! model-generated in the transcript and show it beside the numbers. The
//! transcript itself — `dark_contract::Event`, and the turn loop that
//! writes to it — belongs to `dark-core`, a crate this one does not depend
//! on (Rule 16 again) and does not own. [`Narration::model_generated`] is
//! this module's own contribution to that requirement: a plain `bool`,
//! always `true`, that a caller in `dark-core` reads when it decides how to
//! label the message it puts on the transcript. Placing it beside the
//! actual numbers is `dark-core`'s job, not this module's.
//!
//! # "The JSON is the record"
//!
//! [`narrate`] takes `document: &Document` by reference and never consumes
//! or replaces it; [`Narration`] carries only the prose and the linter's
//! findings, never a copy of the report. A caller cannot reach this
//! module's narration without already holding the `Document` it explains,
//! and nothing here hands back anything that could stand in for it.

use dark_contract::{
    ChunkStream, Engine, ErrCode, Error, FinishReason, Message, Request, Result, Role, RoleClass,
    Sampling, ThinkMode,
};
use tokio_util::sync::CancellationToken;

use crate::output::Document;

/// How many entries of each ranked list (`seams`, `hotspots`, `modules`,
/// `bridges`) the budgeted extract keeps.
///
/// Do step 1 says "the JSON output or a budgeted extract of it." This
/// module always builds the extract, rather than switching representations
/// by size: a fixed item cap keeps the prompt — and so the micro-role's
/// token budget — the same size regardless of how large the repository is,
/// which a byte-length truncation of the full JSON would not guarantee
/// (and could truncate valid JSON mid-token besides).
pub const EXTRACT_MAX_ITEMS: usize = 8;

/// The narration micro-role's sampling settings.
///
/// `dark-qwen`'s `MicroRoles::narrate` names the canonical values — thinking
/// off, temperature 0.4, `top_p` 0.8, 200 tokens, no grammar — but this crate
/// cannot depend on `dark-qwen` either (Rule 16), so these are that
/// profile's numbers, copied rather than imported, the same way
/// `dark-plan`'s own `MicroSampling` copies them for its four micro-roles.
mod sampling {
    /// Generation temperature.
    pub(super) const TEMPERATURE: f32 = 0.4;
    /// Nucleus sampling threshold.
    pub(super) const TOP_P: f32 = 0.8;
    /// The generation limit.
    pub(super) const MAX_TOKENS: usize = 200;
}

/// Why one token in a narration was flagged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlagKind {
    /// The token reads as an identifier, a path, or a qualified name — a
    /// symbol — and the JSON does not contain it anywhere.
    Symbol,
    /// The token reads as a decimal figure, and the JSON does not contain
    /// it anywhere. See [`looks_like_data_number`] for exactly which
    /// numbers this covers, and which it deliberately does not.
    Number,
}

/// One token the linter could not find backing for in the JSON.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Flag {
    /// The flagged token, exactly as it appeared in the narration (after
    /// trimming the sentence punctuation at its edges).
    pub token: String,
    /// Why it was flagged.
    pub kind: FlagKind,
}

/// What [`narrate`] produced.
#[derive(Debug, Clone, PartialEq)]
pub struct Narration {
    /// The narration text.
    pub text: String,
    /// Always `true`. See the module documentation's note on why this
    /// crate carries no richer transcript-marking of its own.
    pub model_generated: bool,
    /// Every token [`lint`] could not find backing for in the full JSON.
    /// Empty when the narration cited nothing it should not have.
    pub flags: Vec<Flag>,
}

/// Builds the budgeted extract Do step 1 asks for: the summary stats, the
/// unresolved-reference count, and the first [`EXTRACT_MAX_ITEMS`] entries
/// of each ranked list, in the order [`Document`] already ranks them.
#[must_use]
fn budgeted_extract(document: &Document) -> serde_json::Value {
    serde_json::json!({
        "version": document.version,
        "stats": document.stats,
        "modules": document.modules.iter().take(EXTRACT_MAX_ITEMS).collect::<Vec<_>>(),
        "seams": document.seams.iter().take(EXTRACT_MAX_ITEMS).collect::<Vec<_>>(),
        "bridges": document.bridges.iter().take(EXTRACT_MAX_ITEMS).collect::<Vec<_>>(),
        "hotspots": document.hotspots.iter().take(EXTRACT_MAX_ITEMS).collect::<Vec<_>>(),
        "unresolved_refs": document.unresolved_refs,
    })
}

/// Builds the two-message prompt: Do step 2 requires the model to name the
/// JSON field behind every figure it states.
fn narrate_prompt(extract_json: &str) -> Vec<Message> {
    let system = "You explain a repository analysis to a person who has not read the raw JSON. \
                  Write two to four sentences of plain prose. For every figure or file you \
                  mention, name the JSON field it came from, in parentheses, right after it \
                  — for example \"crates/dark-core/src/session.rs is the busiest hotspot \
                  (hotspots[0].path)\". Only mention a file, a symbol, or a number that appears \
                  in the JSON you were given. Never invent a file, a module, or a figure that is \
                  not in it.";
    let user = format!(
        "Analysis JSON:\n{extract_json}\n\nWrite the narration now. Write nothing besides the \
         narration itself."
    );
    vec![
        Message::text(Role::System, system),
        Message::text(Role::User, user),
    ]
}

fn build_request(class: RoleClass, messages: Vec<Message>) -> Request {
    Request {
        think: ThinkMode::Off,
        sampling: Sampling {
            temperature: Some(sampling::TEMPERATURE),
            top_p: Some(sampling::TOP_P),
            ..Sampling::default()
        },
        max_tokens: sampling::MAX_TOKENS,
        ..Request::new(class, messages)
    }
}

/// Pulls one item out of a boxed stream without a `futures-util`
/// dependency. See the module documentation.
async fn next_chunk(stream: &mut ChunkStream) -> Option<Result<dark_contract::Chunk>> {
    std::future::poll_fn(|cx| stream.as_mut().poll_next(cx)).await
}

/// Runs one generation and folds its chunks into the visible text.
///
/// Mirrors `dark-plan`'s `chart::sampling::run_generation`, narrowed to
/// what this stage needs: no tool calls, no reasoning capture, no usage
/// bookkeeping.
async fn run_generation(engine: &dyn Engine, request: Request) -> Result<String> {
    let mut stream = engine.stream(request, CancellationToken::new()).await?;
    let mut text = String::new();
    let mut finish = FinishReason::Stop;

    while let Some(chunk) = next_chunk(&mut stream).await {
        match chunk? {
            dark_contract::Chunk::Text(part) => text.push_str(&part),
            dark_contract::Chunk::Done(reason) => {
                finish = reason;
                break;
            }
            dark_contract::Chunk::Reasoning(_)
            | dark_contract::Chunk::ToolCallDelta { .. }
            | dark_contract::Chunk::Usage(_)
            | dark_contract::Chunk::ModelLoading { .. } => {}
        }
    }

    if finish == FinishReason::Error {
        return Err(Error::new(
            ErrCode::EngineGenerate,
            "the engine ended the narration stream with an error and no Err chunk",
        ));
    }
    Ok(text)
}

/// Trims the sentence punctuation a token collects at its edges when a
/// narration is split on whitespace, without touching punctuation that is
/// part of the token itself (`::`, `_`, `.`, `/`).
fn trim_edges(token: &str) -> &str {
    token.trim_matches(|ch: char| {
        matches!(
            ch,
            '.' | ',' | ';' | ':' | '!' | '?' | '"' | '\'' | '(' | ')' | '[' | ']' | '{' | '}'
        )
    })
}

/// The file extensions [`looks_like_symbol`] recognises as code, from F1's
/// thirteen supported languages (`crate::syntax::Language`) plus the common
/// header/source split C and C++ use.
const CODE_EXTENSIONS: &[&str] = &[
    "rs", "go", "ts", "tsx", "js", "jsx", "mjs", "cjs", "py", "pyi", "java", "cs", "rb", "c", "h",
    "cpp", "cc", "cxx", "hpp", "hh", "hxx", "sql", "md",
];

/// Whether `token` ends in a known code file extension, with at least one
/// character before the dot.
fn has_code_extension(token: &str) -> bool {
    let Some(dot) = token.rfind('.') else {
        return false;
    };
    let (stem, ext) = token.split_at(dot);
    !stem.is_empty() && CODE_EXTENSIONS.contains(&&ext[1..])
}

/// Whether `token` reads as a repository path: more than one `/`, or one
/// `/` alongside a file extension. One bare `/` with neither (`and/or`,
/// `he/she`) is left alone — see the module documentation's note on this
/// boundary.
fn has_path_shape(token: &str) -> bool {
    let slashes = token.matches('/').count();
    slashes > 1 || (slashes == 1 && token.contains('.'))
}

/// Whether `token` reads as a `snake_case` identifier: an underscore with
/// an ASCII letter somewhere in the token.
fn has_snake_case(token: &str) -> bool {
    token.contains('_') && token.chars().any(|ch| ch.is_ascii_alphabetic())
}

/// Whether `token` reads as a `camelCase` or `PascalCase` identifier: a
/// lowercase letter immediately followed by an uppercase one. This misses
/// nothing `snake_case` or a bare capitalised English word would trip: a
/// capitalised word on its own ("Modularity") has no such hump, and an
/// all-uppercase acronym ("JSON") has no lowercase letter to hump from.
fn has_camel_hump(token: &str) -> bool {
    token
        .as_bytes()
        .windows(2)
        .any(|pair| pair[0].is_ascii_lowercase() && pair[1].is_ascii_uppercase())
}

/// Whether `token` looks like an identifier, a path, or a qualified name
/// rather than an ordinary English word.
fn looks_like_symbol(token: &str) -> bool {
    token.contains("::")
        || has_path_shape(token)
        || has_snake_case(token)
        || has_camel_hump(token)
        || has_code_extension(token)
}

/// Whether `token` is a plain decimal figure: digits, one `.`, digits, an
/// optional leading `-`.
///
/// # The boundary this deliberately does not cross
///
/// A bare integer is not checked. "the top 5", "all 3 seams", "1st",
/// "one of 12" — an ordinal or a plain count in ordinary prose is
/// indistinguishable, by this linter, from a genuine data figure that
/// happens to be a whole number, and flagging every bare integer would flag
/// far more prose than data. A figure with a fractional part (`0.61`) is
/// the boundary that stays inside what this linter checks: prose produces
/// an accidental decimal far more rarely than it produces an accidental
/// integer, so a `0.61` a narration states is almost always a genuine data
/// figure the JSON should contain, or should not.
fn looks_like_data_number(token: &str) -> bool {
    let mut parts = token.split('.');
    let Some(int_part) = parts.next() else {
        return false;
    };
    let Some(frac_part) = parts.next() else {
        return false;
    };
    if parts.next().is_some() {
        return false;
    }
    let digits = int_part.strip_prefix('-').unwrap_or(int_part);
    !digits.is_empty()
        && digits.bytes().all(|b| b.is_ascii_digit())
        && !frac_part.is_empty()
        && frac_part.bytes().all(|b| b.is_ascii_digit())
}

/// Scans `narration` for every token that looks like a symbol or a data
/// figure and returns the ones `json_text` does not contain anywhere.
///
/// `json_text` is the *full* report, not the budgeted extract the model
/// saw: a token is "invented" when the JSON has no record of it at all, not
/// merely when it fell outside the top [`EXTRACT_MAX_ITEMS`] entries this
/// stage happened to show the model.
#[must_use]
pub fn lint(narration: &str, json_text: &str) -> Vec<Flag> {
    let mut flags = Vec::new();
    let mut already_flagged = std::collections::HashSet::new();

    for raw in narration.split_whitespace() {
        let token = trim_edges(raw);
        if token.is_empty() || json_text.contains(token) {
            continue;
        }

        let kind = if looks_like_symbol(token) {
            FlagKind::Symbol
        } else if looks_like_data_number(token) {
            FlagKind::Number
        } else {
            continue;
        };

        if already_flagged.insert(token.to_owned()) {
            flags.push(Flag {
                token: token.to_owned(),
                kind,
            });
        }
    }

    flags
}

/// Explains `document` in prose, and lints the result.
///
/// # Errors
///
/// Returns an error when the engine fails to start or run the generation.
/// See [`dark_contract::Engine::stream`].
pub async fn narrate(
    engine: &dyn Engine,
    class: RoleClass,
    document: &Document,
) -> Result<Narration> {
    let extract = budgeted_extract(document);
    let extract_json = serde_json::to_string_pretty(&extract).map_err(|err| {
        Error::new(
            ErrCode::ExploreParse,
            format!("cannot build the narration extract: {err}"),
        )
    })?;
    let full_json = serde_json::to_string_pretty(document).map_err(|err| {
        Error::new(
            ErrCode::ExploreParse,
            format!("cannot serialise the report for the narration linter: {err}"),
        )
    })?;

    let request = build_request(class, narrate_prompt(&extract_json));
    let text = run_generation(engine, request).await?;
    let flags = lint(&text, &full_json);

    Ok(Narration {
        text,
        model_generated: true,
        flags,
    })
}

#[cfg(test)]
#[allow(clippy::float_cmp)]
mod tests {
    use dark_engine_fake::script::Turn;
    use dark_engine_fake::{FakeEngine, Script};

    use super::*;
    use crate::discover::DiscoverOptions;
    use crate::output::{self, Sources};
    use crate::seam::{CoChange, Weights};

    fn sample_document() -> Document {
        let graphs = crate::graph::build(&[]);
        let analysis =
            crate::seam::analyse(&graphs, &CoChange::default(), &Weights::default()).unwrap();
        let discover_options = DiscoverOptions::default();
        let weights = Weights::default();
        output::build(&Sources {
            files: &[],
            graphs: &graphs,
            analysis: &analysis,
            cochange: &CoChange::default(),
            discover_options: &discover_options,
            weights: &weights,
            tree_sha: blake3::hash(b"narrate-fixture"),
        })
    }

    fn engine_saying(text: &str) -> FakeEngine {
        FakeEngine::new(Script {
            turns: vec![Turn {
                text: text.to_owned(),
                ..Default::default()
            }],
            ..Default::default()
        })
    }

    #[tokio::test]
    async fn narrate_marks_its_output_as_model_generated() {
        let document = sample_document();
        let engine = engine_saying("This repository has no files yet (stats.files).");

        let narration = narrate(&engine, RoleClass::Scout, &document).await.unwrap();
        assert!(narration.model_generated);
        assert_eq!(
            narration.text,
            "This repository has no files yet (stats.files)."
        );
    }

    #[tokio::test]
    async fn narrate_sends_the_budgeted_extract_and_names_the_micro_role_settings() {
        let document = sample_document();
        let engine = engine_saying("noted (stats.files)");

        narrate(&engine, RoleClass::Scout, &document).await.unwrap();

        let seen = engine.seen_requests();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].think, ThinkMode::Off);
        assert_eq!(seen[0].sampling.temperature, Some(0.4));
        assert_eq!(seen[0].max_tokens, 200);
        let text: String = seen[0]
            .messages
            .iter()
            .map(dark_contract::Message::text_content)
            .collect();
        assert!(text.contains("\"unresolved_refs\""));
    }

    /// The task unit's own "done when": the linter flags an invented
    /// symbol in a test fixture.
    #[tokio::test]
    async fn narrate_flags_an_invented_symbol_the_json_does_not_contain() {
        let document = sample_document();
        let engine = engine_saying(
            "The busiest file is crates/phantom_module/src/invented_hotspot.rs (hotspots[0].path).",
        );

        let narration = narrate(&engine, RoleClass::Scout, &document).await.unwrap();

        assert!(
            narration
                .flags
                .iter()
                .any(|flag| flag.token.contains("invented_hotspot.rs")),
            "flags: {:?}",
            narration.flags
        );
    }

    #[test]
    fn lint_does_not_flag_a_symbol_the_json_actually_contains() {
        let json = r#"{"hotspots":[{"path":"crates/dark-core/src/session.rs","Ca":41}]}"#;
        let narration =
            "crates/dark-core/src/session.rs is the busiest hotspot (hotspots[0].path).";
        assert!(lint(narration, json).is_empty());
    }

    #[test]
    fn lint_flags_a_snake_case_identifier_the_json_does_not_contain() {
        let json = r#"{"stats":{"files":10}}"#;
        let narration = "The function invented_helper_fn drives most of the churn.";
        let flags = lint(narration, json);
        assert_eq!(flags.len(), 1);
        assert_eq!(flags[0].token, "invented_helper_fn");
        assert_eq!(flags[0].kind, FlagKind::Symbol);
    }

    #[test]
    fn lint_flags_a_qualified_path_the_json_does_not_contain() {
        let json = r#"{"stats":{"files":10}}"#;
        let narration = "See dark_core::turn::run for the entry point.";
        let flags = lint(narration, json);
        assert!(
            flags
                .iter()
                .any(|f| f.token.contains("dark_core::turn::run"))
        );
    }

    #[test]
    fn lint_flags_a_decimal_figure_the_json_does_not_contain() {
        let json = r#"{"stats":{"modularity":0.61}}"#;
        let narration = "Modularity comes out to 0.93 (stats.modularity).";
        let flags = lint(narration, json);
        assert_eq!(flags.len(), 1);
        assert_eq!(flags[0].token, "0.93");
        assert_eq!(flags[0].kind, FlagKind::Number);
    }

    #[test]
    fn lint_does_not_flag_a_decimal_figure_the_json_contains() {
        let json = r#"{"stats":{"modularity":0.61}}"#;
        let narration = "Modularity comes out to 0.61 (stats.modularity).";
        assert!(lint(narration, json).is_empty());
    }

    #[test]
    fn lint_does_not_flag_ordinary_prose_or_bare_integers() {
        let json = r#"{"stats":{"files":10}}"#;
        let narration = "There are 10 files, and the top 5 hotspots matter most, per usual.";
        assert!(
            lint(narration, json).is_empty(),
            "flags: {:?}",
            lint(narration, json)
        );
    }

    #[test]
    fn lint_does_not_flag_a_slash_conjunction_as_a_path() {
        let json = r#"{"stats":{"files":10}}"#;
        let narration = "Each edge is scored true/false and read either/or, and/or both ways.";
        assert!(
            lint(narration, json).is_empty(),
            "flags: {:?}",
            lint(narration, json)
        );
    }

    #[test]
    fn lint_does_not_flag_a_plain_capitalised_word_or_an_acronym() {
        let json = r#"{"stats":{"files":10}}"#;
        let narration = "The JSON output has Modularity and Instability figures.";
        assert!(
            lint(narration, json).is_empty(),
            "flags: {:?}",
            lint(narration, json)
        );
    }

    #[test]
    fn lint_flags_a_code_file_the_json_does_not_contain() {
        let json = r#"{"stats":{"files":10}}"#;
        let narration = "Most churn sits in nonexistent.rs this cycle.";
        let flags = lint(narration, json);
        assert_eq!(flags.len(), 1);
        assert_eq!(flags[0].token, "nonexistent.rs");
        assert_eq!(flags[0].kind, FlagKind::Symbol);
    }

    #[test]
    fn lint_reports_a_repeated_invented_token_once() {
        let json = r#"{"stats":{"files":10}}"#;
        let narration = "invented_symbol appears twice: once here, and invented_symbol again.";
        assert_eq!(lint(narration, json).len(), 1);
    }

    #[test]
    fn trim_edges_strips_sentence_punctuation_but_not_internal_characters() {
        assert_eq!(trim_edges("session.rs."), "session.rs");
        assert_eq!(trim_edges("(dark_core::turn)"), "dark_core::turn");
        assert_eq!(trim_edges("\"invented_fn\","), "invented_fn");
    }
}
