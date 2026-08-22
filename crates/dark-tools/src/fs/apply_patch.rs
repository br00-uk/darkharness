//! The `apply_patch` tool.
//!
//! `apply_patch` takes a unified diff, possibly touching several files, and
//! applies every hunk in it or none of it. Each hunk's context and removed
//! lines are checked, at their exact recorded position, against the file's
//! current content; a hunk that does not match fails the whole call before
//! anything is written, which is also what makes a stale patch fail safely.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use dark_contract::{ErrCode, Error, Result, Tool, ToolCtx, ToolResult, ToolSchema, tool::tier};
use serde::Deserialize;
use serde_json::{Value, json};

use super::atomic;
use super::diff_util;
use super::path;
use super::state::ReadState;

/// The unified-diff marker saying the neighbouring line has no trailing
/// newline. git writes it directly after the line it describes.
const NO_NEWLINE_MARKER: &str = "\\ No newline at end of file";

#[derive(Debug, Deserialize)]
struct Args {
    patch: String,
}

/// One line inside a hunk body.
#[derive(Debug, Clone, PartialEq, Eq)]
enum HunkLine {
    /// A line present, unchanged, in both the old and the new content.
    Context(String),
    /// A line present only in the old content.
    Removed(String),
    /// A line present only in the new content.
    Added(String),
}

/// One `@@ ... @@` hunk.
#[derive(Debug)]
struct Hunk {
    /// The 1-based first line this hunk touches in the old content.
    old_start: usize,
    /// The body of the hunk, in order.
    lines: Vec<HunkLine>,
}

/// The header and hunks for one file inside a multi-file patch.
#[derive(Debug)]
struct FilePatch {
    /// `None` when the old side is `/dev/null` (the patch creates the file).
    old_path: Option<String>,
    /// `None` when the new side is `/dev/null` (the patch deletes the file).
    new_path: Option<String>,
    hunks: Vec<Hunk>,
    /// Whether the last old-side line the patch touches has no trailing newline.
    old_no_trailing_newline: bool,
    /// Whether the last new-side line the patch touches has no trailing newline.
    new_no_trailing_newline: bool,
}

/// Applies a multi-file unified diff.
#[derive(Debug)]
pub struct ApplyPatch {
    state: Arc<ReadState>,
}

impl ApplyPatch {
    /// Creates the tool, sharing `state` with the other file tools in this
    /// session.
    pub(crate) fn new(state: Arc<ReadState>) -> Self {
        Self { state }
    }
}

#[async_trait]
impl Tool for ApplyPatch {
    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "apply_patch".to_string(),
            description: "Applies a unified diff to one or more files. Every hunk must match \
                the current file content exactly. The whole patch applies, or none of it does."
                .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "patch": {
                        "type": "string",
                        "description": "A unified diff, in the `--- a/path` / `+++ b/path` / \
                            `@@ -l,s +l,s @@` format. Use /dev/null to create or delete a file.",
                    },
                },
                "required": ["patch"],
            }),
            tier: tier::STANDARD,
            mutating: true,
        }
    }

    async fn invoke(&self, args: Value, ctx: &ToolCtx) -> Result<ToolResult> {
        let args: Args = serde_json::from_value(args).map_err(|err| {
            Error::new(
                ErrCode::ToolInvalidArgs,
                format!("apply_patch arguments: {err}"),
            )
        })?;

        let file_patches = parse(&args.patch)?;
        if file_patches.is_empty() {
            return Err(Error::new(
                ErrCode::ToolInvalidArgs,
                "the patch contains no file headers (`--- ` / `+++ `)",
            ));
        }

        // Plan every change in memory first, so one bad hunk in file three
        // cannot leave files one and two written. Nothing touches disk
        // until every file patch in this call has validated cleanly.
        let mut plans = Vec::with_capacity(file_patches.len());
        for fp in &file_patches {
            plans.push(plan_one_file(fp, ctx).await?);
        }

        let mut touched = Vec::with_capacity(plans.len());
        let mut diff = String::new();
        for plan in &plans {
            apply_plan(plan).await?;
            match &plan.action {
                Action::Delete => {
                    self.state.forget(&plan.target);
                    touched.push(format!("deleted {}", plan.display_path));
                }
                Action::Write { new_content, .. } => {
                    self.state.record(&plan.target, new_content.as_bytes());
                    touched.push(format!("wrote {}", plan.display_path));
                }
            }
            if let Some(rendered) = &plan.diff {
                diff.push_str(rendered);
            }
        }
        // A rename's source path no longer exists; forget it so a later
        // read_file reports ToolNotFound instead of stale content.
        for plan in &plans {
            if let Some(source) = &plan.remove_source {
                self.state.forget(source);
            }
        }

        let mut result = ToolResult::ok(touched.join("\n"));
        if !diff.is_empty() {
            result = result.with_diff(diff);
        }
        Ok(result)
    }
}

enum Action {
    Delete,
    Write { new_content: String },
}

struct Plan {
    target: PathBuf,
    display_path: String,
    action: Action,
    diff: Option<String>,
    /// Set for a rename: the old path to remove once the new path is written.
    remove_source: Option<PathBuf>,
}

async fn plan_one_file(fp: &FilePatch, ctx: &ToolCtx) -> Result<Plan> {
    match (&fp.old_path, &fp.new_path) {
        (None, Some(new_path)) => {
            let target = path::resolve(&ctx.root, new_path)?;
            if tokio::fs::metadata(&target).await.is_ok() {
                return Err(Error::new(
                    ErrCode::ToolFailed,
                    format!("{new_path} already exists, but the patch describes it as new"),
                ));
            }
            let new_content = apply_hunks(fp, "")?;
            let diff = diff_util::render(new_path, "", &new_content);
            Ok(Plan {
                target,
                display_path: new_path.clone(),
                action: Action::Write { new_content },
                diff,
                remove_source: None,
            })
        }
        (Some(old_path), None) => {
            let target = path::resolve(&ctx.root, old_path)?;
            let bytes = read_existing(&target, old_path).await?;
            let old_content = utf8(&bytes, old_path)?;
            let remaining = apply_hunks(fp, &old_content)?;
            if !remaining.is_empty() {
                return Err(Error::new(
                    ErrCode::ToolFailed,
                    format!(
                        "the patch does not remove every line of {old_path}, but marks it deleted"
                    ),
                ));
            }
            let diff = diff_util::render(old_path, &old_content, "");
            Ok(Plan {
                target,
                display_path: old_path.clone(),
                action: Action::Delete,
                diff,
                remove_source: None,
            })
        }
        (Some(old_path), Some(new_path)) => {
            let old_target = path::resolve(&ctx.root, old_path)?;
            let bytes = read_existing(&old_target, old_path).await?;
            let old_content = utf8(&bytes, old_path)?;
            let new_content = apply_hunks(fp, &old_content)?;
            let new_target = path::resolve(&ctx.root, new_path)?;
            let diff = diff_util::render(new_path, &old_content, &new_content);
            let remove_source = if new_target == old_target {
                None
            } else {
                Some(old_target)
            };
            Ok(Plan {
                target: new_target,
                display_path: new_path.clone(),
                action: Action::Write { new_content },
                diff,
                remove_source,
            })
        }
        (None, None) => Err(Error::new(
            ErrCode::ToolInvalidArgs,
            "a patch file header needs at least one real path",
        )),
    }
}

async fn apply_plan(plan: &Plan) -> Result<()> {
    match &plan.action {
        Action::Delete => tokio::fs::remove_file(&plan.target).await.map_err(|err| {
            Error::new(
                ErrCode::ToolFailed,
                format!("cannot delete {}: {err}", plan.display_path),
            )
        }),
        Action::Write { new_content } => {
            let mode = tokio::fs::metadata(&plan.target)
                .await
                .ok()
                .map(|m| m.permissions());
            atomic::write(plan.target.clone(), new_content.clone().into_bytes(), mode).await?;
            if let Some(source) = &plan.remove_source {
                tokio::fs::remove_file(source).await.map_err(|err| {
                    Error::new(
                        ErrCode::ToolFailed,
                        format!(
                            "wrote {}, but could not remove the old path: {err}",
                            plan.display_path
                        ),
                    )
                })?;
            }
            Ok(())
        }
    }
}

async fn read_existing(target: &std::path::Path, display_path: &str) -> Result<Vec<u8>> {
    tokio::fs::read(target).await.map_err(|_| {
        Error::new(
            ErrCode::ToolNotFound,
            format!("{display_path} does not exist"),
        )
    })
}

fn utf8(bytes: &[u8], display_path: &str) -> Result<String> {
    String::from_utf8(bytes.to_vec()).map_err(|_| {
        Error::new(
            ErrCode::ToolFailed,
            format!("{display_path} is not valid UTF-8"),
        )
    })
}

/// Applies every hunk in `fp` to `original`, in order, and returns the
/// resulting content.
///
/// # Errors
///
/// Returns [`ErrCode::ToolFailed`] the moment one hunk's context or removed
/// lines do not match `original` at the position the hunk names, or when the
/// patch disagrees with `original` about a trailing newline. No partial
/// result from a failing hunk ever reaches the caller.
fn apply_hunks(fp: &FilePatch, original: &str) -> Result<String> {
    let (orig_lines, original_ends_with_newline) = split_lines(original);

    // A patch carrying `\ No newline at end of file` on its old side
    // describes a file that does not end with a newline. When the file on
    // disk disagrees, the patch was built against different content, so
    // applying it would silently add or remove a byte the author never saw.
    if !orig_lines.is_empty() && fp.old_no_trailing_newline == original_ends_with_newline {
        return Err(hunk_error(if fp.old_no_trailing_newline {
            "the patch says the original has no trailing newline, but the file on disk has one"
        } else {
            "the file on disk has no trailing newline, but the patch expects one"
        }));
    }

    let mut out: Vec<String> = Vec::new();
    let mut cursor = 0usize;

    for hunk in &fp.hunks {
        let start = hunk.old_start.saturating_sub(1);
        if start < cursor {
            return Err(hunk_error("hunks are out of order or overlap"));
        }
        if start > orig_lines.len() {
            return Err(hunk_error("a hunk starts past the end of the file"));
        }
        out.extend(orig_lines[cursor..start].iter().map(|s| (*s).to_string()));
        cursor = start;

        for line in &hunk.lines {
            match line {
                HunkLine::Context(text) => {
                    let actual = orig_lines.get(cursor).copied();
                    if actual != Some(text.as_str()) {
                        return Err(context_mismatch(hunk.old_start, cursor, text, actual));
                    }
                    out.push(text.clone());
                    cursor += 1;
                }
                HunkLine::Removed(text) => {
                    let actual = orig_lines.get(cursor).copied();
                    if actual != Some(text.as_str()) {
                        return Err(context_mismatch(hunk.old_start, cursor, text, actual));
                    }
                    cursor += 1;
                }
                HunkLine::Added(text) => out.push(text.clone()),
            }
        }
    }
    out.extend(orig_lines[cursor..].iter().map(|s| (*s).to_string()));

    let mut new_content = out.join("\n");
    if !new_content.is_empty() && !fp.new_no_trailing_newline {
        new_content.push('\n');
    }
    Ok(new_content)
}

fn hunk_error(message: &str) -> Error {
    Error::new(
        ErrCode::ToolFailed,
        format!("cannot apply the patch: {message}"),
    )
}

fn context_mismatch(hunk_start: usize, at: usize, expected: &str, actual: Option<&str>) -> Error {
    let actual_display =
        actual.map_or_else(|| "end of file".to_string(), |line| format!("`{line}`"));
    Error::new(
        ErrCode::ToolFailed,
        format!(
            "hunk at line {hunk_start} does not match the file: expected `{expected}` at line {}, found {actual_display}",
            at + 1
        ),
    )
}

/// Splits `text` into lines without their line endings, and reports whether
/// `text` ends with a trailing newline.
fn split_lines(text: &str) -> (Vec<&str>, bool) {
    if text.is_empty() {
        return (Vec::new(), false);
    }
    let ends_with_newline = text.ends_with('\n');
    let trimmed = text.strip_suffix('\n').unwrap_or(text);
    (trimmed.split('\n').collect(), ends_with_newline)
}

/// Parses a (possibly multi-file) unified diff.
///
/// # Errors
///
/// Returns [`ErrCode::ToolInvalidArgs`] when the text does not follow the
/// unified diff format this tool supports.
fn parse(patch: &str) -> Result<Vec<FilePatch>> {
    let lines: Vec<&str> = patch.lines().collect();
    let mut i = 0;
    let mut files = Vec::new();

    while i < lines.len() {
        if !lines[i].starts_with("--- ") {
            i += 1;
            continue;
        }
        let old_header = &lines[i][4..];
        let Some(new_line) = lines.get(i + 1) else {
            return Err(parse_error("`--- ` header with no `+++ ` line after it"));
        };
        let Some(new_header) = new_line.strip_prefix("+++ ") else {
            return Err(parse_error("`--- ` header with no `+++ ` line after it"));
        };
        i += 2;

        let old_path = header_path(old_header);
        let new_path = header_path(new_header);
        let mut hunks = Vec::new();
        let mut old_no_trailing_newline = false;
        let mut new_no_trailing_newline = false;

        while i < lines.len() && lines[i].starts_with("@@ ") {
            let (old_start, old_count, _new_start, new_count) = parse_hunk_header(lines[i])?;
            i += 1;

            let mut body = Vec::new();
            let mut old_seen = 0usize;
            let mut new_seen = 0usize;
            let mut last_prefix = ' ';
            while old_seen < old_count || new_seen < new_count {
                let Some(raw) = lines.get(i) else {
                    return Err(parse_error("a hunk ends before its declared line count"));
                };
                if let Some(rest) = raw.strip_prefix(' ') {
                    body.push(HunkLine::Context(rest.to_string()));
                    old_seen += 1;
                    new_seen += 1;
                    last_prefix = ' ';
                } else if let Some(rest) = raw.strip_prefix('-') {
                    body.push(HunkLine::Removed(rest.to_string()));
                    old_seen += 1;
                    last_prefix = '-';
                } else if let Some(rest) = raw.strip_prefix('+') {
                    body.push(HunkLine::Added(rest.to_string()));
                    new_seen += 1;
                    last_prefix = '+';
                } else if raw.is_empty()
                    && old_count.saturating_sub(old_seen) <= 1
                    && new_count.saturating_sub(new_seen) <= 1
                {
                    // Tolerate a genuinely blank context line with its
                    // leading space stripped by a lossy transport.
                    body.push(HunkLine::Context(String::new()));
                    old_seen += 1;
                    new_seen += 1;
                    last_prefix = ' ';
                } else if *raw == NO_NEWLINE_MARKER {
                    // git emits this immediately after the line it
                    // qualifies, which is usually inside the hunk rather
                    // than after it: a `-` line, the marker, then the `+`
                    // lines. It describes the preceding line and counts
                    // towards neither side's declared line count.
                    match last_prefix {
                        '-' => old_no_trailing_newline = true,
                        '+' => new_no_trailing_newline = true,
                        _ => {
                            old_no_trailing_newline = true;
                            new_no_trailing_newline = true;
                        }
                    }
                } else {
                    return Err(parse_error(&format!("unrecognised hunk line: `{raw}`")));
                }
                i += 1;
            }

            // The marker can also sit after the hunk body, when the last
            // line of the hunk is the one without a newline.
            if lines.get(i) == Some(&NO_NEWLINE_MARKER) {
                match last_prefix {
                    '-' => old_no_trailing_newline = true,
                    '+' => new_no_trailing_newline = true,
                    _ => {
                        old_no_trailing_newline = true;
                        new_no_trailing_newline = true;
                    }
                }
                i += 1;
            }

            hunks.push(Hunk {
                old_start: if old_count == 0 {
                    old_start + 1
                } else {
                    old_start
                },
                lines: body,
            });
        }

        files.push(FilePatch {
            old_path,
            new_path,
            hunks,
            old_no_trailing_newline,
            new_no_trailing_newline,
        });
    }

    Ok(files)
}

fn header_path(raw: &str) -> Option<String> {
    let raw = raw.split('\t').next().unwrap_or(raw).trim();
    if raw == "/dev/null" {
        return None;
    }
    let stripped = raw
        .strip_prefix("a/")
        .or_else(|| raw.strip_prefix("b/"))
        .unwrap_or(raw);
    Some(stripped.to_string())
}

/// Parses `@@ -old_start,old_count +new_start,new_count @@`, defaulting an
/// omitted count to `1`.
fn parse_hunk_header(line: &str) -> Result<(usize, usize, usize, usize)> {
    let body = line
        .strip_prefix("@@ ")
        .and_then(|rest| rest.split(" @@").next())
        .ok_or_else(|| parse_error(&format!("malformed hunk header: `{line}`")))?;

    let mut parts = body.split_whitespace();
    let old = parts
        .next()
        .ok_or_else(|| parse_error(&format!("malformed hunk header: `{line}`")))?;
    let new = parts
        .next()
        .ok_or_else(|| parse_error(&format!("malformed hunk header: `{line}`")))?;

    let (old_start, old_count) = parse_range(old, '-', line)?;
    let (new_start, new_count) = parse_range(new, '+', line)?;
    Ok((old_start, old_count, new_start, new_count))
}

fn parse_range(token: &str, sign: char, line: &str) -> Result<(usize, usize)> {
    let rest = token
        .strip_prefix(sign)
        .ok_or_else(|| parse_error(&format!("malformed hunk header: `{line}`")))?;
    let mut split = rest.splitn(2, ',');
    let start: usize = split
        .next()
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| parse_error(&format!("malformed hunk header: `{line}`")))?;
    let count: usize = match split.next() {
        Some(s) => s
            .parse()
            .map_err(|_| parse_error(&format!("malformed hunk header: `{line}`")))?,
        None => 1,
    };
    Ok((start, count))
}

fn parse_error(message: &str) -> Error {
    Error::new(
        ErrCode::ToolInvalidArgs,
        format!("cannot parse the patch: {message}"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio_util::sync::CancellationToken;

    fn ctx(root: &std::path::Path) -> ToolCtx {
        let bus = dark_contract::EventBus::new();
        ToolCtx {
            root: root.to_path_buf(),
            events: bus.tx(),
            cancel: CancellationToken::new(),
            dark: true,
            human_present: false,
        }
    }

    fn tool() -> ApplyPatch {
        ApplyPatch::new(Arc::new(ReadState::new()))
    }

    #[tokio::test]
    async fn modifies_an_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "one\ntwo\nthree\n").unwrap();
        let patch = "--- a/a.txt\n+++ b/a.txt\n@@ -1,3 +1,3 @@\n one\n-two\n+TWO\n three\n";

        let result = tool()
            .invoke(json!({"patch": patch}), &ctx(dir.path()))
            .await
            .unwrap();

        assert!(!result.is_error);
        assert_eq!(
            std::fs::read_to_string(dir.path().join("a.txt")).unwrap(),
            "one\nTWO\nthree\n"
        );
        assert!(result.diff.unwrap().contains("-two"));
    }

    #[tokio::test]
    async fn creates_a_new_file() {
        let dir = tempfile::tempdir().unwrap();
        let patch = "--- /dev/null\n+++ b/new.txt\n@@ -0,0 +1,2 @@\n+line one\n+line two\n";

        tool()
            .invoke(json!({"patch": patch}), &ctx(dir.path()))
            .await
            .unwrap();

        assert_eq!(
            std::fs::read_to_string(dir.path().join("new.txt")).unwrap(),
            "line one\nline two\n"
        );
    }

    #[tokio::test]
    async fn deletes_a_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("gone.txt"), "bye\n").unwrap();
        let patch = "--- a/gone.txt\n+++ /dev/null\n@@ -1,1 +0,0 @@\n-bye\n";

        let result = tool()
            .invoke(json!({"patch": patch}), &ctx(dir.path()))
            .await
            .unwrap();

        assert!(!result.is_error);
        assert!(!dir.path().join("gone.txt").exists());
    }

    #[tokio::test]
    async fn a_trailing_newline_disagreement_applies_nothing() {
        // The patch was built against a file with no trailing newline. The
        // file on disk has one, so the patch describes different content and
        // applying it would silently change a byte the author never saw.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "one\n").unwrap();
        let patch =
            "--- a/a.txt\n+++ b/a.txt\n@@ -1,1 +1,1 @@\n-one\n\\ No newline at end of file\n+two\n";

        let result = tool()
            .invoke(json!({"patch": patch}), &ctx(dir.path()))
            .await;

        let err = result.expect_err("a trailing-newline disagreement must be refused");
        assert_eq!(
            err.code,
            ErrCode::ToolFailed,
            "message was: {}",
            err.message
        );
        assert!(
            err.message.contains("trailing newline"),
            "unhelpful: {}",
            err.message
        );
        // Nothing was written.
        assert_eq!(
            std::fs::read_to_string(dir.path().join("a.txt")).unwrap(),
            "one\n"
        );
    }

    #[tokio::test]
    async fn a_context_mismatch_applies_nothing() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "one\ntwo\nthree\n").unwrap();
        // The context line "two" does not match "TWO ALREADY CHANGED".
        std::fs::write(
            dir.path().join("a.txt"),
            "one\nTWO ALREADY CHANGED\nthree\n",
        )
        .unwrap();
        let patch = "--- a/a.txt\n+++ b/a.txt\n@@ -1,3 +1,3 @@\n one\n-two\n+TWO\n three\n";

        let err = tool()
            .invoke(json!({"patch": patch}), &ctx(dir.path()))
            .await
            .unwrap_err();

        assert_eq!(err.code, ErrCode::ToolFailed);
        assert_eq!(
            std::fs::read_to_string(dir.path().join("a.txt")).unwrap(),
            "one\nTWO ALREADY CHANGED\nthree\n"
        );
    }

    #[tokio::test]
    async fn a_multi_file_patch_applies_all_or_nothing() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "a-old\n").unwrap();
        std::fs::write(dir.path().join("b.txt"), "b-old\n").unwrap();
        // The second file's context does not match, so neither file changes.
        let patch = "--- a/a.txt\n+++ b/a.txt\n@@ -1,1 +1,1 @@\n-a-old\n+a-new\n\
            --- a/b.txt\n+++ b/b.txt\n@@ -1,1 +1,1 @@\n-b-WRONG\n+b-new\n";

        let err = tool()
            .invoke(json!({"patch": patch}), &ctx(dir.path()))
            .await
            .unwrap_err();

        assert_eq!(err.code, ErrCode::ToolFailed);
        assert_eq!(
            std::fs::read_to_string(dir.path().join("a.txt")).unwrap(),
            "a-old\n"
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("b.txt")).unwrap(),
            "b-old\n"
        );
    }

    #[tokio::test]
    async fn a_multi_file_patch_applies_every_file_when_all_hunks_match() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "a-old\n").unwrap();
        std::fs::write(dir.path().join("b.txt"), "b-old\n").unwrap();
        let patch = "--- a/a.txt\n+++ b/a.txt\n@@ -1,1 +1,1 @@\n-a-old\n+a-new\n\
            --- a/b.txt\n+++ b/b.txt\n@@ -1,1 +1,1 @@\n-b-old\n+b-new\n";

        tool()
            .invoke(json!({"patch": patch}), &ctx(dir.path()))
            .await
            .unwrap();

        assert_eq!(
            std::fs::read_to_string(dir.path().join("a.txt")).unwrap(),
            "a-new\n"
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("b.txt")).unwrap(),
            "b-new\n"
        );
    }

    #[tokio::test]
    async fn a_path_outside_root_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let patch = "--- a/../evil.txt\n+++ b/../evil.txt\n@@ -1,1 +1,1 @@\n-x\n+y\n";

        let err = tool()
            .invoke(json!({"patch": patch}), &ctx(dir.path()))
            .await
            .unwrap_err();
        assert_eq!(err.code, ErrCode::ToolOutsideRoot);
    }

    #[tokio::test]
    async fn a_patch_with_no_file_headers_is_invalid() {
        let dir = tempfile::tempdir().unwrap();
        let err = tool()
            .invoke(json!({"patch": "not a patch"}), &ctx(dir.path()))
            .await
            .unwrap_err();
        assert_eq!(err.code, ErrCode::ToolInvalidArgs);
    }

    #[test]
    fn parses_a_hunk_header_with_default_counts() {
        let (old_start, old_count, new_start, new_count) =
            parse_hunk_header("@@ -5 +5 @@").unwrap();
        assert_eq!((old_start, old_count, new_start, new_count), (5, 1, 5, 1));
    }

    #[test]
    fn split_lines_reports_the_trailing_newline() {
        assert_eq!(split_lines("a\nb\n"), (vec!["a", "b"], true));
        assert_eq!(split_lines("a\nb"), (vec!["a", "b"], false));
        assert_eq!(split_lines(""), (vec![], false));
    }
}
