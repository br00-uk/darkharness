//! Gates the tool set that a session offers to the model, by model size.
//!
//! A model that is too small handles a long tool list poorly: it picks the
//! wrong tool, or it forgets a required argument. [`resolve`] reads
//! `Caps::params_b` once, at the start of a session, and returns the
//! smaller tool set that a model of that size handles well. See task unit
//! `C4`.
//!
//! The tool schemas that this module returns sit in the context prefix (see
//! Rule 5). Call [`resolve`] once per session, before the first turn, and
//! keep the result for the rest of the session. Calling it again with a
//! different result, mid-session, would change the prefix and force a full
//! prefill.
//!
//! # The gate
//!
//! | Model size | Tiers | Maximum tools | Tool calls each turn |
//! | --- | --- | --- | --- |
//! | Below 8B | 1 | 5 | 1 |
//! | 8B to 32B | 1 and 2 | 12 | Many |
//! | Above 32B | 1, 2, and 3 | All | Many |
//!
//! `8B` and `32B` are boundaries, not open intervals, and the build
//! specification states them two different ways in the same paragraph: the
//! table reads "8B to 32B" then "Above 32B", which taken literally would put
//! a model at exactly `params_b = 32.0` in both rows. This module resolves
//! that by treating `32.0` as the start of the "Above 32B" row, because
//! `dark-engine-fake` already ships `FakeEngine::large_caps`, fixed at
//! `params_b = 32.0`, and its own documentation calls it out as the fixture
//! for "the paths that need a large model: native tool parsing, log
//! probabilities, and tier 3 tools." Treating `32.0` as the top of the "8B
//! to 32B" row instead would make that fixture unable to reach the row its
//! own documentation says it reaches. A model at exactly `params_b = 8.0`
//! falls in the "8B to 32B" row; nothing else in the build specification
//! contests that boundary.
//!
//! # The override
//!
//! A repository's `AGENTS.md` may set `tools.tier_override` (task unit
//! `K2`). `dark-tools` has no dependency on the crate that reads
//! `AGENTS.md` (Rule 16), so this module cannot read that configuration
//! itself. The caller reads `tools.tier_override` and passes the value to
//! [`resolve`]. An override picks the whole row of the gating table that
//! matches the requested tier, in place of the row that `params_b` would
//! otherwise pick: a `tier_override` of `1` gates the session exactly as
//! the "Below 8B" row would, tool cap and call budget included, whatever
//! the model's real size is. The table's three columns move together
//! because the columns are not independent settings; they are three facts
//! about one size class. The build specification does not say whether an
//! override may only lower the row or may also raise it; this module
//! accepts either direction. An out-of-range override clamps to the
//! nearest valid tier instead of producing an empty, or an unbounded, tool
//! set.

use dark_contract::tool::tier;
use dark_contract::{Caps, Tool, ToolSchema};

use crate::exec::ExecTool;
use crate::fs::file_tools;
use crate::search::GrepTool;

/// How many tool calls a turn may issue.
///
/// A below-8B model loses track of its own state after more than one tool
/// call in a turn, so the harness holds it to one call. A larger model
/// plans a short sequence of calls reliably.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolCallLimit {
    /// The turn may issue exactly one tool call.
    One,
    /// The harness places no limit of its own on the turn.
    Many,
}

/// The tools that one session offers to the model, gated by model size.
pub struct GatedTools {
    /// The tools, in a fixed order that the harness never reorders during a
    /// session. See Rule 5.
    pub tools: Vec<Box<dyn Tool>>,
    /// How many tool calls one turn may issue.
    pub max_calls_per_turn: ToolCallLimit,
}

impl GatedTools {
    /// Returns the schema of every tool in this set, in the same order.
    ///
    /// This is what the harness sends to the model as the tool list for the
    /// context prefix.
    #[must_use]
    pub fn schemas(&self) -> Vec<ToolSchema> {
        self.tools.iter().map(|tool| tool.schema()).collect()
    }
}

/// One row of the model-size gating table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Profile {
    /// The highest tier that a model of this size sees.
    tier_ceiling: u8,
    /// The largest number of tools the harness offers. `None` means every
    /// tool that the tier ceiling admits, with no further cap.
    tool_cap: Option<usize>,
    /// How many tool calls one turn may issue.
    call_limit: ToolCallLimit,
}

/// The gating row for a model below 8B parameters.
const SMALL: Profile = Profile {
    tier_ceiling: tier::ESSENTIAL,
    tool_cap: Some(5),
    call_limit: ToolCallLimit::One,
};

/// The gating row for a model from 8B up to, but not including, 32B
/// parameters.
const MID: Profile = Profile {
    tier_ceiling: tier::STANDARD,
    tool_cap: Some(12),
    call_limit: ToolCallLimit::Many,
};

/// The gating row for a model at or above 32B parameters.
const LARGE: Profile = Profile {
    tier_ceiling: tier::ADVANCED,
    tool_cap: None,
    call_limit: ToolCallLimit::Many,
};

/// Picks the gating row for a model with `params_b` billion parameters.
///
/// See the module documentation for why `32.0` falls in the [`LARGE`] row.
fn profile_for(params_b: f32) -> Profile {
    if params_b < 8.0 {
        SMALL
    } else if params_b < 32.0 {
        MID
    } else {
        LARGE
    }
}

/// Picks the gating row whose tier ceiling is `tier_ceiling`.
///
/// `tier_ceiling` must already be clamped to a valid tier (see
/// [`tier::ESSENTIAL`] and [`tier::ADVANCED`]); this function maps each of
/// the three valid values to the one row that ceiling names.
fn profile_for_tier(tier_ceiling: u8) -> Profile {
    if tier_ceiling <= tier::ESSENTIAL {
        SMALL
    } else if tier_ceiling <= tier::STANDARD {
        MID
    } else {
        LARGE
    }
}

/// Returns every tool that this crate builds, unfiltered.
///
/// [`resolve`] filters and orders this list before it reaches a model; a
/// caller never sees this unfiltered list.
fn all_tools() -> Vec<Box<dyn Tool>> {
    let mut tools = file_tools();
    tools.push(Box::new(GrepTool::new()));
    tools.push(Box::new(ExecTool::new()));
    tools
}

/// Filters `tools` down to the ones that `max_tier` admits, orders the
/// result deterministically, and caps the count at `max_tools`.
///
/// The order is tier first, then name. Sorting the same input the same way
/// on every call is what keeps the schema list, and so the context prefix,
/// identical across a session's round trips. See Rule 5. When the set is
/// larger than `max_tools`, the tier-then-name order also decides which
/// tools survive the cut: the essential tier keeps its tools before a
/// standard-tier tool takes a slot.
fn apply_profile(
    tools: Vec<Box<dyn Tool>>,
    max_tier: u8,
    max_tools: Option<usize>,
) -> Vec<Box<dyn Tool>> {
    let mut gated: Vec<Box<dyn Tool>> = tools
        .into_iter()
        .filter(|tool| tool.schema().tier <= max_tier)
        .collect();

    gated.sort_by(|left, right| {
        let left = left.schema();
        let right = right.schema();
        left.tier
            .cmp(&right.tier)
            .then_with(|| left.name.cmp(&right.name))
    });

    if let Some(max_tools) = max_tools {
        gated.truncate(max_tools);
    }

    gated
}

/// Resolves the tool set for one session.
///
/// Reads `Caps::params_b` from `caps` to pick a row of the gating table.
/// Pass `tier_override` when the repository's `AGENTS.md` sets
/// `tools.tier_override` (task unit `K2`); pass `None` otherwise, and this
/// function picks the row from `caps.params_b` alone. Call this once, at
/// the start of a session, and reuse the result for every turn in that
/// session. Do not call it again mid-session with a different result; see
/// the module documentation and Rule 5.
#[must_use]
pub fn resolve(caps: &Caps, tier_override: Option<u8>) -> GatedTools {
    let profile = tier_override.map_or_else(
        || profile_for(caps.params_b),
        |requested| profile_for_tier(requested.clamp(tier::ESSENTIAL, tier::ADVANCED)),
    );

    GatedTools {
        tools: apply_profile(all_tools(), profile.tier_ceiling, profile.tool_cap),
        max_calls_per_turn: profile.call_limit,
    }
}

#[cfg(test)]
mod tests {
    use async_trait::async_trait;
    use dark_contract::{Device, Result, ToolCtx, ToolResult};
    use dark_engine_fake::FakeEngine;

    use super::*;

    /// Builds a minimal `Caps` value for a model with `params_b` billion
    /// parameters. The other fields do not matter to this module.
    fn caps(params_b: f32) -> Caps {
        Caps {
            model_id: "test/model".to_owned(),
            max_context: 8192,
            granted_context: 8192,
            native_tools: false,
            thinking: false,
            grammar: false,
            vision: false,
            logprobs: false,
            params_b,
            quant: "q4k".to_owned(),
            device: Device::Cpu,
            measured_tok_s: None,
        }
    }

    /// Returns the tool names of a gated set, in order.
    fn names(gated: &GatedTools) -> Vec<String> {
        gated.tools.iter().map(|tool| tool.schema().name).collect()
    }

    /// A tool used only to test [`apply_profile`] against counts and tiers
    /// that the crate's real tool set does not, today, reach.
    #[derive(Debug)]
    struct FakeTool {
        name: &'static str,
        tier: u8,
    }

    #[async_trait]
    impl Tool for FakeTool {
        fn schema(&self) -> ToolSchema {
            ToolSchema {
                name: self.name.to_owned(),
                description: "a fake tool used only in the registry tests".to_owned(),
                parameters: serde_json::json!({}),
                tier: self.tier,
                mutating: false,
            }
        }

        async fn invoke(&self, _args: serde_json::Value, _ctx: &ToolCtx) -> Result<ToolResult> {
            Ok(ToolResult::ok("fake"))
        }
    }

    fn fake(name: &'static str, tier: u8) -> Box<dyn Tool> {
        Box::new(FakeTool { name, tier })
    }

    /// Returns the tool names from a plain `Vec<Box<dyn Tool>>`, in order,
    /// as owned strings, since a schema's name does not outlive the call
    /// that produced it.
    fn schema_names(tools: &[Box<dyn Tool>]) -> Vec<String> {
        tools.iter().map(|tool| tool.schema().name).collect()
    }

    /// Turns a list of string literals into the owned `Vec<String>` that
    /// [`schema_names`] returns, so a test can compare the two directly.
    fn owned(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| (*name).to_owned()).collect()
    }

    // -- resolve(): the real tool set, gated by size -----------------------

    #[test]
    fn a_below_8b_model_sees_exactly_the_tier_1_tools() {
        let gated = resolve(&caps(4.0), None);
        assert_eq!(
            names(&gated),
            owned(&[
                "edit_file",
                "list_dir",
                "read_file",
                "run_command",
                "write_file"
            ])
        );
        assert_eq!(gated.max_calls_per_turn, ToolCallLimit::One);
    }

    #[test]
    fn a_mid_size_model_adds_the_tier_2_tools_and_allows_many_calls() {
        let gated = resolve(&caps(14.0), None);
        assert_eq!(
            names(&gated),
            owned(&[
                "edit_file",
                "list_dir",
                "read_file",
                "run_command",
                "write_file",
                "apply_patch",
                "glob",
                "grep",
            ])
        );
        assert_eq!(gated.max_calls_per_turn, ToolCallLimit::Many);
    }

    #[test]
    fn a_large_model_sees_every_tool_the_crate_builds() {
        let gated = resolve(&caps(70.0), None);
        // The crate has no tier-3 tool yet, so this is the same eight tools
        // as the mid-size row; the large row differs in having no cap, not
        // in which real tools currently pass its tier filter.
        assert_eq!(gated.tools.len(), 8);
        assert_eq!(gated.max_calls_per_turn, ToolCallLimit::Many);
    }

    #[test]
    fn a_model_at_exactly_8b_gets_the_mid_row() {
        let gated = resolve(&caps(8.0), None);
        assert_eq!(gated.tools.len(), 8);
        assert_eq!(gated.max_calls_per_turn, ToolCallLimit::Many);
    }

    #[test]
    fn a_model_just_below_8b_gets_the_small_row() {
        let gated = resolve(&caps(7.999), None);
        assert_eq!(gated.tools.len(), 5);
        assert_eq!(gated.max_calls_per_turn, ToolCallLimit::One);
    }

    #[test]
    fn a_model_just_below_32b_still_gets_the_mid_row() {
        let gated = resolve(&caps(31.999), None);
        assert_eq!(gated.tools.len(), 8);
        assert_eq!(gated.max_calls_per_turn, ToolCallLimit::Many);
    }

    #[test]
    fn a_model_at_exactly_32b_gets_the_large_row() {
        // FakeEngine::large_caps is fixed at params_b = 32.0 and is the
        // fixture the workspace already ships for "tier 3 tools"; this
        // module's boundary choice makes that fixture reach the large row.
        // See the module documentation.
        let gated = resolve(&FakeEngine::large_caps(), None);
        assert_eq!(gated.tools.len(), 8);
        assert_eq!(gated.max_calls_per_turn, ToolCallLimit::Many);
    }

    // -- resolve(): the session-start-only contract -------------------------

    #[test]
    fn resolving_the_same_caps_twice_produces_an_identical_schema_list() {
        // The result feeds the context prefix; a caller that resolves twice
        // for the same caps must see the same bytes both times. See Rule 5.
        let first = resolve(&caps(14.0), None).schemas();
        let second = resolve(&caps(14.0), None).schemas();
        assert_eq!(first, second);
    }

    // -- resolve(): tools.tier_override --------------------------------------

    #[test]
    fn an_override_can_narrow_a_large_model_to_the_below_8b_row() {
        let gated = resolve(&caps(70.0), Some(tier::ESSENTIAL));
        assert_eq!(
            names(&gated),
            owned(&[
                "edit_file",
                "list_dir",
                "read_file",
                "run_command",
                "write_file"
            ])
        );
        // The override picks the whole "Below 8B" row, call budget included,
        // not only which tools are offered.
        assert_eq!(gated.max_calls_per_turn, ToolCallLimit::One);
    }

    #[test]
    fn an_override_can_widen_a_small_model_to_the_mid_row() {
        let gated = resolve(&caps(4.0), Some(tier::STANDARD));
        assert_eq!(gated.tools.len(), 8);
        // The override picks the whole "8B to 32B" row, call budget
        // included, not only which tools are offered.
        assert_eq!(gated.max_calls_per_turn, ToolCallLimit::Many);
    }

    #[test]
    fn an_override_below_the_lowest_tier_clamps_up_instead_of_emptying_the_set() {
        let gated = resolve(&caps(14.0), Some(0));
        assert_eq!(
            names(&gated),
            owned(&[
                "edit_file",
                "list_dir",
                "read_file",
                "run_command",
                "write_file"
            ])
        );
        assert_eq!(gated.max_calls_per_turn, ToolCallLimit::One);
    }

    #[test]
    fn an_override_above_the_highest_tier_clamps_down() {
        let unclamped_would_differ = resolve(&caps(4.0), Some(200));
        let clamped_to_advanced = resolve(&caps(4.0), Some(tier::ADVANCED));
        assert_eq!(names(&unclamped_would_differ), names(&clamped_to_advanced));
    }

    // -- apply_profile(): the cap and the ordering, isolated from the crate's
    //    current, small real tool set ---------------------------------------

    #[test]
    fn apply_profile_filters_by_tier_when_uncapped() {
        let tools = vec![
            fake("c", tier::ADVANCED),
            fake("a", tier::ESSENTIAL),
            fake("b", tier::STANDARD),
        ];

        let gated = apply_profile(tools, tier::STANDARD, None);

        assert_eq!(schema_names(&gated), owned(&["a", "b"]));
    }

    #[test]
    fn apply_profile_orders_by_tier_then_name_and_caps_the_count() {
        let tools = vec![
            fake("zzz_tier1", tier::ESSENTIAL),
            fake("mmm_tier1", tier::ESSENTIAL),
            fake("aaa_tier1", tier::ESSENTIAL),
            fake("bbb_tier2", tier::STANDARD),
            fake("ccc_tier2", tier::STANDARD),
            fake("ddd_tier3", tier::ADVANCED),
        ];

        let gated = apply_profile(tools, tier::ADVANCED, Some(4));

        // Tier 1 fills the cap alphabetically before a tier-2 tool takes a
        // slot; tier 3 never gets a slot at all at this cap.
        assert_eq!(
            schema_names(&gated),
            owned(&["aaa_tier1", "mmm_tier1", "zzz_tier1", "bbb_tier2"])
        );
    }

    #[test]
    fn apply_profile_with_no_cap_keeps_every_admitted_tool() {
        let tools = vec![
            fake("b", tier::STANDARD),
            fake("a", tier::ESSENTIAL),
            fake("c", tier::ADVANCED),
        ];

        let gated = apply_profile(tools, tier::ADVANCED, None);

        assert_eq!(schema_names(&gated), owned(&["a", "b", "c"]));
    }
}
