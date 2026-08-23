//! The stage 2 (Seed) input: what the repository already tells the pipeline
//! before any model runs.
//!
//! Stage 2 of task unit `E1` uses no model (see the stage table in Do step
//! 1). It reads the repository and reports seams, blast radius, and the
//! module list. Computing those numbers is `dark-explore`'s job (task unit
//! `F3`, `crates/dark-explore/src/seam/`), and `dark-plan` does not depend
//! on `dark-explore` — nothing in this crate's `Cargo.toml` names it, and
//! adding it would widen the dependency surface for no charting-side
//! reason. So [`SeedReport`] is the plain data contract between the two: a
//! caller who has run `/explore` builds one of these from
//! `.dark/explore/<tree-sha>.json` and hands it to the charting pipeline.
//! Charting stage 3 (axis sweep, `E2`) reads the numbers out of it; stage 2
//! itself is exactly this hand-off, nothing more.

use serde::{Deserialize, Serialize};

/// One module the seed report names.
///
/// Mirrors one entry of the `modules` array in `.dark/explore/<tree-sha>.json`
/// (see task unit `F4`, Do step 1), trimmed to the fields the axis sweep
/// seeds an answer with: a module's path, and how coupled it is to the rest
/// of the repository.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SeedModule {
    /// The module's path, relative to the repository root.
    pub path: String,
    /// How many other modules depend on this one (afferent coupling).
    pub incoming: u32,
    /// How many modules this one depends on (efferent coupling).
    pub outgoing: u32,
}

/// One seam the seed report names.
///
/// Mirrors one entry of the `seams` array in
/// `.dark/explore/<tree-sha>.json`, trimmed to what an axis answer quotes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SeedSeam {
    /// The edge's source.
    pub from: String,
    /// The edge's target.
    pub to: String,
    /// The seam score, in the range 0 to 1. Higher means a better boundary.
    pub score: f32,
    /// `true` when the edge is a bridge: cutting it disconnects the graph.
    pub hard: bool,
}

/// The computed blast radius for the change under discussion.
///
/// Mirrors task unit `F3`, Do step 9: `R` is the full reverse-reachable
/// set, `r_bounded` is the same traversal stopped at a strong seam. A large
/// `r` with a small `r_bounded` means a seam already limits the change —
/// the axis sweep's seed for "blast radius" and "current shape" (`E2`, Do
/// step 4) is exactly this pair of numbers, not a guess.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlastRadius {
    /// The full reverse-reachable set size.
    pub r: u32,
    /// The reverse-reachable set size, stopped at a strong seam.
    pub r_bounded: u32,
}

/// What stage 2 (Seed) hands the rest of the charting pipeline.
///
/// See the module documentation for why `dark-plan` takes this as plain
/// data instead of computing it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct SeedReport {
    /// The seams `dark-explore` ranked highest, most useful boundary first.
    #[serde(default)]
    pub seams: Vec<SeedSeam>,
    /// The blast radius for the change under discussion, when the caller
    /// has computed one. Charting a brand new destination with no known
    /// symbol set yet leaves this `None`.
    #[serde(default)]
    pub blast_radius: Option<BlastRadius>,
    /// The modules the repository is built from.
    #[serde(default)]
    pub modules: Vec<SeedModule>,
}

impl SeedReport {
    /// Renders the numbers an axis answer should be seeded with, as short
    /// text a prompt can quote directly.
    ///
    /// Returns `None` when the report carries nothing to seed — an empty
    /// report, for example a destination with no repository behind it yet.
    #[must_use]
    pub fn seed_text(&self) -> Option<String> {
        if self.seams.is_empty() && self.blast_radius.is_none() && self.modules.is_empty() {
            return None;
        }

        let mut lines = Vec::new();
        if let Some(radius) = self.blast_radius {
            lines.push(format!(
                "blast radius: {} files reachable, {} within a bounding seam",
                radius.r, radius.r_bounded
            ));
        }
        if !self.seams.is_empty() {
            let top: Vec<String> = self
                .seams
                .iter()
                .take(5)
                .map(|seam| {
                    format!(
                        "{} -> {} (score {:.2}{})",
                        seam.from,
                        seam.to,
                        seam.score,
                        if seam.hard { ", bridge" } else { "" }
                    )
                })
                .collect();
            lines.push(format!("seams: {}", top.join("; ")));
        }
        if !self.modules.is_empty() {
            let names: Vec<&str> = self.modules.iter().map(|m| m.path.as_str()).collect();
            lines.push(format!("modules: {}", names.join(", ")));
        }
        Some(lines.join("\n"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_report_seeds_nothing() {
        assert_eq!(SeedReport::default().seed_text(), None);
    }

    #[test]
    fn a_report_with_a_blast_radius_seeds_the_numbers_not_a_guess() {
        let report = SeedReport {
            blast_radius: Some(BlastRadius {
                r: 40,
                r_bounded: 6,
            }),
            ..SeedReport::default()
        };
        let text = report.seed_text().expect("seeds something");
        assert!(text.contains("40 files reachable"));
        assert!(text.contains("6 within a bounding seam"));
    }

    #[test]
    fn a_report_round_trips_through_json() {
        let report = SeedReport {
            seams: vec![SeedSeam {
                from: "dark-core::turn".to_owned(),
                to: "dark-contract".to_owned(),
                score: 0.81,
                hard: false,
            }],
            blast_radius: Some(BlastRadius {
                r: 12,
                r_bounded: 3,
            }),
            modules: vec![SeedModule {
                path: "crates/dark-lexicon".to_owned(),
                incoming: 6,
                outgoing: 11,
            }],
        };
        let json = serde_json::to_string(&report).unwrap();
        let back: SeedReport = serde_json::from_str(&json).unwrap();
        assert_eq!(back, report);
    }
}
