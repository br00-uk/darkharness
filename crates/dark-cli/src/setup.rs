//! `dark setup`: the one command the primary requirement allows to use
//! the network.
//!
//! After `dark setup` completes, a person disconnects the network and
//! keeps working (Section 3.2). Task unit `J3` lists eight steps for this
//! command; most of them need a task unit that has not landed yet
//! (`dark tune` writes a hardware profile in `B6`; model download,
//! conversion, and the live verification test are `B2` to `B5`; pack
//! indexing is `G1` to `G5`). This module runs every step that it
//! genuinely can today, and reports plainly — never silently, never with
//! a faked result — which steps still wait on a task unit.
//!
//! Two steps need nothing that has not landed: detecting the ecosystem
//! (step 6) is a filesystem read, and running the doctor report (step 8)
//! is [`crate::doctor::report`] itself. `--dry-run` prints the plan
//! without touching the filesystem at all, so it is safe to run on any
//! machine, at any point in the build.

use std::path::Path;

/// Whether a setup step ran for real or is waiting on a task unit.
#[derive(Debug, Clone, PartialEq, Eq)]
enum StepOutcome {
    /// The step ran and this is what it found or did.
    Done(String),
    /// The step cannot run yet. Names the task unit that will let it, and
    /// what it will do once that lands.
    Pending {
        /// The task unit this step waits on.
        task_unit: &'static str,
        /// What the step will do once `task_unit` lands.
        plan: &'static str,
    },
}

/// One step of `dark setup`, in the order task unit `J3` lists them.
struct Step {
    /// The step's position in the eight-step list.
    number: u8,
    /// The step's title, matching the `J3` "Do" list.
    title: &'static str,
}

/// The eight steps, in order. `title` matches the wording of task unit
/// `J3`'s numbered list.
const STEPS: [Step; 8] = [
    Step {
        number: 1,
        title: "Run dark tune. Write the hardware profile.",
    },
    Step {
        number: 2,
        title: "Recommend a profile: model, quantisation, context, expected rate, sharing.",
    },
    Step {
        number: 3,
        title: "Download the weights. Show the size before the download starts.",
    },
    Step {
        number: 4,
        title: "Convert the weights to UQFF, so a later model swap is fast.",
    },
    Step {
        number: 5,
        title: "Verify with a live test: one generation, one tool call, one embedding.",
    },
    Step {
        number: 6,
        title: "Detect the ecosystem. Suggest documentation packs.",
    },
    Step {
        number: 7,
        title: "Index the packs.",
    },
    Step {
        number: 8,
        title: "Run dark doctor. Print OFFLINE READY or list what is missing.",
    },
];

/// A manifest file this step recognises, and the ecosystem it names.
///
/// Step 6 of task unit `J3` names these four files explicitly; this list
/// order is also the report order, so the output does not depend on
/// filesystem iteration order (in the spirit of Rule 30, even though this
/// scan is not one of the `/explore` stages that rule governs).
const ECOSYSTEM_MANIFESTS: [(&str, &str, &str); 4] = [
    ("Cargo.toml", "Rust", "rust-std"),
    ("package.json", "Node.js", "node"),
    ("pyproject.toml", "Python", "python"),
    ("go.mod", "Go", "go"),
];

/// Reads `repo_root` for the manifest files task unit `J3` step 6 names,
/// and returns one line per ecosystem found, naming the pack `dark pack
/// add` would suggest for it.
///
/// Real: this is a filesystem read, nothing else. It needs no engine, no
/// model, and no network.
fn detect_ecosystems(repo_root: &Path) -> Vec<String> {
    ECOSYSTEM_MANIFESTS
        .iter()
        .filter(|(file_name, _, _)| repo_root.join(file_name).is_file())
        .map(|(file_name, ecosystem, pack)| {
            format!("found {file_name}: {ecosystem}. Suggested pack: dark pack add {pack}.")
        })
        .collect()
}

/// Runs step 6 (detect the ecosystem, suggest packs) for real.
fn run_ecosystem_step(repo_root: &Path) -> StepOutcome {
    let found = detect_ecosystems(repo_root);
    if found.is_empty() {
        StepOutcome::Done(format!(
            "no recognised manifest (Cargo.toml, package.json, pyproject.toml, go.mod) under {}.",
            repo_root.display()
        ))
    } else {
        StepOutcome::Done(found.join(" "))
    }
}

/// Runs step 8 (run `dark doctor`, print `OFFLINE READY` or the gap
/// list) for real, by calling the same report [`crate::doctor::report`]
/// builds for `dark doctor --offline`.
fn run_doctor_step(dark_home: &Path, repo_root: &Path) -> StepOutcome {
    let outcome = crate::doctor::report(dark_home, repo_root);
    let rendered = outcome.render(true);
    StepOutcome::Done(rendered.trim_end().to_owned())
}

/// Returns the outcome of every step, in order, without changing
/// anything on disk. Steps 6 and 8 run for real; every other step
/// reports [`StepOutcome::Pending`], because it needs a task unit that
/// has not landed.
fn run_steps(dark_home: &Path, repo_root: &Path) -> Vec<StepOutcome> {
    vec![
        StepOutcome::Pending {
            task_unit: "B6",
            plan: "measure this machine and write $DARK_HOME/config.toml's [hardware] section.",
        },
        StepOutcome::Pending {
            task_unit: "B6",
            plan: "pick a model, quantisation, and context length from the hardware profile.",
        },
        StepOutcome::Pending {
            task_unit: "B2",
            plan: "show the download size, then fetch the weights through dark-airlock.",
        },
        StepOutcome::Pending {
            task_unit: "B2",
            plan: "convert the downloaded weights to UQFF.",
        },
        StepOutcome::Pending {
            task_unit: "B2 to B5",
            plan: "run one generation, one tool call, and one embedding against the loaded \
                   model.",
        },
        run_ecosystem_step(repo_root),
        StepOutcome::Pending {
            task_unit: "G1 to G5",
            plan: "chunk and index every suggested pack.",
        },
        run_doctor_step(dark_home, repo_root),
    ]
}

/// Renders the plan or the outcome of every step as text for a person to
/// read.
///
/// In `--dry-run` mode this describes what each step would do, without
/// running any of them — including the two steps ([`run_ecosystem_step`]
/// and [`run_doctor_step`]) that could otherwise run for real, so
/// `--dry-run` never touches the filesystem. Otherwise it runs
/// [`run_steps`] and reports what actually happened.
fn render_plan() -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    for step in &STEPS {
        let _ = writeln!(out, "{}. {}", step.number, step.title);
    }
    out
}

/// Renders the outcome of a real (non-dry-run) run.
fn render_outcomes(outcomes: &[StepOutcome]) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    for (step, outcome) in STEPS.iter().zip(outcomes) {
        match outcome {
            StepOutcome::Done(detail) => {
                let _ = writeln!(out, "{}. [DONE] {}\n   {}", step.number, step.title, detail);
            }
            StepOutcome::Pending { task_unit, plan } => {
                let _ = writeln!(
                    out,
                    "{}. [PENDING] {}\n   waits on task unit {task_unit}: {plan}",
                    step.number, step.title,
                );
            }
        }
    }
    out
}

/// Reports whether every step in `outcomes` ran for real.
fn all_done(outcomes: &[StepOutcome]) -> bool {
    outcomes
        .iter()
        .all(|outcome| matches!(outcome, StepOutcome::Done(_)))
}

/// Runs `dark setup`.
///
/// `dry_run` prints the eight-step plan and returns without touching the
/// filesystem or the network. Without it, this runs every step that can
/// run today (detecting the ecosystem, then the doctor report) and
/// reports, plainly, which steps still wait on a task unit; it returns an
/// error in that case, because `dark setup` has not, in fact, finished
/// preparing the machine for offline work yet.
///
/// # Errors
///
/// Returns an error when the current directory cannot be read, or — for
/// a real (non-dry-run) run — when a step is still pending a task unit.
pub(crate) fn run_command(dry_run: bool) -> anyhow::Result<()> {
    if dry_run {
        print!("{}", render_plan());
        println!(
            "\nDry run: nothing was downloaded, converted, or written. Step 6 and step 8 can \
             run for real today; every other step waits on the task unit named in the build \
             specification."
        );
        return Ok(());
    }

    let dark_home = crate::dark_home();
    let repo_root = crate::repo_root()?;
    let outcomes = run_steps(&dark_home, &repo_root);
    print!("{}", render_outcomes(&outcomes));

    if all_done(&outcomes) {
        Ok(())
    } else {
        anyhow::bail!(
            "dark setup cannot finish yet: one or more steps wait on a task unit that has not \
             landed. See the plan above."
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn detect_ecosystems_finds_a_rust_workspace() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("Cargo.toml"), "[workspace]").unwrap();
        let found = detect_ecosystems(tmp.path());
        assert_eq!(found.len(), 1);
        assert!(found[0].contains("Rust"));
        assert!(found[0].contains("dark pack add rust-std"));
    }

    #[test]
    fn detect_ecosystems_finds_more_than_one_manifest() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("Cargo.toml"), "[workspace]").unwrap();
        fs::write(tmp.path().join("package.json"), "{}").unwrap();
        let found = detect_ecosystems(tmp.path());
        assert_eq!(found.len(), 2);
    }

    #[test]
    fn detect_ecosystems_is_empty_for_a_bare_directory() {
        let tmp = TempDir::new().unwrap();
        assert!(detect_ecosystems(tmp.path()).is_empty());
    }

    #[test]
    fn detect_ecosystems_ignores_a_directory_named_like_a_manifest() {
        let tmp = TempDir::new().unwrap();
        fs::create_dir(tmp.path().join("Cargo.toml")).unwrap();
        assert!(detect_ecosystems(tmp.path()).is_empty());
    }

    #[test]
    fn run_ecosystem_step_reports_done_even_with_nothing_found() {
        let tmp = TempDir::new().unwrap();
        let outcome = run_ecosystem_step(tmp.path());
        assert!(matches!(outcome, StepOutcome::Done(_)));
    }

    #[test]
    fn run_doctor_step_reports_done_and_embeds_the_doctor_report() {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().join("home");
        let repo = tmp.path().join("repo");
        fs::create_dir_all(&home).unwrap();
        fs::create_dir_all(&repo).unwrap();
        let StepOutcome::Done(detail) = run_doctor_step(&home, &repo) else {
            panic!("run_doctor_step must always be Done");
        };
        // The doctor report always names at least one check, whatever the
        // machine looks like.
        assert!(detail.contains("Git presence"));
    }

    #[test]
    fn run_steps_has_one_outcome_per_step() {
        let tmp = TempDir::new().unwrap();
        let outcomes = run_steps(tmp.path(), tmp.path());
        assert_eq!(outcomes.len(), STEPS.len());
    }

    #[test]
    fn run_steps_marks_steps_one_through_five_and_seven_as_pending() {
        let tmp = TempDir::new().unwrap();
        let outcomes = run_steps(tmp.path(), tmp.path());
        for index in [0usize, 1, 2, 3, 4, 6] {
            assert!(
                matches!(outcomes[index], StepOutcome::Pending { .. }),
                "step {} should be pending",
                index + 1
            );
        }
    }

    #[test]
    fn run_steps_marks_steps_six_and_eight_as_done() {
        let tmp = TempDir::new().unwrap();
        let outcomes = run_steps(tmp.path(), tmp.path());
        assert!(matches!(outcomes[5], StepOutcome::Done(_)));
        assert!(matches!(outcomes[7], StepOutcome::Done(_)));
    }

    #[test]
    fn all_done_is_false_while_any_step_is_pending() {
        let outcomes = vec![
            StepOutcome::Done("ok".to_owned()),
            StepOutcome::Pending {
                task_unit: "B2",
                plan: "download",
            },
        ];
        assert!(!all_done(&outcomes));
    }

    #[test]
    fn all_done_is_true_when_every_step_finished() {
        let outcomes = vec![
            StepOutcome::Done("ok".to_owned()),
            StepOutcome::Done("also ok".to_owned()),
        ];
        assert!(all_done(&outcomes));
    }

    #[test]
    fn render_plan_lists_all_eight_steps_in_order() {
        let text = render_plan();
        for step in &STEPS {
            assert!(
                text.contains(&format!("{}. ", step.number)),
                "missing step {}",
                step.number
            );
        }
    }

    #[test]
    fn render_outcomes_marks_pending_and_done_distinctly() {
        let outcomes = vec![
            StepOutcome::Done("finished".to_owned()),
            StepOutcome::Pending {
                task_unit: "B2",
                plan: "download the weights.",
            },
        ];
        let text = render_outcomes(&outcomes);
        assert!(text.contains("[DONE]"));
        assert!(text.contains("[PENDING]"));
        assert!(text.contains("B2"));
    }
}
