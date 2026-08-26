//! `dark plan`: chart a map, and work it (task units `E1` to `E7`).
//!
//! # Why this lives in the composition root
//!
//! `dark-plan` depends on `dark-contract` alone. It holds the charting
//! pipeline and knows nothing about where a repository analysis is stored
//! or how a map is written down. `dark-explore` produces the analysis and
//! reaches for no other workspace crate (Rule 16). `dark-cartograph`
//! stores the map and does the same. Three crates that must not see each
//! other, and one command that needs all three — so the join is here, the
//! same role `crate::scrape`, `crate::pack`, and `crate::fogmap` already
//! play.
//!
//! # The seed
//!
//! Stage 2 of charting reads the repository and produces "seams, blast
//! radius, module list" with no model involved (task unit `E1`). That is
//! exactly `dark explore`'s output, so this reads the report rather than
//! recomputing it. When no current report exists, charting says so and
//! stops: running a minute of analysis inside another command hides where
//! the time went, and `dark explore` is one command away.
//!
//! Nothing here opens a network connection.

use std::io::Write as _;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use dark_cartograph::journal::{
    self, JournalEvent, MapCreated, MapStatus, TicketCreated, TicketStatus, TicketType, Timestamp,
};
use dark_cartograph::store::Store;
use dark_contract::{ErrCode, Error, RoleClass};
use dark_plan::axes::AxisSets;
use dark_plan::chart::{
    ChartConfig, ChartOutput, ChartPipeline, ChartRun, Checkpoint, CheckpointStore,
    DestinationInput, SeedModule, SeedReport, SeedSeam, StageImpls,
};
use dark_plan::extract::DefaultExtractor;
use dark_plan::sharpen::DefaultSharpener;
use dark_plan::size::DefaultSizer;
use dark_plan::wire::DefaultWirer;
use ulid::Ulid;

use crate::PlanAction;
use crate::explore::Report;
use crate::profile::{Mode, Profile};

/// How many seams the seed carries into charting.
///
/// The whole ranked list would crowd out the destination in an 18k
/// context. The top few are the ones a person would look at first, and
/// stage 2's job is to orient the model, not to hand it the analysis.
const SEED_SEAMS: usize = 12;

/// How many modules the seed carries.
const SEED_MODULES: usize = 40;

/// How many hotspots the repository summary names.
const SUMMARY_HOTSPOTS: usize = 5;

/// Runs the `dark plan` subcommand named by `action`.
///
/// # Errors
///
/// Returns an error when the repository cannot be read, when no analysis
/// exists yet, when the engine fails, or when the map cannot be written.
pub(crate) fn run_command(action: PlanAction) -> Result<()> {
    match action {
        PlanAction::Chart { idea } => chart(&idea, false),
        PlanAction::Resume { idea } => chart(&idea, true),
        PlanAction::Work { ticket } => work(ticket.as_deref()),
    }
}

// ------------------------------------------------------------ checkpoints ---

/// A [`CheckpointStore`] that writes one JSON object per line under
/// `$DARK_HOME/maps/<map-id>/checkpoints.jsonl`.
///
/// Beside the map's own journal, and in the same shape: a charting run
/// killed at stage 5 must resume from what is on disk, and one appended
/// line per finished stage is the smallest thing that survives a crash
/// mid-write — a truncated final line loses that stage, never the ones
/// before it.
struct FileCheckpoints {
    /// `$DARK_HOME/maps`.
    maps_root: PathBuf,
}

impl FileCheckpoints {
    /// The file holding `map_id`'s checkpoints.
    fn path(&self, map_id: &str) -> PathBuf {
        self.maps_root.join(map_id).join("checkpoints.jsonl")
    }
}

/// Builds the error a checkpoint read or write failure reports.
fn checkpoint_failed(message: String) -> Error {
    Error::new(ErrCode::ToolFailed, message)
        .with_remedy("Check the permissions on $DARK_HOME/maps.")
}

impl CheckpointStore for FileCheckpoints {
    fn record(&self, checkpoint: &Checkpoint) -> dark_contract::Result<()> {
        let path = self.path(&checkpoint.map_id);
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|err| {
                checkpoint_failed(format!("cannot create {}: {err}", dir.display()))
            })?;
        }
        let mut line = serde_json::to_string(checkpoint)
            .map_err(|err| checkpoint_failed(format!("cannot serialise a checkpoint: {err}")))?;
        line.push('\n');

        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|err| checkpoint_failed(format!("cannot open {}: {err}", path.display())))?;
        file.write_all(line.as_bytes())
            .map_err(|err| checkpoint_failed(format!("cannot write {}: {err}", path.display())))?;
        file.sync_all()
            .map_err(|err| checkpoint_failed(format!("cannot flush {}: {err}", path.display())))
    }

    fn load(&self, map_id: &str) -> dark_contract::Result<Vec<Checkpoint>> {
        let path = self.path(map_id);
        if !path.is_file() {
            return Ok(Vec::new());
        }
        let text = std::fs::read_to_string(&path)
            .map_err(|err| checkpoint_failed(format!("cannot read {}: {err}", path.display())))?;

        let mut out = Vec::new();
        for (index, line) in text.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            match serde_json::from_str::<Checkpoint>(line) {
                Ok(checkpoint) => out.push(checkpoint),
                // A truncated final line is a crash mid-write, and the
                // stages before it are still good. Any earlier line
                // failing to parse is corruption worth reporting.
                Err(err) if index + 1 == text.lines().count() => {
                    let _ = err;
                    break;
                }
                Err(err) => {
                    return Err(checkpoint_failed(format!(
                        "line {} of {} is not a checkpoint: {err}",
                        index + 1,
                        path.display()
                    )));
                }
            }
        }
        Ok(out)
    }
}

// ------------------------------------------------------------------ seed ---

/// Builds stage 2's seed from an analysis `dark explore` already wrote.
///
/// `blast_radius` stays `None`: a fresh destination names no symbol yet,
/// which is the case [`SeedReport::blast_radius`] documents.
#[must_use]
pub(crate) fn seed_from(report: &Report) -> SeedReport {
    SeedReport {
        seams: report
            .seams
            .iter()
            .take(SEED_SEAMS)
            .map(|seam| SeedSeam {
                from: seam.from.clone(),
                to: seam.to.clone(),
                #[allow(
                    clippy::cast_possible_truncation,
                    reason = "a seam score is in 0.0..=1.0, far inside f32's exact range"
                )]
                score: seam.score as f32,
                hard: seam.hard,
            })
            .collect(),
        blast_radius: None,
        modules: report
            .modules
            .iter()
            .take(SEED_MODULES)
            .map(|module| SeedModule {
                path: module.path.clone(),
                incoming: module.ca,
                outgoing: module.ce,
            })
            .collect(),
    }
}

/// Builds the repository summary stage 1 reads.
///
/// Every line is counted, not written by a model: the summary a
/// destination is settled against must be the same summary tomorrow, and
/// a model asked to describe a repository twice does not oblige. `dark
/// extend` writes the prose half separately, and says which half is which.
#[must_use]
pub(crate) fn repo_summary(report: &Report, profile: Option<&Profile>) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();

    // What `dark extend` or `dark refactor` decided, first: it changes
    // what the destination should even be. Charting a refactor towards a
    // new language against a summary of the old one settles the wrong
    // destination entirely.
    if let Some(profile) = profile {
        match profile.mode {
            Mode::Extend => {
                if let Some(language) = &profile.language {
                    let _ = writeln!(
                        out,
                        "This repository is being extended. Keep writing {language}, and \
                         follow the conventions in AGENTS.md."
                    );
                }
            }
            Mode::Refactor => {
                let target = profile
                    .target_language
                    .as_deref()
                    .unwrap_or("the same language");
                let _ = write!(out, "This repository is being refactored to {target}");
                match &profile.pattern {
                    Some(pattern) => {
                        let _ = writeln!(out, ", towards a {pattern} architecture.");
                    }
                    None => {
                        let _ = writeln!(out, ".");
                    }
                }
                if let Some(from) = &profile.language {
                    let _ = writeln!(
                        out,
                        "The existing code is {from}; read it for behaviour, not for style."
                    );
                }
            }
        }
        if !profile.is_current(&report.tree_sha) {
            let _ = writeln!(
                out,
                "(That decision was made against an older commit; the modules below may have \
                 moved since.)"
            );
        }
        let _ = writeln!(out);
    }

    let _ = writeln!(
        out,
        "{} files, {} definitions, {} file-level dependencies.",
        report.stats.files, report.stats.defs, report.stats.edges_f
    );

    if !report.modules.is_empty() {
        let communities = report
            .modules
            .iter()
            .map(|module| module.community)
            .collect::<std::collections::BTreeSet<_>>()
            .len();
        let _ = writeln!(
            out,
            "{} modules in {communities} communities (modularity {:.2}).",
            report.modules.len(),
            report.stats.modularity
        );

        let mut largest: Vec<&crate::explore::ReportModule> = report.modules.iter().collect();
        largest.sort_by(|a, b| b.files.cmp(&a.files).then_with(|| a.path.cmp(&b.path)));
        let named: Vec<String> = largest
            .iter()
            .take(SUMMARY_HOTSPOTS)
            .map(|module| format!("{} ({} files)", module.path, module.files))
            .collect();
        if !named.is_empty() {
            let _ = writeln!(out, "Largest: {}.", named.join(", "));
        }
    }

    if !report.hotspots.is_empty() {
        let named: Vec<String> = report
            .hotspots
            .iter()
            .take(SUMMARY_HOTSPOTS)
            .map(|hotspot| format!("{} ({} importers)", hotspot.path, hotspot.ca))
            .collect();
        let _ = writeln!(out, "Most depended on: {}.", named.join(", "));
    }

    if let Some(seam) = report.seams.first() {
        let _ = writeln!(
            out,
            "Strongest boundary: {} to {} (score {:.2}).",
            seam.from, seam.to, seam.score
        );
    }

    out
}

// ------------------------------------------------------------- write map ---

/// Converts a charted ticket kind to the store's own.
const fn store_type(kind: dark_plan::chart::TicketKind) -> TicketType {
    match kind {
        dark_plan::chart::TicketKind::Research => TicketType::Research,
        dark_plan::chart::TicketKind::Prototype => TicketType::Prototype,
        dark_plan::chart::TicketKind::Grilling => TicketType::Grilling,
        dark_plan::chart::TicketKind::Task => TicketType::Task,
    }
}

/// Records one event: durably to the journal first, then into the store.
///
/// The journal is the source of truth (`dark map rebuild` replays it), so
/// it is written first — a store row with no journal line behind it would
/// vanish on the next rebuild.
fn record(store: &mut Store, maps_root: &Path, map_id: &str, event: &JournalEvent) -> Result<()> {
    journal::append(maps_root, map_id, event).map_err(crate::contract_error)?;
    store.apply(event).map_err(crate::contract_error)
}

/// Resolves the instruction chain stage 1 reads, or an empty string.
///
/// A repository with no `AGENTS.md` charts perfectly well; the stage
/// prompt simply has one fewer input. A chain that will not resolve is
/// the same case from charting's point of view, so it is not an error
/// here.
fn agents_chain(root: &Path, dark_home: &Path) -> String {
    dark_agentsmd::resolve(
        dark_home,
        root,
        &dark_agentsmd::WorkingSet::new(),
        &dark_agentsmd::AgentsMdConfig::default(),
        &|text: &str| text.split_whitespace().count(),
    )
    .map(|chain| chain.prefix_text())
    .unwrap_or_default()
}

/// Writes a charting run's output into the journal and the store.
///
/// The order matters: the map, then every ticket, then the edges. An edge
/// names two tickets and the schema enforces that both exist, so wiring
/// before the tickets land fails the foreign key (task unit `E1`, Do step
/// 7 says the same for the same reason).
fn write_map(
    store: &mut Store,
    maps_root: &Path,
    output: &ChartOutput,
    now: Timestamp,
) -> Result<()> {
    record(
        store,
        maps_root,
        &output.map_id,
        &JournalEvent::MapCreated(MapCreated {
            id: output.map_id.clone(),
            name: output.destination.destination.clone(),
            destination: output
                .tickets
                .first()
                .map_or_else(String::new, |ticket| ticket.id.clone()),
            notes: output.destination.notes.clone(),
            created_at: now,
            status: MapStatus::Active,
        }),
    )?;

    for ticket in &output.tickets {
        record(
            store,
            maps_root,
            &output.map_id,
            &JournalEvent::TicketCreated(TicketCreated {
                id: ticket.id.clone(),
                map_id: output.map_id.clone(),
                name: ticket.name.clone(),
                question: ticket.question.clone(),
                ticket_type: store_type(ticket.ticket_type),
                hitl: ticket.hitl,
                status: TicketStatus::Open,
                created_at: now,
                ordinal: ticket.ordinal,
                // `ChartedTicket::axis` is a list because a split
                // ticket may inherit more than one; the column takes
                // one. The first is the ticket's own axis, and the
                // rest come from the merge that produced it.
                axis: ticket.axis.first().cloned(),
                tokens_used: None,
            }),
        )?;
    }

    for edge in &output.edges {
        store
            .add_edge(maps_root, &output.map_id, &edge.blocker, &edge.blocked)
            .map_err(crate::contract_error)?;
    }

    Ok(())
}

// --------------------------------------------------------------- commands ---

/// Resolves the analysis charting needs, or explains what to run.
pub(crate) fn report_or_explain(root: &Path) -> Result<Report> {
    let report = crate::explore::cached_report(root)?.ok_or_else(|| {
        anyhow::anyhow!(
            "no analysis of this repository exists yet. Run dark explore first — it reads \
             the seams and modules every other command here is built on."
        )
    })?;

    // Saying which tree an analysis describes is cheap; letting a person
    // act on a stale one without knowing is not.
    if let Ok(current) = crate::explore::current_tree_sha(root)
        && current != report.tree_sha
    {
        println!(
            "using the analysis of an earlier tree ({}). Run dark explore --refresh to \
             bring it up to date.",
            &report.tree_sha[..report.tree_sha.len().min(12)]
        );
    }
    Ok(report)
}

/// Runs `dark plan "<idea>"`, and `dark plan resume` when `resume` is set.
fn chart(idea: &str, resume: bool) -> Result<()> {
    let root = crate::repo_root()?;
    let dark_home = crate::dark_home();
    let maps_root = dark_home.join("maps");
    let report = report_or_explain(&root)?;

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("could not start the charting runtime")?;

    let run = runtime.block_on(chart_run(ChartRequest {
        idea,
        resume,
        root: &root,
        dark_home: &dark_home,
        maps_root: &maps_root,
        report: &report,
    }))?;

    match run {
        ChartRun::NoMapNeeded { destination, .. } => {
            println!();
            println!("no map needed: {}", destination.destination);
            println!(
                "every question this raised is already sharp enough to answer, so the work \
                 fits one session. Ask for it directly."
            );
        }
        ChartRun::Charted(output) => {
            let mut store = Store::open(&root).map_err(crate::contract_error)?;
            write_map(&mut store, &maps_root, &output, now_millis())?;
            print!("{}", charted_text(&output));
        }
    }
    Ok(())
}

/// What one charting run needs, gathered so [`chart_async`] takes one
/// argument rather than six.
struct ChartRequest<'a> {
    idea: &'a str,
    resume: bool,
    root: &'a Path,
    dark_home: &'a Path,
    maps_root: &'a Path,
    report: &'a Report,
}

/// Brings a session up and runs the charting pipeline against it.
///
/// Writes nothing and prints nothing: a caller inside the terminal
/// application needs the result as a value, and one that has just charted
/// a map needs to write it exactly once. Both call this and then do their
/// own half.
async fn chart_run(request: ChartRequest<'_>) -> Result<ChartRun> {
    let bus = dark_contract::EventBus::new();
    let harness = crate::harness::bring_up(crate::harness::BringUp {
        root: request.root.to_path_buf(),
        dark_home: request.dark_home.to_path_buf(),
        preferred_model: None,
        policy: dark_core::policy::PolicyConfig::default(),
        mode: dark_core::policy::RunMode::Headless { yes: false },
        events: bus.tx(),
        tier_override: None,
    })
    .await?;

    let chain = agents_chain(request.root, request.dark_home);
    let seed = seed_from(request.report);
    let profile = Profile::read(request.root)?;
    let summary = repo_summary(request.report, profile.as_ref());

    // `dark-plan` cannot read the profile table itself (Rule 17), so the
    // composition root resolves it and passes the plain flag in. See
    // `dark_plan::chart::gate`.
    let resolved = dark_qwen::profile::ProfileTable::builtin()
        .resolve(&harness.caps)
        .map_err(crate::contract_error)?;
    resolved
        .authorize_charting()
        .map_err(crate::contract_error)?;

    let config = ChartConfig {
        role_class: RoleClass::Architect,
        sampling: dark_plan::chart::StageSampling::default(),
        model_id: resolved.model_id.clone(),
        allow_charting: resolved.profile.allow_charting,
        ticket_budget_tokens: ticket_budget(resolved.granted_context),
    };

    let checkpoints = FileCheckpoints {
        maps_root: request.maps_root.to_path_buf(),
    };
    let axis_sets = AxisSets::builtin();
    let pipeline = ChartPipeline::new(harness.engine.as_ref(), config, &axis_sets, &checkpoints);

    let extractor = DefaultExtractor;
    let sharpener = DefaultSharpener::default();
    let sizer = DefaultSizer::default();
    let wirer = DefaultWirer;
    let stages = StageImpls {
        extractor: &extractor,
        sharpener: &sharpener,
        sizer: &sizer,
        wirer: &wirer,
    };

    let map_id = if request.resume {
        latest_map_id(request.maps_root)?
    } else {
        Ulid::new().to_string()
    };

    let run = if request.resume {
        pipeline.resume(&map_id, seed, &stages).await
    } else {
        pipeline
            .chart(
                &map_id,
                DestinationInput {
                    idea: request.idea,
                    agents_md: &chain,
                    repo_summary: &summary,
                },
                seed,
                &stages,
            )
            .await
    }
    .map_err(crate::contract_error)?;
    Ok(run)
}

/// The token budget one ticket must fit: task unit `E5`'s formula.
#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "a context window is far inside f64's exact integer range, and the product is \
              clamped to a positive value below it before the cast"
)]
fn ticket_budget(granted_context: usize) -> usize {
    (granted_context as f64 * 0.55) as usize
}

/// Milliseconds since the Unix epoch, for a journal timestamp.
fn now_millis() -> Timestamp {
    i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis(),
    )
    .unwrap_or(i64::MAX)
}

/// Returns the newest map identifier under `maps_root`.
///
/// A map identifier is a ULID, which leads with its millisecond timestamp,
/// so the newest is the last in byte order — no clock read and no file
/// timestamp, the same reasoning `crate::session::session_ids` uses.
fn latest_map_id(maps_root: &Path) -> Result<String> {
    let mut ids: Vec<String> = std::fs::read_dir(maps_root)
        .with_context(|| format!("cannot read {}", maps_root.display()))?
        .flatten()
        .filter(|entry| entry.path().is_dir())
        .filter_map(|entry| entry.file_name().into_string().ok())
        .collect();
    ids.sort();
    ids.pop().ok_or_else(|| {
        anyhow::anyhow!(
            "no map to resume under {}. Chart one with dark plan \"<idea>\".",
            maps_root.display()
        )
    })
}

/// Renders what charting produced.
fn charted_text(output: &ChartOutput) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    let _ = writeln!(out, "map {}", output.map_id);
    let _ = writeln!(out, "destination: {}", output.destination.destination);
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "{} ticket(s), {} blocking edge(s), {} fog patch(es), {} out of scope",
        output.tickets.len(),
        output.edges.len(),
        output.fog.len(),
        output.out_of_scope.len(),
    );
    for ticket in &output.tickets {
        let _ = writeln!(
            out,
            "  {} {} [{}]",
            ticket.id,
            ticket.name,
            ticket.ticket_type.as_str()
        );
    }
    let _ = writeln!(out);
    let _ = writeln!(out, "dark map show {} to read it.", output.map_id);
    let _ = writeln!(out, "dark plan work to take the first takeable ticket.");
    out
}

/// Charts a map and returns what it produced as text.
///
/// The terminal application needs the words as a value: inside the
/// alternate screen, anything printed to standard output lands under the
/// interface. See [`crate::explore::summarise`].
///
/// # Errors
///
/// Same as [`chart`].
pub(crate) fn chart_text(root: &Path, idea: &str) -> Result<String> {
    let dark_home = crate::dark_home();
    let maps_root = dark_home.join("maps");
    let report = report_or_explain(root)?;

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("could not start the charting runtime")?;

    let run = runtime.block_on(chart_run(ChartRequest {
        idea,
        resume: false,
        root,
        dark_home: &dark_home,
        maps_root: &maps_root,
        report: &report,
    }))?;

    match run {
        ChartRun::NoMapNeeded { destination, .. } => Ok(format!(
            "no map needed: {}\nevery question this raised is already sharp enough to \
             answer, so the work fits one session.",
            destination.destination
        )),
        ChartRun::Charted(output) => {
            let mut store = Store::open(root).map_err(crate::contract_error)?;
            write_map(&mut store, &maps_root, &output, now_millis())?;
            Ok(charted_text(&output))
        }
    }
}

/// Returns the ticket `dark plan work` would take, as text. See
/// [`chart_text`].
///
/// # Errors
///
/// Same as [`work`].
pub(crate) fn work_text(root: &Path, ticket: Option<&str>) -> Result<String> {
    let maps_root = crate::dark_home().join("maps");
    take_ticket(root, &maps_root, ticket)
}

/// Runs `dark plan work [ticket]`.
fn work(ticket: Option<&str>) -> Result<()> {
    let root = crate::repo_root()?;
    let maps_root = crate::dark_home().join("maps");
    print!("{}", take_ticket(&root, &maps_root, ticket)?);
    Ok(())
}

/// Chooses a ticket from the newest map and describes it.
fn take_ticket(root: &Path, maps_root: &Path, ticket: Option<&str>) -> Result<String> {
    use std::fmt::Write as _;
    let store = Store::open(root).map_err(crate::contract_error)?;
    let map_id = latest_map_id(maps_root)?;

    let frontier =
        dark_cartograph::frontier::frontier(&store, &map_id).map_err(crate::contract_error)?;

    if frontier.is_empty() {
        return Ok(format!(
            "nothing on the frontier of {map_id}: every open ticket is blocked, or the map \
             is finished. dark map health --map {map_id} says which.\n"
        ));
    }

    let chosen = match ticket {
        Some(id) => frontier
            .iter()
            .find(|entry| entry.id == id)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "{id} is not on the frontier. It is blocked, resolved, or not in this map. \
                     Run dark plan work with no ticket to take the first takeable one."
                )
            })?,
        None => &frontier[0],
    };

    let mut out = String::new();
    let _ = writeln!(out, "{} {}", chosen.id, chosen.name);
    let _ = writeln!(out);
    let _ = writeln!(out, "{}", chosen.question);
    let _ = writeln!(out);
    if chosen.hitl {
        let _ = writeln!(
            out,
            "This ticket needs a person. Answer it yourself — the harness must not answer \
             for you (Rule 22)."
        );
    } else {
        let _ = writeln!(out, "Take it with: dark run \"{}\"", chosen.question);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn report() -> Report {
        serde_json::from_str(
            r#"{
              "tree_sha": "aa",
              "config_hash": "bb",
              "stats": {"files": 12, "defs": 90, "edges_f": 20, "edges_s": 140, "modularity": 0.51},
              "seams": [
                {"from": "a.rs", "to": "b.rs", "score": 0.71, "hard": true,
                 "betweenness": 0.4, "crosses_community": true,
                 "abstractness_target": 0.2, "cochange": 0.1, "test_proximity": 0.0}
              ],
              "modules": [
                {"path": "src/core", "files": 7, "Ca": 4, "Ce": 1, "A": 0.2, "I": 0.2, "community": 0},
                {"path": "src/edge", "files": 5, "Ca": 0, "Ce": 3, "A": 0.1, "I": 0.9, "community": 1}
              ],
              "hotspots": [{"path": "src/core/mod.rs", "Ca": 9}]
            }"#,
        )
        .expect("the fixture parses")
    }

    #[test]
    fn the_seed_carries_the_seams_and_modules_charting_reads() {
        let seed = seed_from(&report());
        assert_eq!(seed.seams.len(), 1);
        assert_eq!(seed.seams[0].from, "a.rs");
        assert!(seed.seams[0].hard);
        assert_eq!(seed.modules.len(), 2);
        assert_eq!(seed.modules[0].incoming, 4);
        assert_eq!(seed.modules[0].outgoing, 1);
    }

    #[test]
    fn a_fresh_destination_seeds_no_blast_radius() {
        // A blast radius needs a symbol, and a destination nobody has
        // charted yet names none.
        assert!(seed_from(&report()).blast_radius.is_none());
    }

    #[test]
    fn the_summary_is_counted_not_written() {
        let summary = repo_summary(&report(), None);
        assert!(summary.contains("12 files"), "{summary}");
        assert!(summary.contains("90 definitions"), "{summary}");
        assert!(summary.contains("modularity 0.51"), "{summary}");
        assert!(summary.contains("src/core"), "{summary}");
        assert!(
            summary.contains("src/core/mod.rs (9 importers)"),
            "{summary}"
        );
        assert!(summary.contains("a.rs to b.rs"), "{summary}");
    }

    #[test]
    fn the_same_report_summarises_identically() {
        // Stage 1 settles the destination against this. A summary that
        // moved between runs would move the destination with it.
        assert_eq!(repo_summary(&report(), None), repo_summary(&report(), None));
    }

    fn profile(mode: Mode, tree_sha: &str) -> Profile {
        Profile {
            mode,
            language: Some("rust".to_owned()),
            target_language: match mode {
                Mode::Extend => None,
                Mode::Refactor => Some("go".to_owned()),
            },
            pattern: match mode {
                Mode::Extend => None,
                Mode::Refactor => Some("service split".to_owned()),
            },
            tree_sha: tree_sha.to_owned(),
        }
    }

    #[test]
    fn an_extend_profile_tells_charting_to_keep_the_language() {
        let summary = repo_summary(&report(), Some(&profile(Mode::Extend, "aa")));
        assert!(summary.contains("being extended"), "{summary}");
        assert!(summary.contains("Keep writing rust"), "{summary}");
    }

    #[test]
    fn a_refactor_profile_names_the_target_and_the_pattern() {
        // Charting a refactor against a summary of the old language would
        // settle the wrong destination, so this leads the summary.
        let summary = repo_summary(&report(), Some(&profile(Mode::Refactor, "aa")));
        assert!(summary.contains("refactored to go"), "{summary}");
        assert!(summary.contains("service split"), "{summary}");
        assert!(summary.contains("existing code is rust"), "{summary}");
        assert!(
            summary.find("refactored").unwrap() < summary.find("12 files").unwrap(),
            "the decision must come before the counts:\n{summary}"
        );
    }

    #[test]
    fn a_profile_from_an_older_commit_says_so() {
        let summary = repo_summary(&report(), Some(&profile(Mode::Extend, "an-older-tree")));
        assert!(summary.contains("older commit"), "{summary}");
    }

    #[test]
    fn a_current_profile_does_not_warn() {
        let summary = repo_summary(&report(), Some(&profile(Mode::Extend, "aa")));
        assert!(!summary.contains("older commit"), "{summary}");
    }

    #[test]
    fn an_empty_report_still_summarises() {
        let empty: Report = serde_json::from_str(
            r#"{"tree_sha":"a","config_hash":"b",
                "stats":{"files":0,"defs":0,"edges_f":0,"edges_s":0},"seams":[]}"#,
        )
        .expect("a report from before these fields existed still parses");
        let summary = repo_summary(&empty, None);
        assert!(summary.contains("0 files"), "{summary}");
    }

    #[test]
    fn a_checkpoint_round_trips_through_the_file_store() {
        let dir = TempDir::new().unwrap();
        let store = FileCheckpoints {
            maps_root: dir.path().to_path_buf(),
        };
        let checkpoint = Checkpoint {
            map_id: "M1".to_owned(),
            stage: dark_plan::chart::Stage::Seed,
            recorded_at: 7,
            payload: serde_json::json!({"seams": []}),
        };
        store.record(&checkpoint).unwrap();
        let loaded = store.load("M1").unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].map_id, "M1");
        assert_eq!(loaded[0].recorded_at, 7);
    }

    #[test]
    fn a_map_with_no_checkpoints_loads_an_empty_list() {
        let dir = TempDir::new().unwrap();
        let store = FileCheckpoints {
            maps_root: dir.path().to_path_buf(),
        };
        assert!(store.load("never-charted").unwrap().is_empty());
    }

    #[test]
    fn a_truncated_final_line_loses_only_that_stage() {
        // A crash mid-write leaves a partial line. The stages before it
        // are still good, and a resume that threw them away would restart
        // a run that had nearly finished.
        let dir = TempDir::new().unwrap();
        let store = FileCheckpoints {
            maps_root: dir.path().to_path_buf(),
        };
        store
            .record(&Checkpoint {
                map_id: "M1".to_owned(),
                stage: dark_plan::chart::Stage::Seed,
                recorded_at: 1,
                payload: serde_json::json!({}),
            })
            .unwrap();
        let path = store.path("M1");
        let mut text = std::fs::read_to_string(&path).unwrap();
        text.push_str("{\"map_id\": \"M1\", \"stage\"");
        std::fs::write(&path, text).unwrap();

        let loaded = store.load("M1").unwrap();
        assert_eq!(loaded.len(), 1, "the complete stage survives");
    }

    #[test]
    fn the_ticket_budget_is_just_over_half_the_granted_context() {
        assert_eq!(ticket_budget(20_000), 11_000);
        assert_eq!(ticket_budget(0), 0);
    }

    #[test]
    fn resuming_with_no_map_says_what_to_run() {
        let dir = TempDir::new().unwrap();
        let err = latest_map_id(dir.path()).unwrap_err().to_string();
        assert!(err.contains("dark plan"), "{err}");
    }

    #[test]
    fn the_newest_map_is_the_one_resumed() {
        let dir = TempDir::new().unwrap();
        for id in ["01AAA", "01CCC", "01BBB"] {
            std::fs::create_dir_all(dir.path().join(id)).unwrap();
        }
        assert_eq!(latest_map_id(dir.path()).unwrap(), "01CCC");
    }
}
