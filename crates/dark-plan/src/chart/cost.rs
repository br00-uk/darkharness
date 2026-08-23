//! The cost estimate that task unit `E1`, Do step 9, requires printing
//! before charting starts.
//!
//! `dark-plan` does not print anything itself — it has no terminal to print
//! to (`dark-tui` owns that; `dark-core` wires the two together) — so this
//! module produces the text, and the caller shows it.

use std::fmt;

/// The inputs a cost estimate needs.
///
/// `axis_count` is exact: the axis set is chosen before charting starts.
/// `estimated_candidates` and `estimated_tickets` are not exact — nothing
/// is, before extraction runs — so the caller supplies a guess, typically
/// from a similar past map, or a fixed default when none exists.
#[derive(Debug, Clone, PartialEq)]
pub struct CostInputs {
    /// How many axes the axis sweep (stage 3) will ask about.
    pub axis_count: usize,
    /// A guess at how many candidates extraction (stage 4) will produce.
    /// Sharpening (stage 5) runs once per candidate.
    pub estimated_candidates: usize,
    /// A guess at how many tickets the map will end with, after sizing
    /// (stage 6). Sizing and wiring (stage 7) each run once per ticket.
    pub estimated_tickets: usize,
    /// The average tokens one generation produces, used to convert a
    /// generation count into a time estimate.
    pub avg_tokens_per_generation: usize,
    /// The measured generation rate, from `Caps::measured_tok_s`.
    pub tok_s: f32,
    /// The model identifier, for example `qwen3-14b-q4`.
    pub model_id: String,
}

/// A charting cost estimate, ready to print.
///
/// [`fmt::Display`] renders the exact block the build specification shows
/// in Do step 9 of task unit `E1`.
#[derive(Debug, Clone, PartialEq)]
pub struct CostEstimate {
    /// The destination this estimate is for.
    pub destination: String,
    /// How many axes the sweep covers.
    pub axis_count: usize,
    /// How many generations extraction and sharpening together cost.
    pub extract_and_sharpen_generations: usize,
    /// How many generations sizing and wiring together cost.
    pub size_and_wire_generations: usize,
    /// The estimated wall-clock time, in seconds.
    pub estimated_seconds: f32,
    /// The measured generation rate this estimate assumes.
    pub tok_s: f32,
    /// The model identifier this estimate assumes.
    pub model_id: String,
}

impl CostEstimate {
    /// Computes an estimate from [`CostInputs`].
    ///
    /// - The axis sweep (stage 3) costs one generation per axis.
    /// - Extraction (stage 4) costs one generation; sharpening (stage 5)
    ///   costs one generation per estimated candidate.
    /// - Sizing (stage 6) and wiring (stage 7) each cost one generation per
    ///   estimated ticket — "`~2N` generations," in the build
    ///   specification's own notation.
    #[must_use]
    pub fn estimate(destination: &str, inputs: &CostInputs) -> Self {
        let axis_generations = inputs.axis_count;
        let extract_and_sharpen_generations = 1 + inputs.estimated_candidates;
        let size_and_wire_generations = 2 * inputs.estimated_tickets;

        let total_generations =
            axis_generations + extract_and_sharpen_generations + size_and_wire_generations;
        #[allow(clippy::cast_precision_loss)]
        let total_tokens = (total_generations * inputs.avg_tokens_per_generation) as f32;
        let estimated_seconds = if inputs.tok_s > 0.0 {
            total_tokens / inputs.tok_s
        } else {
            0.0
        };

        Self {
            destination: destination.to_owned(),
            axis_count: inputs.axis_count,
            extract_and_sharpen_generations,
            size_and_wire_generations,
            estimated_seconds,
            tok_s: inputs.tok_s,
            model_id: inputs.model_id.clone(),
        }
    }

    /// Returns the estimated time in whole minutes, rounded up so the
    /// printed estimate never reads as faster than the run can be.
    #[must_use]
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    pub fn estimated_minutes(&self) -> u32 {
        (self.estimated_seconds / 60.0).ceil().max(1.0) as u32
    }
}

impl fmt::Display for CostEstimate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "Charting {:?}", self.destination)?;
        writeln!(
            f,
            "  {} axes, 1 turn each   ~{} generations   deliberate, thinking on",
            self.axis_count, self.axis_count
        )?;
        writeln!(
            f,
            "  extract and sharpen    ~{} generations   grammar-constrained",
            self.extract_and_sharpen_generations
        )?;
        writeln!(
            f,
            "  size and wire          ~{} generations   single token",
            self.size_and_wire_generations
        )?;
        write!(
            f,
            "  estimated              ~{} min at {} tok/s on {}",
            self.estimated_minutes(),
            self.tok_s,
            self.model_id
        )
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::float_cmp)]
    use super::*;

    fn sample_inputs() -> CostInputs {
        CostInputs {
            axis_count: 10,
            estimated_candidates: 13,
            estimated_tickets: 8,
            avg_tokens_per_generation: 120,
            tok_s: 41.2,
            model_id: "qwen3-14b-q4".to_owned(),
        }
    }

    #[test]
    fn generation_counts_match_the_do_step_9_formula() {
        let estimate = CostEstimate::estimate("offline pack format", &sample_inputs());
        assert_eq!(estimate.axis_count, 10);
        assert_eq!(estimate.extract_and_sharpen_generations, 14); // 1 + 13
        assert_eq!(estimate.size_and_wire_generations, 16); // 2 * 8
    }

    #[test]
    fn a_zero_token_rate_estimates_zero_seconds_rather_than_dividing_by_zero() {
        let mut inputs = sample_inputs();
        inputs.tok_s = 0.0;
        let estimate = CostEstimate::estimate("x", &inputs);
        assert_eq!(estimate.estimated_seconds, 0.0);
        assert_eq!(estimate.estimated_minutes(), 1); // rounds up, never zero
    }

    #[test]
    fn display_matches_the_build_specification_shape() {
        let estimate = CostEstimate::estimate("offline pack format", &sample_inputs());
        let text = estimate.to_string();
        assert!(text.starts_with("Charting \"offline pack format\"\n"));
        assert!(text.contains("10 axes, 1 turn each   ~10 generations   deliberate, thinking on"));
        assert!(text.contains("extract and sharpen    ~14 generations   grammar-constrained"));
        assert!(text.contains("size and wire          ~16 generations   single token"));
        assert!(text.contains("estimated              ~"));
        assert!(text.contains("tok/s on qwen3-14b-q4"));
    }
}
