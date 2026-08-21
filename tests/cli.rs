//! End-to-end tests that invoke the compiled binary.
//!
//! `CARGO_BIN_EXE_darkharness` is set by Cargo for integration tests, so these
//! run the real executable without needing a helper crate to locate it.

use std::process::Command;

fn binary() -> Command {
    Command::new(env!("CARGO_BIN_EXE_darkharness"))
}

#[test]
fn run_reports_task_count() {
    let output = binary()
        .args(["run", "--name", "ci", "--workers", "3"])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "expected success, got {:?}",
        output.status
    );

    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        stdout.contains("completed 3 task(s)"),
        "unexpected stdout: {stdout}"
    );
}

#[test]
fn run_defaults_to_a_single_task() {
    let output = binary().arg("run").output().unwrap();

    assert!(output.status.success());
    assert!(
        String::from_utf8(output.stdout)
            .unwrap()
            .contains("completed 1 task(s)")
    );
}

#[test]
fn invalid_worker_count_fails_with_a_message() {
    let output = binary().args(["run", "--workers", "0"]).output().unwrap();

    assert!(!output.status.success(), "zero workers must exit non-zero");

    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("workers must be at least 1"),
        "unexpected stderr: {stderr}"
    );
}

#[test]
fn help_is_available() {
    let output = binary().arg("--help").output().unwrap();

    assert!(output.status.success());
    assert!(
        String::from_utf8(output.stdout)
            .unwrap()
            .contains("darkharness")
    );
}
