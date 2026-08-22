//! Content search across the repository.
//!
//! This module builds the `grep` tool. The tool searches file contents for a
//! regular expression. It runs in process. It never starts `rg` or `git` as a
//! child process. See task unit `C2`.
//!
//! The tool walks the repository with the `ignore` crate and searches each
//! file with `grep-searcher` and `grep-regex`. These are the crates that
//! ripgrep itself uses, so the gitignore behaviour matches `git grep`:
//!
//! - The walk skips a file that `.gitignore`, `.git/info/exclude`, or the
//!   global gitignore excludes, but only inside a real Git repository. A
//!   directory with no `.git` entry searches every file, exactly as `git
//!   grep` would refuse to filter outside a repository.
//! - The walk skips hidden files and directories (dot-prefixed names), the
//!   same default that ripgrep uses. Use an explicit `path` argument to
//!   search inside a hidden directory.
//! - The walk does not follow a symbolic link. A tool never leaves the
//!   repository root, and a followed link could point outside it.

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use dark_contract::tool::tier;
use dark_contract::{ErrCode, Error, Result, Tool, ToolCtx, ToolResult, ToolSchema};
use grep_regex::{RegexMatcher, RegexMatcherBuilder};
use grep_searcher::sinks::Lossy;
use grep_searcher::{BinaryDetection, Searcher, SearcherBuilder};
use ignore::WalkBuilder;
use serde::Deserialize;

/// The largest number of matches that one call returns.
///
/// A caller may ask for fewer with `max_results`. A caller never gets more.
/// This keeps a broad pattern from flooding the model's context.
const HARD_MAX_MATCHES: usize = 200;

/// How many matches a search collects before it stops counting the exact
/// total.
///
/// The reported total gains a trailing `+` once a search reaches this count,
/// rather than paying to scan every match in a huge or repetitive file.
const COLLECT_CEILING: usize = HARD_MAX_MATCHES * 10;

/// The largest number of characters that one reported line keeps.
///
/// A longer line is cut and marked with an ellipsis. One minified line must
/// not dominate the result.
const MAX_LINE_CHARS: usize = 500;

/// The arguments that the `grep` tool accepts.
#[derive(Deserialize)]
struct Args {
    /// The regular expression to search for.
    pattern: String,
    /// A file or directory to search, relative to the repository root.
    #[serde(default)]
    path: Option<String>,
    /// Matches the pattern without regard to letter case.
    #[serde(default)]
    case_insensitive: bool,
    /// Treats the pattern as literal text instead of a regular expression.
    #[serde(default)]
    fixed_strings: bool,
    /// The largest number of matches to return. Capped at [`HARD_MAX_MATCHES`].
    #[serde(default)]
    max_results: Option<usize>,
}

/// One matched line.
struct MatchLine {
    /// The absolute path of the file that holds this line.
    path: PathBuf,
    /// The 1-based line number.
    line: u64,
    /// The line text, trimmed and capped. See [`clip_line`].
    text: String,
}

/// Escapes regular-expression metacharacters.
///
/// The `fixed_strings` argument uses this function to turn the caller's text
/// into a pattern that matches only that literal text.
fn escape_regex(pattern: &str) -> String {
    let mut escaped = String::with_capacity(pattern.len());
    for ch in pattern.chars() {
        if matches!(
            ch,
            '\\' | '.' | '+' | '*' | '?' | '(' | ')' | '|' | '[' | ']' | '{' | '}' | '^' | '$'
        ) {
            escaped.push('\\');
        }
        escaped.push(ch);
    }
    escaped
}

/// Trims the line terminator and caps the length of one reported line.
fn clip_line(text: &str) -> String {
    let trimmed = text.trim_end_matches(['\n', '\r']);
    if trimmed.chars().count() > MAX_LINE_CHARS {
        let clipped: String = trimmed.chars().take(MAX_LINE_CHARS).collect();
        format!("{clipped}…")
    } else {
        trimmed.to_string()
    }
}

/// Lists the files under `target`, sorted by path.
///
/// The sort uses `Path`'s own byte-based ordering, not locale collation, so
/// the result is the same on every run. See Rule 30.
fn walk_files(target: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let walker = WalkBuilder::new(target).follow_links(false).build();
    for entry in walker {
        match entry {
            Ok(entry) => {
                if entry
                    .file_type()
                    .is_some_and(|file_type| file_type.is_file())
                {
                    files.push(entry.into_path());
                }
            }
            Err(err) => {
                // An unreadable directory entry does not fail the whole
                // search. The caller still gets the matches from every file
                // the walk could read.
                tracing::debug!(error = %err, "search skipped an unreadable entry");
            }
        }
    }
    files.sort();
    files
}

/// Searches one file and appends its matches to `out`.
///
/// Returns `true` once `out` holds `ceiling` matches, so the caller can stop
/// walking further files.
fn search_file(
    matcher: &RegexMatcher,
    searcher: &mut Searcher,
    path: &Path,
    out: &mut Vec<MatchLine>,
    ceiling: usize,
) -> bool {
    let result = searcher.search_path(
        matcher,
        path,
        Lossy(|line_number, text| {
            out.push(MatchLine {
                path: path.to_path_buf(),
                line: line_number,
                text: clip_line(text),
            });
            Ok(out.len() < ceiling)
        }),
    );
    if let Err(err) = result {
        // A file that fails to open or read (for example, a permission
        // error) does not fail the whole search.
        tracing::debug!(path = %path.display(), error = %err, "search skipped a file");
    }
    out.len() >= ceiling
}

/// Renders the matches as text for the model to read.
///
/// `total` is the match count, with a trailing `+` when the search reached
/// [`COLLECT_CEILING`] before it could count every match.
fn format_content(
    pattern: &str,
    root: &Path,
    shown: &[MatchLine],
    truncated: bool,
    total: &str,
) -> String {
    use std::fmt::Write as _;

    if shown.is_empty() {
        return format!("No matches for `{pattern}`.");
    }
    let mut out = format!("{total} match(es) for `{pattern}`:\n");
    for m in shown {
        let rel = m.path.strip_prefix(root).unwrap_or(&m.path);
        let _ = writeln!(out, "{}:{}: {}", rel.display(), m.line, m.text);
    }
    if truncated {
        let _ = write!(
            out,
            "\n[truncated: showing {} of {total} matches. Narrow the pattern or the path to see more.]\n",
            shown.len()
        );
    }
    out
}

/// The `grep` tool. It searches file contents for a regular expression.
///
/// The search walks the repository with the `ignore` crate, so it skips the
/// files that `.gitignore` excludes, in the same way that `git grep` does.
#[derive(Debug, Clone, Copy, Default)]
pub struct GrepTool;

impl GrepTool {
    /// Creates the tool.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for GrepTool {
    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "grep".to_string(),
            description: format!(
                "Search file contents in the repository for a regular expression. \
                 Skips the files that `.gitignore` excludes. Returns at most \
                 {HARD_MAX_MATCHES} matches."
            ),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "pattern": {
                        "type": "string",
                        "description": "The regular expression to search for."
                    },
                    "path": {
                        "type": "string",
                        "description": "A file or directory to search, relative to the repository root. Defaults to the whole repository."
                    },
                    "case_insensitive": {
                        "type": "boolean",
                        "description": "Match the pattern without regard to letter case. Defaults to false."
                    },
                    "fixed_strings": {
                        "type": "boolean",
                        "description": "Treat the pattern as literal text instead of a regular expression. Defaults to false."
                    },
                    "max_results": {
                        "type": "integer",
                        "description": format!(
                            "The largest number of matches to return. Defaults to {HARD_MAX_MATCHES}. The harness never returns more than {HARD_MAX_MATCHES}."
                        )
                    }
                },
                "required": ["pattern"],
                "additionalProperties": false
            }),
            tier: tier::STANDARD,
            mutating: false,
        }
    }

    async fn invoke(&self, args: serde_json::Value, ctx: &ToolCtx) -> Result<ToolResult> {
        let args: Args = serde_json::from_value(args).map_err(|err| {
            Error::new(
                ErrCode::ToolInvalidArgs,
                format!("the grep arguments are not valid: {err}"),
            )
        })?;

        if args.pattern.is_empty() {
            return Err(Error::new(
                ErrCode::ToolInvalidArgs,
                "the pattern must not be empty",
            ));
        }

        let root = ctx.root.canonicalize().map_err(|err| {
            Error::new(
                ErrCode::ToolFailed,
                format!("cannot resolve the repository root: {err}"),
            )
        })?;

        let requested = args
            .path
            .as_deref()
            .map_or_else(|| ctx.root.clone(), |sub| ctx.root.join(sub));

        if !requested.exists() {
            return Err(Error::new(
                ErrCode::ToolNotFound,
                format!("no such file or directory: {}", requested.display()),
            ));
        }

        let target = requested.canonicalize().map_err(|err| {
            Error::new(
                ErrCode::ToolFailed,
                format!("cannot resolve the search path: {err}"),
            )
        })?;

        if !target.starts_with(&root) {
            return Err(Error::new(
                ErrCode::ToolOutsideRoot,
                format!("{} is outside the repository root", requested.display()),
            ));
        }

        let pattern_source = if args.fixed_strings {
            escape_regex(&args.pattern)
        } else {
            args.pattern.clone()
        };

        let mut matcher_builder = RegexMatcherBuilder::new();
        matcher_builder.case_insensitive(args.case_insensitive);
        let matcher = matcher_builder.build(&pattern_source).map_err(|err| {
            Error::new(
                ErrCode::ToolInvalidArgs,
                format!("the pattern is not a valid regular expression: {err}"),
            )
        })?;

        let mut searcher = SearcherBuilder::new()
            .binary_detection(BinaryDetection::quit(0))
            .line_number(true)
            .build();

        let cap = args.max_results.map_or(HARD_MAX_MATCHES, |requested_cap| {
            requested_cap.clamp(1, HARD_MAX_MATCHES)
        });
        let ceiling = COLLECT_CEILING.max(cap);

        let mut found = Vec::new();
        let ceiling_hit = if target.is_file() {
            search_file(&matcher, &mut searcher, &target, &mut found, ceiling)
        } else {
            let mut hit = false;
            for file in walk_files(&target) {
                if search_file(&matcher, &mut searcher, &file, &mut found, ceiling) {
                    hit = true;
                    break;
                }
            }
            hit
        };

        let truncated = found.len() > cap || ceiling_hit;
        let total_display = if ceiling_hit {
            format!("{}+", found.len())
        } else {
            found.len().to_string()
        };
        let shown = if found.len() > cap {
            &found[..cap]
        } else {
            &found[..]
        };

        Ok(ToolResult::ok(format_content(
            &args.pattern,
            &root,
            shown,
            truncated,
            &total_display,
        )))
    }
}

#[cfg(test)]
mod tests {
    use std::fmt::Write as _;
    use std::fs;
    use std::path::Path;
    use std::process::Command;

    use dark_contract::EventBus;
    use tempfile::TempDir;
    use tokio_util::sync::CancellationToken;

    use super::*;

    fn ctx_for(root: &Path) -> ToolCtx {
        let bus = EventBus::new();
        ToolCtx {
            root: root.to_path_buf(),
            events: bus.tx(),
            cancel: CancellationToken::new(),
            dark: false,
            human_present: false,
        }
    }

    /// Writes `contents` to a file at `dir` joined with each of `rel`'s
    /// components, creating parent directories as needed.
    ///
    /// Building the path from components, instead of a string with a
    /// hardcoded separator, keeps the fixture correct on every platform.
    fn write(dir: &Path, rel: &[&str], contents: &str) {
        let mut path = dir.to_path_buf();
        for part in rel {
            path.push(part);
        }
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, contents).unwrap();
    }

    fn git(dir: &Path, args: &[&str]) {
        let status = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .status()
            .expect("git is installed");
        assert!(status.success(), "git {args:?} failed");
    }

    async fn run_grep(root: &Path, args: serde_json::Value) -> Result<ToolResult> {
        let tool = GrepTool::new();
        let ctx = ctx_for(root);
        tool.invoke(args, &ctx).await
    }

    /// Parses a line of this tool's own report format, `path:line: text`,
    /// into `(path, line)`. Header and footer lines have no digit-only
    /// second column, so they parse to `None` and drop out of comparisons.
    fn parse_match_line(line: &str) -> Option<(String, String)> {
        let mut parts = line.splitn(3, ':');
        let path = parts.next()?.to_string();
        let lineno = parts.next()?.to_string();
        parts.next()?;
        if lineno.parse::<u64>().is_ok() {
            Some((path, lineno))
        } else {
            None
        }
    }

    #[test]
    fn schema_describes_grep() {
        let schema = GrepTool::new().schema();
        assert_eq!(schema.name, "grep");
        assert_eq!(schema.tier, tier::STANDARD);
        assert!(!schema.mutating);
        let required = schema.parameters["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v == "pattern"));
    }

    #[tokio::test]
    async fn finds_a_matching_line() {
        let dir = TempDir::new().unwrap();
        write(dir.path(), &["a.txt"], "hello world\nneedle here\n");

        let result = run_grep(dir.path(), serde_json::json!({"pattern": "needle"}))
            .await
            .unwrap();

        assert!(!result.is_error);
        assert!(result.content.contains("needle here"));
        assert!(result.content.contains("a.txt"));
    }

    #[tokio::test]
    async fn reports_no_matches_without_erroring() {
        let dir = TempDir::new().unwrap();
        write(dir.path(), &["a.txt"], "hello world\n");

        let result = run_grep(dir.path(), serde_json::json!({"pattern": "needle"}))
            .await
            .unwrap();

        assert!(!result.is_error);
        assert!(result.content.contains("No matches"));
    }

    #[tokio::test]
    async fn excludes_gitignored_files() {
        let dir = TempDir::new().unwrap();
        git(dir.path(), &["init", "-q"]);
        write(dir.path(), &[".gitignore"], "ignored.txt\n");
        write(dir.path(), &["visible.txt"], "needle in visible\n");
        write(dir.path(), &["ignored.txt"], "needle in ignored\n");

        let result = run_grep(dir.path(), serde_json::json!({"pattern": "needle"}))
            .await
            .unwrap();

        assert!(result.content.contains("visible.txt"));
        assert!(!result.content.contains("ignored.txt"));
    }

    #[tokio::test]
    async fn matches_git_grep_on_the_fixture_repository() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        git(root, &["init", "-q"]);
        write(root, &[".gitignore"], "ignored.txt\n");
        write(root, &["visible.txt"], "needle in visible\nsecond line\n");
        write(root, &["another.rs"], "// needle in a comment\nfn f() {}\n");
        write(root, &["ignored.txt"], "needle in ignored\n");
        git(root, &["add", "-A"]);

        let git_output = Command::new("git")
            .arg("-C")
            .arg(root)
            .args(["grep", "-n", "--no-color", "-e", "needle"])
            .output()
            .expect("git grep runs");
        assert!(
            git_output.status.success(),
            "git grep failed: {}",
            String::from_utf8_lossy(&git_output.stderr)
        );
        let git_stdout = String::from_utf8_lossy(&git_output.stdout).into_owned();
        let mut git_pairs: Vec<(String, String)> =
            git_stdout.lines().filter_map(parse_match_line).collect();
        git_pairs.sort();

        let result = run_grep(root, serde_json::json!({"pattern": "needle"}))
            .await
            .unwrap();
        let mut tool_pairs: Vec<(String, String)> = result
            .content
            .lines()
            .filter_map(parse_match_line)
            .collect();
        tool_pairs.sort();

        assert_eq!(git_pairs, tool_pairs);
        assert!(!tool_pairs.is_empty());
        assert!(!tool_pairs.iter().any(|(path, _)| path.contains("ignored")));
    }

    #[tokio::test]
    async fn truncates_and_reports_when_matches_exceed_the_cap() {
        let dir = TempDir::new().unwrap();
        let mut content = String::new();
        for i in 0..(HARD_MAX_MATCHES + 25) {
            let _ = writeln!(content, "needle {i}");
        }
        write(dir.path(), &["big.txt"], &content);

        let result = run_grep(dir.path(), serde_json::json!({"pattern": "needle"}))
            .await
            .unwrap();

        assert!(!result.is_error);
        assert!(result.content.contains("truncated"));
        let shown = result
            .content
            .lines()
            .filter(|line| line.starts_with("big.txt:"))
            .count();
        assert_eq!(shown, HARD_MAX_MATCHES);
    }

    #[tokio::test]
    async fn max_results_can_lower_the_cap_but_not_raise_it() {
        let dir = TempDir::new().unwrap();
        let mut content = String::new();
        for i in 0..10 {
            let _ = writeln!(content, "needle {i}");
        }
        write(dir.path(), &["f.txt"], &content);

        let lowered = run_grep(
            dir.path(),
            serde_json::json!({"pattern": "needle", "max_results": 3}),
        )
        .await
        .unwrap();
        let shown = lowered
            .content
            .lines()
            .filter(|line| line.starts_with("f.txt:"))
            .count();
        assert_eq!(shown, 3);
        assert!(lowered.content.contains("truncated"));

        let raised = run_grep(
            dir.path(),
            serde_json::json!({"pattern": "needle", "max_results": HARD_MAX_MATCHES * 10}),
        )
        .await
        .unwrap();
        let shown_raised = raised
            .content
            .lines()
            .filter(|line| line.starts_with("f.txt:"))
            .count();
        assert_eq!(shown_raised, 10);
        assert!(!raised.content.contains("truncated"));
    }

    #[tokio::test]
    async fn rejects_a_path_outside_the_root() {
        let base = TempDir::new().unwrap();
        let root = base.path().join("repo");
        fs::create_dir_all(&root).unwrap();
        write(base.path(), &["outside.txt"], "needle\n");

        let result = run_grep(
            &root,
            serde_json::json!({"pattern": "needle", "path": "../outside.txt"}),
        )
        .await;

        assert_eq!(result.unwrap_err().code, ErrCode::ToolOutsideRoot);
    }

    #[tokio::test]
    async fn rejects_a_missing_path() {
        let dir = TempDir::new().unwrap();

        let result = run_grep(
            dir.path(),
            serde_json::json!({"pattern": "needle", "path": "nope.txt"}),
        )
        .await;

        assert_eq!(result.unwrap_err().code, ErrCode::ToolNotFound);
    }

    #[tokio::test]
    async fn rejects_an_invalid_pattern() {
        let dir = TempDir::new().unwrap();

        let result = run_grep(dir.path(), serde_json::json!({"pattern": "("})).await;

        assert_eq!(result.unwrap_err().code, ErrCode::ToolInvalidArgs);
    }

    #[tokio::test]
    async fn rejects_an_empty_pattern() {
        let dir = TempDir::new().unwrap();

        let result = run_grep(dir.path(), serde_json::json!({"pattern": ""})).await;

        assert_eq!(result.unwrap_err().code, ErrCode::ToolInvalidArgs);
    }

    #[tokio::test]
    async fn rejects_a_missing_pattern_field() {
        let dir = TempDir::new().unwrap();

        let result = run_grep(dir.path(), serde_json::json!({})).await;

        assert_eq!(result.unwrap_err().code, ErrCode::ToolInvalidArgs);
    }

    #[tokio::test]
    async fn case_insensitive_matches_regardless_of_case() {
        let dir = TempDir::new().unwrap();
        write(dir.path(), &["a.txt"], "NEEDLE here\n");

        let result = run_grep(
            dir.path(),
            serde_json::json!({"pattern": "needle", "case_insensitive": true}),
        )
        .await
        .unwrap();

        assert!(result.content.contains("NEEDLE here"));
    }

    #[tokio::test]
    async fn fixed_strings_treats_the_pattern_as_literal_text() {
        let dir = TempDir::new().unwrap();
        write(dir.path(), &["a.txt"], "a.b\naxb\n");

        let result = run_grep(
            dir.path(),
            serde_json::json!({"pattern": "a.b", "fixed_strings": true}),
        )
        .await
        .unwrap();

        assert!(result.content.contains("a.b"));
        assert!(!result.content.contains("axb"));
    }

    #[tokio::test]
    async fn a_path_argument_restricts_the_search() {
        let dir = TempDir::new().unwrap();
        write(dir.path(), &["top.txt"], "needle top\n");
        write(dir.path(), &["sub", "nested.txt"], "needle nested\n");

        let result = run_grep(
            dir.path(),
            serde_json::json!({"pattern": "needle", "path": "sub"}),
        )
        .await
        .unwrap();

        assert!(result.content.contains("nested.txt"));
        assert!(!result.content.contains("top.txt"));
    }

    #[tokio::test]
    async fn a_path_argument_can_name_a_single_file() {
        let dir = TempDir::new().unwrap();
        write(dir.path(), &["top.txt"], "needle top\n");
        write(dir.path(), &["sub", "nested.txt"], "needle nested\n");

        let result = run_grep(
            dir.path(),
            serde_json::json!({"pattern": "needle", "path": "top.txt"}),
        )
        .await
        .unwrap();

        assert!(result.content.contains("top.txt"));
        assert!(!result.content.contains("nested.txt"));
    }
}
