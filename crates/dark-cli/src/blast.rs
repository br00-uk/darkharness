//! `dark blast <symbol>`: shows what a change to a symbol can affect
//! (task unit `F3`).
//!
//! The answer is a walk over the S-graph, backwards: everything that
//! references the symbol, then everything that references those, and so
//! on. [`dark_explore::seam::blast_for_symbol`] does that walk twice —
//! once unbounded, and once stopped at any edge whose projected seam
//! score reaches the bounding threshold — and the difference between the
//! two is the useful number. A large unbounded reach with a small bounded
//! one means a seam already limits the change; two similar numbers mean
//! nothing does.
//!
//! # Why this always recomputes
//!
//! `dark explore` writes `.dark/explore/<tree-sha>.json`, and `dark
//! seams` reuses it. This command cannot: that report carries the summary
//! counts and the top seams, not the graph, and a blast radius is a walk
//! over the graph itself.
//!
//! Nothing here opens a network connection: the pipeline reads files on
//! disk and runs `git log` locally.

use anyhow::Result;
use dark_explore::seam::{self, SymbolBlast};

/// How many affected files to list before summarising the rest.
const LISTED_FILES: usize = 20;

/// Runs `dark blast <symbol>`.
///
/// # Errors
///
/// Returns an error when the repository cannot be analysed — see
/// [`crate::explore::analyse_for_blast`] — and when no definition in the
/// repository carries the name asked for.
pub(crate) fn run_command(symbol: &str) -> Result<()> {
    let analysed = crate::explore::analyse_for_blast(None)?;

    let blast = seam::blast_for_symbol(&analysed.graphs, &analysed.analysis.seams, symbol)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "no definition in this repository is called {symbol}. Run dark explore to see \
                 what the analysis found, and check the spelling and the letter case."
            )
        })?;

    print_report(symbol, &blast);
    Ok(())
}

/// Prints what the walk found.
fn print_report(symbol: &str, blast: &SymbolBlast) {
    print!("{}", render_report(symbol, blast));
}

/// Renders the report as text.
///
/// Separate from [`print_report`] so a test can assert on what a person
/// reads. The two counts and the file list have to agree with each other,
/// and that agreement is not visible from the fields alone.
fn render_report(symbol: &str, blast: &SymbolBlast) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();

    let _ = writeln!(
        out,
        "{symbol}: {} definition(s) with this name",
        blast.definitions
    );
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "reachable:    {} definition(s) reference it, directly or through others",
        blast.reachable,
    );
    let _ = writeln!(
        out,
        "bounded:      {} of those are inside the nearest seams",
        blast.bounded,
    );
    let _ = writeln!(
        out,
        "containment:  {:.0}% of the reach is cut away by the seams around it",
        blast.containment() * 100.0,
    );

    if blast.files.is_empty() {
        let _ = writeln!(out);
        // `SymbolBlast::files` names the files a change reaches *other
        // than* the ones the symbol is defined in, so an empty list and a
        // reach of zero are different answers. One sentence for both
        // contradicts the count printed three lines above it.
        let _ = if blast.bounded == 0 {
            writeln!(out, "nothing else in this repository references it.")
        } else {
            writeln!(out, "every reference is inside the file(s) that define it.")
        };
        return out;
    }

    let _ = writeln!(out);
    let _ = writeln!(out, "files a change would reach, inside the seams:");
    for path in blast.files.iter().take(LISTED_FILES) {
        let _ = writeln!(out, "  {}", path.display());
    }
    if blast.files.len() > LISTED_FILES {
        let _ = writeln!(out, "  … and {} more", blast.files.len() - LISTED_FILES);
    }

    if blast.bounding_seams > 0 {
        let _ = writeln!(out);
        let _ = writeln!(
            out,
            "{} seam(s) stop the walk from going further. Run dark seams to see them.",
            blast.bounding_seams,
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    /// A blast result with the counts these tests care about.
    fn blast(reachable: usize, bounded: usize, files: Vec<&str>) -> SymbolBlast {
        SymbolBlast {
            definitions: 1,
            reachable,
            bounded,
            files: files.into_iter().map(PathBuf::from).collect(),
            bounding_seams: 0,
        }
    }

    #[test]
    fn a_symbol_nothing_references_says_so() {
        let report = render_report("Lonely", &blast(0, 0, vec![]));
        assert!(
            report.contains("nothing else in this repository references it."),
            "{report}"
        );
    }

    #[test]
    fn a_symbol_referenced_only_at_home_never_claims_nothing_references_it() {
        // The file list drops the files the symbol is defined in, so a
        // symbol used only where it is defined has a reach and no files.
        // Saying "nothing references it" there contradicts the count.
        let report = render_report("Local", &blast(7, 7, vec![]));
        assert!(
            !report.contains("nothing else in this repository references it."),
            "the report must not contradict its own count of 7:\n{report}"
        );
        assert!(
            report.contains("every reference is inside the file(s) that define it."),
            "{report}"
        );
    }

    #[test]
    fn containment_is_zero_when_nothing_references_the_symbol() {
        assert!((blast(0, 0, Vec::new()).containment() - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn containment_is_zero_when_no_seam_cuts_the_reach() {
        // Everything reachable is also inside the seams, so the seams cut
        // nothing away.
        assert!((blast(10, 10, Vec::new()).containment() - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn containment_reports_the_fraction_the_seams_cut_away() {
        // Two of ten stay inside the seams, so eight tenths are cut away.
        assert!((blast(10, 2, Vec::new()).containment() - 0.8).abs() < 1e-9);
    }

    #[test]
    fn printing_a_report_with_no_affected_files_does_not_panic() {
        // The empty case takes its own branch, so it is worth running.
        print_report("some_symbol", &blast(0, 0, Vec::new()));
    }

    #[test]
    fn printing_a_long_file_list_does_not_panic() {
        let many: Vec<String> = (0..LISTED_FILES + 5)
            .map(|n| format!("src/f{n}.rs"))
            .collect();
        let refs: Vec<&str> = many.iter().map(String::as_str).collect();
        print_report("some_symbol", &blast(30, 25, refs));
    }
}
