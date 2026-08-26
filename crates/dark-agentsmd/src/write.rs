//! Writing a generated section into an instruction file without
//! destroying what a person wrote around it.
//!
//! # The rule
//!
//! `AGENTS.md` is a file people write by hand. `dark extend` and `dark
//! refactor` have things to add to it — the language, the house style, the
//! module summary — and regenerate them whenever the repository moves.
//! Those two facts are in tension, and the resolution is not negotiable:
//! **a generated section never touches a line it did not write.**
//!
//! So the generated text lives between two markers, and this module
//! replaces what is between them and nothing else. A file that already
//! exists and carries no markers is left exactly as it is: appending to it
//! would put machine text under a person's heading, and rewriting it would
//! lose their work. Refusing and saying so is the only honest third
//! option.
//!
//! # Why not a separate file
//!
//! A second file resolves into the chain (see [`crate::resolve`]) and
//! would avoid the question. It also splits what an agent must read across
//! two places and leaves the person's own file silent about the fact that
//! anything was generated. One file, one marked region, visible in the
//! diff.

use std::path::Path;

use dark_contract::{ErrCode, Error, Result};

/// Opens the generated region.
pub const BEGIN: &str = "<!-- dark:begin discovery -->";

/// Closes the generated region.
pub const END: &str = "<!-- dark:end discovery -->";

/// What [`upsert`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wrote {
    /// The file did not exist, and now holds the section alone.
    Created,
    /// The file existed with markers, and the region between them changed.
    Replaced,
    /// The file existed with markers, and the region already said this.
    Unchanged,
}

/// Builds the error a refusal reports.
fn refused(path: &Path) -> Error {
    Error::new(
        ErrCode::ToolFailed,
        format!(
            "{} already exists and has no generated section to replace",
            path.display()
        ),
    )
    .with_remedy(format!(
        "Add these two lines where the generated notes should go, then run this again:\n\
         {BEGIN}\n{END}"
    ))
}

/// Builds the error an input containing a marker reports.
fn marker_in_body() -> Error {
    Error::new(
        ErrCode::ToolFailed,
        "the generated section contains a section marker".to_owned(),
    )
    .with_remedy("This is a bug in whatever produced the text; report it.".to_owned())
}

/// Renders `body` wrapped in its markers, with a trailing newline.
#[must_use]
pub fn section(body: &str) -> String {
    format!("{BEGIN}\n{}\n{END}\n", body.trim_end())
}

/// Writes `body` into the generated region of the file at `path`.
///
/// Creates the file when it does not exist. Replaces the region when the
/// file exists and carries both markers. **Refuses** when the file exists
/// and does not, because there is no way to add to it that cannot lose
/// something a person wrote.
///
/// # Errors
///
/// Returns [`ErrCode::ToolFailed`] when the file exists without markers,
/// when `body` itself contains a marker, when the markers are out of
/// order, and when the file cannot be read or written.
pub fn upsert(path: &Path, body: &str) -> Result<Wrote> {
    if body.contains(BEGIN) || body.contains(END) {
        return Err(marker_in_body());
    }

    let Some(existing) = read_if_present(path)? else {
        write_atomically(path, &section(body))?;
        return Ok(Wrote::Created);
    };

    let replaced = replace_region(&existing, body).ok_or_else(|| refused(path))?;
    if replaced == existing {
        return Ok(Wrote::Unchanged);
    }
    write_atomically(path, &replaced)?;
    Ok(Wrote::Replaced)
}

/// Returns the file's text, or `None` when it does not exist.
fn read_if_present(path: &Path) -> Result<Option<String>> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(Error::new(
            ErrCode::ToolFailed,
            format!("cannot read {}: {err}", path.display()),
        )),
    }
}

/// Replaces the region between the markers in `existing`.
///
/// Returns `None` when either marker is missing, or when the end marker
/// comes first — a file whose markers are out of order has been edited by
/// hand into a shape this cannot reason about, and guessing which half is
/// the generated one would be exactly the mistake the module exists to
/// prevent.
#[must_use]
pub fn replace_region(existing: &str, body: &str) -> Option<String> {
    let begin = existing.find(BEGIN)?;
    let end = existing.find(END)?;
    if end < begin {
        return None;
    }
    let after = end + END.len();
    Some(format!(
        "{}{}{}",
        &existing[..begin],
        section(body).trim_end(),
        &existing[after..]
    ))
}

/// Writes `text` to `path` by writing a neighbouring temporary file and
/// renaming it over the target.
///
/// A half-written `AGENTS.md` is read by the next session as the
/// instruction chain, so the file must never be observed partly written.
/// The rename is atomic on every platform this runs on when the temporary
/// file is in the same directory, which is why it is not written to a
/// system temporary directory.
fn write_atomically(path: &Path, text: &str) -> Result<()> {
    let failed = |err: std::io::Error| {
        Error::new(
            ErrCode::ToolFailed,
            format!("cannot write {}: {err}", path.display()),
        )
        .with_remedy("Check the permissions on the repository root.")
    };

    if let Some(dir) = path.parent()
        && !dir.as_os_str().is_empty()
    {
        std::fs::create_dir_all(dir).map_err(failed)?;
    }

    let temporary = path.with_extension("dark-tmp");
    std::fs::write(&temporary, text).map_err(failed)?;
    std::fs::rename(&temporary, path).map_err(|err| {
        // The temporary file is this function's own litter; leaving it
        // behind after a failed rename would confuse the next run.
        let _ = std::fs::remove_file(&temporary);
        failed(err)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn file(dir: &TempDir) -> std::path::PathBuf {
        dir.path().join("AGENTS.md")
    }

    #[test]
    fn a_missing_file_is_created_with_the_section() {
        let dir = TempDir::new().unwrap();
        let path = file(&dir);
        assert_eq!(upsert(&path, "Rust 2024.").unwrap(), Wrote::Created);

        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.starts_with(BEGIN), "{text}");
        assert!(text.contains("Rust 2024."), "{text}");
        assert!(text.trim_end().ends_with(END), "{text}");
    }

    #[test]
    fn hand_written_prose_around_the_markers_survives_a_regeneration() {
        // The whole point of the module. A person's own instructions must
        // read identically before and after.
        let dir = TempDir::new().unwrap();
        let path = file(&dir);
        std::fs::write(
            &path,
            format!(
                "# Our rules\n\nAlways run the linter.\n\n{BEGIN}\nold facts\n{END}\n\n\
                 ## Review\n\nTwo approvals.\n"
            ),
        )
        .unwrap();

        assert_eq!(upsert(&path, "new facts").unwrap(), Wrote::Replaced);

        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("# Our rules"), "{text}");
        assert!(text.contains("Always run the linter."), "{text}");
        assert!(text.contains("## Review"), "{text}");
        assert!(text.contains("Two approvals."), "{text}");
        assert!(text.contains("new facts"), "{text}");
        assert!(!text.contains("old facts"), "{text}");
    }

    #[test]
    fn a_file_with_no_markers_is_never_modified() {
        // Appending would put machine text under a person's heading, and
        // rewriting would lose their work. Refusing is the only option
        // that keeps the file theirs.
        let dir = TempDir::new().unwrap();
        let path = file(&dir);
        let original = "# Our rules\n\nAlways run the linter.\n";
        std::fs::write(&path, original).unwrap();

        let err = upsert(&path, "facts").unwrap_err();
        assert_eq!(err.code, ErrCode::ToolFailed);
        assert!(
            err.remedy.as_ref().is_some_and(|r| r.contains(BEGIN)),
            "the remedy must show the markers to add: {:?}",
            err.remedy
        );
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            original,
            "the file must be byte-identical after a refusal"
        );
    }

    #[test]
    fn writing_the_same_body_twice_reports_no_change() {
        let dir = TempDir::new().unwrap();
        let path = file(&dir);
        assert_eq!(upsert(&path, "facts").unwrap(), Wrote::Created);
        assert_eq!(upsert(&path, "facts").unwrap(), Wrote::Unchanged);
    }

    #[test]
    fn regenerating_twice_leaves_the_same_bytes() {
        let dir = TempDir::new().unwrap();
        let path = file(&dir);
        upsert(&path, "facts").unwrap();
        let once = std::fs::read_to_string(&path).unwrap();
        upsert(&path, "facts").unwrap();
        assert_eq!(once, std::fs::read_to_string(&path).unwrap());
    }

    #[test]
    fn markers_out_of_order_are_refused_rather_than_guessed_at() {
        let dir = TempDir::new().unwrap();
        let path = file(&dir);
        let original = format!("{END}\nsomething\n{BEGIN}\n");
        std::fs::write(&path, &original).unwrap();

        assert!(upsert(&path, "facts").is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
    }

    #[test]
    fn a_body_carrying_a_marker_is_refused() {
        // Otherwise the next regeneration would find three markers and
        // replace the wrong region.
        let dir = TempDir::new().unwrap();
        let path = file(&dir);
        let err = upsert(&path, &format!("facts\n{END}\nsmuggled")).unwrap_err();
        assert!(err.message.contains("marker"), "{}", err.message);
        assert!(!path.exists(), "nothing must be written");
    }

    #[test]
    fn no_temporary_file_is_left_behind() {
        let dir = TempDir::new().unwrap();
        let path = file(&dir);
        upsert(&path, "facts").unwrap();
        let leftovers: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.contains("dark-tmp"))
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");
    }

    #[test]
    fn the_section_ends_with_exactly_one_newline_however_the_body_ends() {
        for body in ["facts", "facts\n", "facts\n\n\n"] {
            let rendered = section(body);
            assert!(rendered.ends_with(&format!("{END}\n")), "{rendered:?}");
            assert!(!rendered.ends_with(&format!("{END}\n\n")), "{rendered:?}");
        }
    }
}
