//! The machine-readable `toml darkharness` block inside an instruction file.
//!
//! An `AGENTS.md` file (or any file in the resolved chain) may carry one
//! fenced block tagged ```` ```toml darkharness ````. This module extracts
//! that block and validates it against a narrow allowlist. See task unit
//! K2, section 4.10, and Rule 35: a repository configuration file must
//! never widen its own permissions.
//!
//! # Why this module does not depend on `dark-core`
//!
//! `crates/dark-core/src/policy/config.rs` already encodes the
//! allow/confirm/deny ordering that decides whether a new policy value is
//! more restrictive than the value it replaces. This module needs that
//! same ordering, but `dark-agentsmd` sits below `dark-core` in the
//! dependency graph (Rule 17), so it cannot depend on it. [`PolicyValue`]
//! repeats that ordering locally instead of importing it. Keep the two in
//! step: if `dark-core`'s ordering changes, update this one to match.
//!
//! # Why `tools.tier_override` may only narrow
//!
//! The build specification attaches the "only when the new value is more
//! restrictive" qualifier to `policy.read`, `policy.write`, and
//! `policy.exec`, but not to `tools.tier_override`. Read literally, a
//! repository could then raise its own tool tier. That contradicts what
//! task unit K2 says it is for ("prevent a repository from widening its own
//! permissions") and what it says it is done when ("a permission-widening
//! value is rejected"), and the specification's own worked example sets
//! `tier_override = 1`, which narrows. This module therefore accepts a
//! `tools.tier_override` only when it is no higher than the tier the model
//! already gets from its own size. A repository must not be able to hand a
//! 4B model the tools of a 32B one.
//!
//! `dark_tools::registry::resolve` holds the other half of this and
//! deliberately accepts an override in either direction, because it is a
//! mechanism and not a gate: a person's own configuration may widen the
//! tool set on a machine they control. A repository may not, and this
//! module is what stops it. Put a new restriction on a repository file
//! here, not there.

use std::collections::BTreeMap;
use std::fmt;

use dark_contract::{ErrCode, Error, Result};

/// The fence-info string that opens the machine-readable block:
/// ```` ```toml darkharness ````.
const FENCE_INFO: &str = "toml darkharness";

/// One of the three settings a policy value can take.
///
/// This mirrors `dark_core::policy::config::PolicyValue` exactly: the same
/// three variants, in the same strictness order. See the module
/// documentation for why this module cannot import that type instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicyValue {
    /// Run the action without asking.
    Allow,
    /// Ask a person, or the `--yes` flag in headless mode.
    Confirm,
    /// Refuse the action.
    Deny,
}

impl PolicyValue {
    /// Returns the strictness rank. `Allow` is loosest, `Deny` is
    /// strictest.
    fn rank(self) -> u8 {
        match self {
            Self::Allow => 0,
            Self::Confirm => 1,
            Self::Deny => 2,
        }
    }

    /// Returns `true` when `self` is at least as restrictive as `other`.
    ///
    /// A repository configuration block may only replace a policy value
    /// with one that is at least as strict as the value it replaces. See
    /// Rule 35.
    #[must_use]
    pub fn is_at_least_as_strict_as(self, other: Self) -> bool {
        self.rank() >= other.rank()
    }

    /// Parses the lowercase string form (`"allow"`, `"confirm"`,
    /// `"deny"`). Returns `None` for every other string.
    fn parse(text: &str) -> Option<Self> {
        match text {
            "allow" => Some(Self::Allow),
            "confirm" => Some(Self::Confirm),
            "deny" => Some(Self::Deny),
            _ => None,
        }
    }
}

impl fmt::Display for PolicyValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = match self {
            Self::Allow => "allow",
            Self::Confirm => "confirm",
            Self::Deny => "deny",
        };
        f.write_str(text)
    }
}

/// The policy values that a repository configuration block is judged
/// against. A block that names a value looser than the matching field here
/// is rejected; see Rule 35.
///
/// The caller builds this from the harness's actual, already-effective
/// policy configuration (for example, `dark_core::policy::PolicyConfig`
/// converted field by field) before calling [`extract`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CurrentPolicy {
    /// The current permission for a read action.
    pub read: PolicyValue,
    /// The current permission for a write action.
    pub write: PolicyValue,
    /// The current permission for a command execution.
    pub exec: PolicyValue,
    /// The highest tool tier that the model already gets from its own size,
    /// from `1` to `3`. A block that asks for a higher tier is rejected;
    /// see the note on `tools.tier_override` in the module documentation.
    pub tier: u8,
}

/// The policy narrowing that a configuration block requested and that this
/// module accepted.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PolicyOverrides {
    /// The accepted new value for `policy.read`, when the block set one.
    pub read: Option<PolicyValue>,
    /// The accepted new value for `policy.write`, when the block set one.
    pub write: Option<PolicyValue>,
    /// The accepted new value for `policy.exec`, when the block set one.
    pub exec: Option<PolicyValue>,
}

/// The keys that this module accepted out of one configuration block.
///
/// Every field starts empty. A field fills in only when the block set the
/// matching key and that key passed validation. See task unit K2, step 2.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ConfigBlock {
    /// Narrower `policy.read`, `policy.write`, and `policy.exec` values.
    pub policy: PolicyOverrides,
    /// The accepted `tools.tier_override` value, a tool tier from `1` to
    /// `3`.
    pub tools_tier_override: Option<u8>,
    /// The raw `plan.axes.<name>` tables, keyed by axis-set name. This
    /// module accepts the subtree without validating its shape; task unit
    /// `E2` owns what a well-formed axis set looks like.
    pub plan_axes: BTreeMap<String, toml::Value>,
    /// The accepted `agents_md.budget_tokens` value.
    pub agents_md_budget_tokens: Option<usize>,
}

/// One key that a configuration block named and that this module refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RejectedKey {
    /// The dotted key path, for example `policy.write_outside_root`.
    pub key: String,
    /// Why the key was rejected, for a person to read.
    pub reason: String,
}

/// The result of extracting and validating one configuration block.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ExtractOutcome {
    /// The keys the block set, and that passed validation.
    pub accepted: ConfigBlock,
    /// The keys the block set, and that this module refused. Every
    /// prohibited key produces one entry here rather than aborting the
    /// whole block, so a caller can report every problem at once.
    pub rejected: Vec<RejectedKey>,
}

/// Extracts and validates the machine-readable block in `markdown`.
///
/// Returns `Ok(None)` when `markdown` carries no fenced block tagged
/// ```` ```toml darkharness ````. When it carries the tag but the fence
/// never closes, this also returns `Ok(None)`: an unterminated fence is not
/// a machine-readable block, it is prose that happens to open one.
///
/// `current` is the policy state that a `policy.read`, `policy.write`, or
/// `policy.exec` key is judged against: only a strictly more restrictive
/// value is accepted (Rule 35).
///
/// # Errors
///
/// Returns [`ErrCode::ToolInvalidArgs`] when the block's text is present
/// but is not valid TOML. A key this module does not recognise, or a value
/// that fails validation for a key it does recognise, is not an error: it
/// is reported in [`ExtractOutcome::rejected`] instead, so the rest of the
/// block still applies.
pub fn extract(markdown: &str, current: CurrentPolicy) -> Result<Option<ExtractOutcome>> {
    let Some(block_text) = find_fenced_block(markdown) else {
        return Ok(None);
    };

    // `toml::from_str` parses a full document (table headers included).
    // `str::parse::<toml::Value>` parses one bare value expression instead
    // and rejects `[policy]` as trailing content after it, so it is the
    // wrong entry point for a fenced block, which is always a document.
    let table: toml::Table = toml::from_str(&block_text).map_err(|err| {
        Error::new(
            ErrCode::ToolInvalidArgs,
            format!("invalid toml darkharness block: {err}"),
        )
    })?;

    Ok(Some(validate(&table, current)))
}

/// Finds the first fenced block tagged ```` ```toml darkharness ```` in
/// `markdown` and returns its body, excluding the fence lines themselves.
fn find_fenced_block(markdown: &str) -> Option<String> {
    let mut lines = markdown.lines();
    while let Some(line) = lines.next() {
        let trimmed = line.trim_start();
        let Some(info) = trimmed.strip_prefix("```") else {
            continue;
        };
        if info.trim() != FENCE_INFO {
            continue;
        }

        let mut body = String::new();
        for content_line in lines.by_ref() {
            if content_line.trim_start().starts_with("```") {
                return Some(body);
            }
            body.push_str(content_line);
            body.push('\n');
        }
        // The fence never closed: treat it as absent rather than guessing
        // where it would have ended.
        return None;
    }
    None
}

/// Walks the parsed table and sorts every key into `accepted` or
/// `rejected`.
fn validate(table: &toml::value::Table, current: CurrentPolicy) -> ExtractOutcome {
    let mut outcome = ExtractOutcome::default();

    for (top_key, top_value) in table {
        match top_key.as_str() {
            "policy" => validate_policy(top_value, current, &mut outcome),
            "tools" => validate_tools(top_value, current, &mut outcome),
            "plan" => validate_plan(top_value, &mut outcome),
            "agents_md" => validate_agents_md(top_value, &mut outcome),
            other => reject(&mut outcome, other, "unrecognised key"),
        }
    }

    outcome
}

/// Adds one rejected key with `reason` to `outcome`.
fn reject(outcome: &mut ExtractOutcome, key: impl Into<String>, reason: impl Into<String>) {
    push_rejected(&mut outcome.rejected, key, reason);
}

/// Adds one rejected key with `reason` directly to a rejection list.
///
/// This is the lower-level form [`reject`] wraps: it takes the list on its
/// own, rather than the whole [`ExtractOutcome`], so a caller that already
/// holds a disjoint mutable borrow of `outcome.accepted` — for example
/// [`validate_policy_field`] — can still record a rejection.
fn push_rejected(
    rejected: &mut Vec<RejectedKey>,
    key: impl Into<String>,
    reason: impl Into<String>,
) {
    rejected.push(RejectedKey {
        key: key.into(),
        reason: reason.into(),
    });
}

/// Validates the `[policy]` table: `read`, `write`, and `exec` narrow the
/// current value or are rejected; `write_outside_root` and `default_dark`
/// are always rejected; every other subkey is rejected as unrecognised.
fn validate_policy(value: &toml::Value, current: CurrentPolicy, outcome: &mut ExtractOutcome) {
    let Some(table) = value.as_table() else {
        reject(outcome, "policy", "must be a table");
        return;
    };

    for (key, val) in table {
        match key.as_str() {
            "read" => validate_policy_field(
                val,
                "policy.read",
                current.read,
                &mut outcome.accepted.policy.read,
                &mut outcome.rejected,
            ),
            "write" => validate_policy_field(
                val,
                "policy.write",
                current.write,
                &mut outcome.accepted.policy.write,
                &mut outcome.rejected,
            ),
            "exec" => validate_policy_field(
                val,
                "policy.exec",
                current.exec,
                &mut outcome.accepted.policy.exec,
                &mut outcome.rejected,
            ),
            "write_outside_root" => reject(
                outcome,
                "policy.write_outside_root",
                "a repository configuration file can never widen this permission (Rule 34); \
                 this key is never accepted, whatever value it names",
            ),
            "default_dark" => reject(
                outcome,
                "policy.default_dark",
                "dark-mode settings are not part of the machine-readable block",
            ),
            other => reject(outcome, format!("policy.{other}"), "unrecognised key"),
        }
    }
}

/// Validates one `policy.<field>` value: parses it, then accepts it only
/// when it is at least as strict as `current_value`. Writes the accepted
/// value into `slot`, or a rejection into `rejected`.
fn validate_policy_field(
    value: &toml::Value,
    key: &str,
    current_value: PolicyValue,
    slot: &mut Option<PolicyValue>,
    rejected: &mut Vec<RejectedKey>,
) {
    let Some(text) = value.as_str() else {
        push_rejected(
            rejected,
            key,
            "must be a string: \"allow\", \"confirm\", or \"deny\"",
        );
        return;
    };
    let Some(requested) = PolicyValue::parse(text) else {
        push_rejected(
            rejected,
            key,
            format!("\"{text}\" is not one of \"allow\", \"confirm\", \"deny\""),
        );
        return;
    };
    if !requested.is_at_least_as_strict_as(current_value) {
        push_rejected(
            rejected,
            key,
            format!(
                "\"{requested}\" is less restrictive than the current value \"{current_value}\"; \
                 a repository configuration file cannot widen its own permissions (Rule 35)"
            ),
        );
        return;
    }
    *slot = Some(requested);
}

/// Validates the `[tools]` table: `tier_override` is a tool tier from `1`
/// to `3`, and only one that is no higher than `current.tier`; every other
/// subkey is rejected.
fn validate_tools(value: &toml::Value, current: CurrentPolicy, outcome: &mut ExtractOutcome) {
    let Some(table) = value.as_table() else {
        reject(outcome, "tools", "must be a table");
        return;
    };

    for (key, val) in table {
        if key == "tier_override" {
            match val.as_integer().and_then(|n| u8::try_from(n).ok()) {
                Some(tier) if !(1..=3).contains(&tier) => reject(
                    outcome,
                    "tools.tier_override",
                    "must be an integer tool tier: 1, 2, or 3",
                ),
                Some(tier) if tier > current.tier => reject(
                    outcome,
                    "tools.tier_override",
                    format!(
                        "tier {tier} is higher than tier {}, which is what this model's size \
                         already gives it; a repository configuration file cannot widen its \
                         own permissions (Rule 35)",
                        current.tier
                    ),
                ),
                Some(tier) => {
                    outcome.accepted.tools_tier_override = Some(tier);
                }
                None => reject(
                    outcome,
                    "tools.tier_override",
                    "must be an integer tool tier: 1, 2, or 3",
                ),
            }
        } else {
            reject(outcome, format!("tools.{key}"), "unrecognised key");
        }
    }
}

/// Validates the `[plan]` table: only `axes.*` is accepted, and its
/// subtree is copied through unvalidated (task unit `E2` owns its shape);
/// every other `plan` subkey is rejected.
fn validate_plan(value: &toml::Value, outcome: &mut ExtractOutcome) {
    let Some(table) = value.as_table() else {
        reject(outcome, "plan", "must be a table");
        return;
    };

    for (key, val) in table {
        if key == "axes" {
            let Some(axes_table) = val.as_table() else {
                reject(outcome, "plan.axes", "must be a table");
                continue;
            };
            for (axis_name, axis_value) in axes_table {
                outcome
                    .accepted
                    .plan_axes
                    .insert(axis_name.clone(), axis_value.clone());
            }
        } else {
            reject(outcome, format!("plan.{key}"), "unrecognised key");
        }
    }
}

/// Validates the `[agents_md]` table: only `budget_tokens` is accepted,
/// and only as a non-negative integer; every other subkey is rejected.
fn validate_agents_md(value: &toml::Value, outcome: &mut ExtractOutcome) {
    let Some(table) = value.as_table() else {
        reject(outcome, "agents_md", "must be a table");
        return;
    };

    for (key, val) in table {
        if key == "budget_tokens" {
            match val.as_integer().and_then(|n| usize::try_from(n).ok()) {
                Some(tokens) => {
                    outcome.accepted.agents_md_budget_tokens = Some(tokens);
                }
                None => reject(
                    outcome,
                    "agents_md.budget_tokens",
                    "must be a non-negative integer",
                ),
            }
        } else {
            reject(outcome, format!("agents_md.{key}"), "unrecognised key");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn permissive() -> CurrentPolicy {
        CurrentPolicy {
            read: PolicyValue::Allow,
            write: PolicyValue::Confirm,
            exec: PolicyValue::Confirm,
            tier: 3,
        }
    }

    /// The same policy, but for a model whose own size gives it tier 1
    /// only. A block that asks for a higher tier must be refused.
    fn small_model() -> CurrentPolicy {
        CurrentPolicy {
            tier: 1,
            ..permissive()
        }
    }

    fn wrap(toml_body: &str) -> String {
        format!("# Notes\n\n```toml darkharness\n{toml_body}\n```\n\nMore prose.\n")
    }

    #[test]
    fn extract_returns_none_when_no_block_is_present() {
        let outcome =
            extract("# Notes\n\nNo machine-readable block here.\n", permissive()).unwrap();
        assert!(outcome.is_none());
    }

    #[test]
    fn extract_ignores_a_toml_fence_without_the_darkharness_tag() {
        let markdown = "```toml\n[policy]\nexec = \"deny\"\n```\n";
        let outcome = extract(markdown, permissive()).unwrap();
        assert!(outcome.is_none());
    }

    #[test]
    fn extract_returns_none_for_an_unterminated_fence() {
        let markdown = "```toml darkharness\n[policy]\nexec = \"deny\"\n";
        let outcome = extract(markdown, permissive()).unwrap();
        assert!(outcome.is_none());
    }

    #[test]
    fn malformed_toml_is_an_error() {
        let markdown = wrap("not = [valid");
        let err = extract(&markdown, permissive()).unwrap_err();
        assert_eq!(err.code, ErrCode::ToolInvalidArgs);
    }

    #[test]
    fn accepts_exec_when_it_narrows_from_confirm_to_deny() {
        let markdown = wrap("[policy]\nexec = \"deny\"");
        let outcome = extract(&markdown, permissive()).unwrap().unwrap();
        assert_eq!(outcome.accepted.policy.exec, Some(PolicyValue::Deny));
        assert!(outcome.rejected.is_empty());
    }

    #[test]
    fn accepts_read_when_it_narrows_from_allow_to_confirm() {
        let markdown = wrap("[policy]\nread = \"confirm\"");
        let outcome = extract(&markdown, permissive()).unwrap().unwrap();
        assert_eq!(outcome.accepted.policy.read, Some(PolicyValue::Confirm));
    }

    #[test]
    fn rejects_exec_when_it_widens_from_confirm_to_allow() {
        let markdown = wrap("[policy]\nexec = \"allow\"");
        let outcome = extract(&markdown, permissive()).unwrap().unwrap();
        assert!(outcome.accepted.policy.exec.is_none());
        assert_eq!(outcome.rejected.len(), 1);
        assert_eq!(outcome.rejected[0].key, "policy.exec");
    }

    #[test]
    fn rejects_write_when_it_widens_from_confirm_to_allow() {
        let markdown = wrap("[policy]\nwrite = \"allow\"");
        let outcome = extract(&markdown, permissive()).unwrap().unwrap();
        assert!(outcome.accepted.policy.write.is_none());
        assert_eq!(outcome.rejected[0].key, "policy.write");
    }

    #[test]
    fn an_equal_value_counts_as_narrowing_and_is_accepted() {
        let markdown = wrap("[policy]\nexec = \"confirm\"");
        let outcome = extract(&markdown, permissive()).unwrap().unwrap();
        assert_eq!(outcome.accepted.policy.exec, Some(PolicyValue::Confirm));
    }

    #[test]
    fn rejects_write_outside_root_naming_deny() {
        let markdown = wrap("[policy]\nwrite_outside_root = \"deny\"");
        let outcome = extract(&markdown, permissive()).unwrap().unwrap();
        assert!(
            outcome
                .rejected
                .iter()
                .any(|r| r.key == "policy.write_outside_root")
        );
    }

    #[test]
    fn rejects_write_outside_root_naming_allow() {
        let markdown = wrap("[policy]\nwrite_outside_root = \"allow\"");
        let outcome = extract(&markdown, permissive()).unwrap().unwrap();
        assert!(
            outcome
                .rejected
                .iter()
                .any(|r| r.key == "policy.write_outside_root")
        );
    }

    #[test]
    fn rejects_a_dark_mode_setting() {
        let markdown = wrap("[policy]\ndefault_dark = true");
        let outcome = extract(&markdown, permissive()).unwrap().unwrap();
        assert!(
            outcome
                .rejected
                .iter()
                .any(|r| r.key == "policy.default_dark")
        );
    }

    #[test]
    fn rejects_a_model_setting() {
        let markdown = wrap("[model]\nid = \"Qwen/Qwen3-32B\"");
        let outcome = extract(&markdown, permissive()).unwrap().unwrap();
        assert!(outcome.rejected.iter().any(|r| r.key == "model"));
    }

    #[test]
    fn rejects_an_unrecognised_top_level_key() {
        let markdown = wrap("[widget]\ncolor = \"red\"");
        let outcome = extract(&markdown, permissive()).unwrap().unwrap();
        assert!(outcome.rejected.iter().any(|r| r.key == "widget"));
    }

    #[test]
    fn rejects_an_unrecognised_key_inside_a_known_table() {
        let markdown = wrap("[policy]\nnetwork = \"allow\"");
        let outcome = extract(&markdown, permissive()).unwrap().unwrap();
        assert!(outcome.rejected.iter().any(|r| r.key == "policy.network"));
    }

    #[test]
    fn accepts_tools_tier_override() {
        let markdown = wrap("[tools]\ntier_override = 1");
        let outcome = extract(&markdown, permissive()).unwrap().unwrap();
        assert_eq!(outcome.accepted.tools_tier_override, Some(1));
    }

    #[test]
    fn rejects_a_tier_override_that_raises_the_tier_the_model_earned() {
        let markdown = wrap("[tools]\ntier_override = 3");
        let outcome = extract(&markdown, small_model()).unwrap().unwrap();

        assert_eq!(outcome.accepted.tools_tier_override, None);
        let rejected = outcome
            .rejected
            .iter()
            .find(|r| r.key == "tools.tier_override")
            .expect("a widening tier override must be rejected");
        assert!(
            rejected.reason.contains("Rule 35"),
            "the reason must name the rule it breaks: {}",
            rejected.reason
        );
    }

    #[test]
    fn accepts_a_tier_override_that_narrows() {
        let markdown = wrap("[tools]\ntier_override = 1");
        let outcome = extract(&markdown, permissive()).unwrap().unwrap();

        assert_eq!(outcome.accepted.tools_tier_override, Some(1));
        assert!(outcome.rejected.is_empty());
    }

    #[test]
    fn accepts_a_tier_override_equal_to_the_tier_the_model_earned() {
        let markdown = wrap("[tools]\ntier_override = 1");
        let outcome = extract(&markdown, small_model()).unwrap().unwrap();

        assert_eq!(outcome.accepted.tools_tier_override, Some(1));
    }

    #[test]
    fn rejects_a_tier_override_out_of_range() {
        let markdown = wrap("[tools]\ntier_override = 9");
        let outcome = extract(&markdown, permissive()).unwrap().unwrap();
        assert!(
            outcome
                .rejected
                .iter()
                .any(|r| r.key == "tools.tier_override")
        );
    }

    #[test]
    fn accepts_plan_axes_wildcard() {
        let markdown =
            wrap("[plan.axes.spec]\naxes = [\"data model and invariants\", \"observability\"]");
        let outcome = extract(&markdown, permissive()).unwrap().unwrap();
        assert!(outcome.accepted.plan_axes.contains_key("spec"));
    }

    #[test]
    fn rejects_an_unrecognised_plan_subkey() {
        let markdown = wrap("[plan]\nstrategy = \"wide\"");
        let outcome = extract(&markdown, permissive()).unwrap().unwrap();
        assert!(outcome.rejected.iter().any(|r| r.key == "plan.strategy"));
    }

    #[test]
    fn accepts_agents_md_budget_tokens() {
        let markdown = wrap("[agents_md]\nbudget_tokens = 900");
        let outcome = extract(&markdown, permissive()).unwrap().unwrap();
        assert_eq!(outcome.accepted.agents_md_budget_tokens, Some(900));
    }

    #[test]
    fn rejects_an_unrecognised_agents_md_subkey() {
        let markdown = wrap("[agents_md]\nfollow_imports = true");
        let outcome = extract(&markdown, permissive()).unwrap().unwrap();
        assert!(
            outcome
                .rejected
                .iter()
                .any(|r| r.key == "agents_md.follow_imports")
        );
    }

    #[test]
    fn reports_every_problem_in_one_pass() {
        let markdown =
            wrap("[policy]\nexec = \"allow\"\nwrite_outside_root = \"deny\"\n[model]\nid = \"x\"");
        let outcome = extract(&markdown, permissive()).unwrap().unwrap();
        assert_eq!(outcome.rejected.len(), 3);
    }

    #[test]
    fn policy_value_strictness_matches_dark_core_ordering() {
        assert!(PolicyValue::Deny.is_at_least_as_strict_as(PolicyValue::Confirm));
        assert!(PolicyValue::Deny.is_at_least_as_strict_as(PolicyValue::Allow));
        assert!(PolicyValue::Confirm.is_at_least_as_strict_as(PolicyValue::Allow));
        assert!(!PolicyValue::Allow.is_at_least_as_strict_as(PolicyValue::Confirm));
        assert!(!PolicyValue::Confirm.is_at_least_as_strict_as(PolicyValue::Deny));
        assert!(PolicyValue::Confirm.is_at_least_as_strict_as(PolicyValue::Confirm));
    }
}
