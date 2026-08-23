//! The axis sets that stage 3 (axis sweep) enumerates against, and the
//! destination type that picks one.
//!
//! Task unit `E2`, Do step 1, gives the three built-in axis sets verbatim as
//! a `[plan.axes.*]` TOML document. Do step 1 also says "The configuration
//! and the AGENTS.md block override them": `dark-plan` does not parse
//! `AGENTS.md` itself (that is `dark-agentsmd`'s job, task units `K1` to
//! `K3`, and `dark-plan` does not depend on it), so [`AxisSets::from_toml`]
//! takes the override as plain TOML text however the caller obtained it —
//! from `config.toml` directly, or from a fenced block `dark-agentsmd`
//! extracted out of an `AGENTS.md` file. Layering the two is
//! [`AxisSets::layered`]: apply configuration first, then the instruction
//! file's block, so the nearer file wins, matching Rule 22's "nearest file
//! has precedence."

use dark_contract::{ErrCode, Error, Result};
use serde::{Deserialize, Serialize};

/// The type of destination a map charts a way towards.
///
/// Stage 1 (destination) settles this; stage 3 (axis sweep) uses it to pick
/// an axis set (task unit `E2`, Do step 2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DestinationType {
    /// A new capability, built from a written specification.
    Spec,
    /// A choice between named options, with no new code implied.
    Decision,
    /// A change to something that already exists in the repository.
    InPlace,
}

impl DestinationType {
    /// Returns the `[plan.axes.*]` table name this destination type reads
    /// from, for example `"spec"`.
    #[must_use]
    pub fn table_name(self) -> &'static str {
        match self {
            Self::Spec => "spec",
            Self::Decision => "decision",
            Self::InPlace => "in_place",
        }
    }
}

/// One named list of axes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AxisSet {
    /// The axes, in the order the sweep asks about them.
    pub axes: Vec<String>,
}

impl AxisSet {
    fn new(axes: &[&str]) -> Self {
        Self {
            axes: axes.iter().map(|axis| (*axis).to_owned()).collect(),
        }
    }
}

/// The three axis sets a destination type selects between.
///
/// Built from [`AxisSets::builtin`] by default; [`AxisSets::from_toml`] and
/// [`AxisSets::layered`] apply an override.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AxisSets {
    /// The axes for a `spec` destination.
    pub spec: AxisSet,
    /// The axes for a `decision` destination.
    pub decision: AxisSet,
    /// The axes for an `in_place` destination.
    pub in_place: AxisSet,
}

impl AxisSets {
    /// Returns the axis set for `destination_type`.
    #[must_use]
    pub fn for_destination(&self, destination_type: DestinationType) -> &AxisSet {
        match destination_type {
            DestinationType::Spec => &self.spec,
            DestinationType::Decision => &self.decision,
            DestinationType::InPlace => &self.in_place,
        }
    }

    /// Builds the three built-in axis sets, exactly as task unit `E2`, Do
    /// step 1, lists them.
    #[must_use]
    pub fn builtin() -> Self {
        Self {
            spec: AxisSet::new(&[
                "data model and invariants",
                "interfaces and boundaries",
                "failure modes and error handling",
                "lifecycle, migration and backfill",
                "observability",
                "testing strategy",
                "performance envelope",
                "security and permissions",
                "dependencies and versioning",
                "rollout and reversibility",
            ]),
            decision: AxisSet::new(&[
                "options on the table",
                "evaluation criteria",
                "constraints that remove options",
                "cost to reverse",
                "who must agree",
                "what would show we are wrong",
            ]),
            in_place: AxisSet::new(&[
                "current shape",
                "target shape",
                "migration path",
                "blast radius",
                "verification",
                "rollback",
            ]),
        }
    }

    /// Parses a `[plan.axes.*]` TOML document, layered over
    /// [`AxisSets::builtin`]. A table this document omits keeps the
    /// built-in axis list for that destination type.
    ///
    /// # Errors
    ///
    /// Returns [`ErrCode::EngineLoad`] when `text` is not valid TOML, or
    /// when a `[plan.axes.*]` table names an empty `axes` list — an axis
    /// set with no axes can never produce "candidates or an explicit empty
    /// answer" (task unit `E2`'s "Done when"), because there is nothing to
    /// ask about.
    pub fn from_toml(text: &str) -> Result<Self> {
        Self::from_toml_over(text, &Self::builtin())
    }

    /// Parses a `[plan.axes.*]` TOML document, falling back to `base` for
    /// any table the document omits. See [`AxisSets::layered`], which folds
    /// this over a sequence of documents so an earlier layer's override
    /// survives a later layer that names a different table.
    ///
    /// # Errors
    ///
    /// See [`AxisSets::from_toml`].
    fn from_toml_over(text: &str, base: &Self) -> Result<Self> {
        #[derive(Debug, Default, Deserialize)]
        #[serde(default)]
        struct Document {
            plan: PlanSection,
        }

        #[derive(Debug, Default, Deserialize)]
        #[serde(default)]
        struct PlanSection {
            axes: AxesSection,
        }

        #[derive(Debug, Default, Deserialize)]
        #[serde(default)]
        struct AxesSection {
            spec: Option<AxisSet>,
            decision: Option<AxisSet>,
            in_place: Option<AxisSet>,
        }

        let document: Document = toml::from_str(text).map_err(|err| {
            Error::new(
                ErrCode::EngineLoad,
                format!("invalid axis set configuration: {err}"),
            )
            .with_remedy("Check the [plan.axes.spec], [plan.axes.decision], and [plan.axes.in_place] tables.")
        })?;

        let sets = Self {
            spec: document.plan.axes.spec.unwrap_or_else(|| base.spec.clone()),
            decision: document
                .plan
                .axes
                .decision
                .unwrap_or_else(|| base.decision.clone()),
            in_place: document
                .plan
                .axes
                .in_place
                .unwrap_or_else(|| base.in_place.clone()),
        };

        for (name, set) in [
            ("spec", &sets.spec),
            ("decision", &sets.decision),
            ("in_place", &sets.in_place),
        ] {
            if set.axes.is_empty() {
                return Err(Error::new(
                    ErrCode::EngineLoad,
                    format!("[plan.axes.{name}] names no axes"),
                )
                .with_remedy(
                    "Give the table an axes list, or remove it to keep the built-in one.",
                ));
            }
        }

        Ok(sets)
    }

    /// Applies zero or more override documents in order, the nearest one
    /// last, matching Rule 22.
    ///
    /// A typical caller passes `[config_toml, agents_md_block]`: the
    /// repository configuration first, then the instruction file's
    /// `[plan.axes.*]` block, so the instruction file — nearer to the code
    /// under discussion — wins on any axis set it names. A layer that
    /// overrides only one table (say `decision`) leaves an earlier layer's
    /// override of another table (say `spec`) in place, rather than
    /// resetting it to the built-in list.
    ///
    /// # Errors
    ///
    /// Returns [`ErrCode::EngineLoad`] on the first document that fails to
    /// parse. See [`AxisSets::from_toml`].
    pub fn layered<'a>(overrides: impl IntoIterator<Item = &'a str>) -> Result<Self> {
        let mut sets = Self::builtin();
        for text in overrides {
            sets = Self::from_toml_over(text, &sets)?;
        }
        Ok(sets)
    }
}

impl Default for AxisSets {
    fn default() -> Self {
        Self::builtin()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_axis_sets_match_the_build_specification_counts() {
        let sets = AxisSets::builtin();
        assert_eq!(sets.spec.axes.len(), 10);
        assert_eq!(sets.decision.axes.len(), 6);
        assert_eq!(sets.in_place.axes.len(), 6);
        assert_eq!(sets.spec.axes[0], "data model and invariants");
        assert_eq!(sets.decision.axes[0], "options on the table");
        assert_eq!(sets.in_place.axes[0], "current shape");
    }

    #[test]
    fn for_destination_selects_the_matching_set() {
        let sets = AxisSets::builtin();
        assert_eq!(
            sets.for_destination(DestinationType::Decision).axes[0],
            "options on the table"
        );
    }

    #[test]
    fn destination_type_table_names_match_the_toml_table_names() {
        assert_eq!(DestinationType::Spec.table_name(), "spec");
        assert_eq!(DestinationType::Decision.table_name(), "decision");
        assert_eq!(DestinationType::InPlace.table_name(), "in_place");
    }

    #[test]
    fn an_override_replaces_only_the_named_table() {
        let sets = AxisSets::from_toml(
            r#"
            [plan.axes.decision]
            axes = ["one custom axis"]
            "#,
        )
        .expect("valid override");

        assert_eq!(sets.decision.axes, vec!["one custom axis".to_owned()]);
        // The other two tables keep their built-in axes.
        assert_eq!(sets.spec.axes.len(), 10);
        assert_eq!(sets.in_place.axes.len(), 6);
    }

    #[test]
    fn an_empty_axes_list_is_rejected() {
        let err = AxisSets::from_toml(
            r"
            [plan.axes.spec]
            axes = []
            ",
        )
        .expect_err("an empty axis set must be rejected");
        assert_eq!(err.code, ErrCode::EngineLoad);
    }

    #[test]
    fn invalid_toml_is_rejected_with_a_remedy() {
        let err = AxisSets::from_toml("not valid toml =").expect_err("must fail");
        assert_eq!(err.code, ErrCode::EngineLoad);
        assert!(err.remedy.is_some());
    }

    #[test]
    fn layered_applies_the_nearest_override_last() {
        let config = r#"
            [plan.axes.spec]
            axes = ["from config"]
            "#;
        let agents_md_block = r#"
            [plan.axes.spec]
            axes = ["from agents md"]
            "#;
        let sets = AxisSets::layered([config, agents_md_block]).expect("valid layers");
        assert_eq!(sets.spec.axes, vec!["from agents md".to_owned()]);
    }

    #[test]
    fn layered_with_no_overrides_is_the_builtin_table() {
        let sets = AxisSets::layered(std::iter::empty()).expect("valid");
        assert_eq!(sets, AxisSets::builtin());
    }

    #[test]
    fn a_later_layer_naming_a_different_table_does_not_erase_an_earlier_override() {
        let config = r#"
            [plan.axes.spec]
            axes = ["from config"]
            "#;
        let agents_md_block = r#"
            [plan.axes.decision]
            axes = ["from agents md"]
            "#;
        let sets = AxisSets::layered([config, agents_md_block]).expect("valid layers");
        assert_eq!(sets.spec.axes, vec!["from config".to_owned()]);
        assert_eq!(sets.decision.axes, vec!["from agents md".to_owned()]);
    }
}
