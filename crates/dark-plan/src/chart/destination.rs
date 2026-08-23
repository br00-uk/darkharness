//! Stage 1 of the charting pipeline: settle the destination.
//!
//! Task unit `E1`, Do step 1 (stage 1) and Do step 3: "Settle the
//! destination first. The destination fixes the scope." Context in: "The
//! idea, AGENTS.md, repository summary." Output: `{destination, notes,
//! type}`. Micro-role: `deliberate`.

use dark_contract::{Engine, Message, Result, Role, RoleClass};
use serde::{Deserialize, Serialize};

use crate::axes::DestinationType;
use crate::chart::sampling::{MicroSampling, build_request, run_generation};
use dark_contract::{ErrCode, Error};

/// What stage 1 produced.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DestinationRecord {
    /// What the map is charting a way towards.
    pub destination: String,
    /// Free-text notes about the destination, when the model gave any.
    pub notes: Option<String>,
    /// The destination's type. Selects the axis set stage 3 sweeps.
    pub destination_type: DestinationType,
}

/// Builds the stage 1 prompt.
///
/// `deliberate` runs with no grammar constraint (`MicroSampling::deliberate`
/// sets `grammar: false`), so this asks for a small tagged-line format
/// instead of JSON, and [`parse_destination`] parses it leniently. A
/// caller who resolves a profile with `Caps::grammar` and
/// `Profile::force_grammar` both set may still choose to layer a JSON
/// schema onto the returned [`dark_contract::Request`] before sending it;
/// this function's prompt does not assume that happened.
fn destination_prompt(idea: &str, agents_md: &str, repo_summary: &str) -> Vec<Message> {
    let context = format!(
        "The idea:\n{idea}\n\nAGENTS.md:\n{agents_md}\n\nRepository summary:\n{repo_summary}\n\n\
         Settle the destination this map charts a way towards. Answer in exactly this shape, \
         one field per line:\n\nDESTINATION: <what the map charts a way towards, one paragraph>\n\
         NOTES: <anything else worth carrying forward, or leave blank>\n\
         TYPE: <one of spec, decision, in_place>\n\n\
         spec means a new capability built from a written specification. decision means a \
         choice between named options, with no new code implied. in_place means a change to \
         something that already exists in the repository."
    );

    vec![
        Message::text(
            Role::System,
            "You settle the destination for a decision map. This is the only turn you get: \
             there is no earlier conversation to draw on, and no later turn to fix a vague \
             answer. Read only the idea, AGENTS.md, and the repository summary you were given.",
        ),
        Message::text(Role::User, context),
    ]
}

/// Reads the text after a `TAG:` prefix on this line, when the line starts
/// with one of `tags` (case-insensitive).
fn tag_prefix<'a>(line: &'a str, tags: &[&'a str]) -> Option<(&'a str, &'a str)> {
    let trimmed = line.trim_start();
    for tag in tags {
        let prefix = format!("{tag}:");
        if trimmed.len() >= prefix.len() && trimmed[..prefix.len()].eq_ignore_ascii_case(&prefix) {
            return Some((tag, trimmed[prefix.len()..].trim()));
        }
    }
    None
}

/// Parses a stage 1 response in the shape [`destination_prompt`] asks for.
///
/// # Errors
///
/// Returns [`ErrCode::EngineGenerate`] when the response names no
/// `DESTINATION` field, or when `TYPE` is not `spec`, `decision`, or
/// `in_place`.
fn parse_destination(text: &str) -> Result<DestinationRecord> {
    const TAGS: [&str; 3] = ["DESTINATION", "NOTES", "TYPE"];

    let mut destination = String::new();
    let mut notes = String::new();
    let mut destination_type_text = String::new();
    let mut current: Option<&str> = None;

    for line in text.lines() {
        if let Some((tag, rest)) = tag_prefix(line, &TAGS) {
            current = Some(tag);
            match tag {
                "DESTINATION" => rest.clone_into(&mut destination),
                "NOTES" => rest.clone_into(&mut notes),
                "TYPE" => rest.clone_into(&mut destination_type_text),
                _ => unreachable!("tag_prefix only returns a tag from TAGS"),
            }
            continue;
        }

        // A continuation line for a multi-line field.
        match current {
            Some("DESTINATION") if !line.trim().is_empty() => {
                destination.push(' ');
                destination.push_str(line.trim());
            }
            Some("NOTES") if !line.trim().is_empty() => {
                notes.push(' ');
                notes.push_str(line.trim());
            }
            _ => {}
        }
    }

    if destination.trim().is_empty() {
        return Err(Error::new(
            ErrCode::EngineGenerate,
            "stage 1 produced no DESTINATION field",
        )
        .with_remedy("Retry stage 1. See dark map chart --resume --from-stage 1."));
    }

    let destination_type = match destination_type_text.trim().to_lowercase().as_str() {
        "spec" => DestinationType::Spec,
        "decision" => DestinationType::Decision,
        "in_place" | "in-place" => DestinationType::InPlace,
        other => {
            return Err(Error::new(
                ErrCode::EngineGenerate,
                format!("stage 1 produced an unrecognised TYPE {other:?}"),
            )
            .with_remedy(
                "Retry stage 1. TYPE must be spec, decision, or in_place. See dark map chart \
                 --resume --from-stage 1.",
            ));
        }
    };

    Ok(DestinationRecord {
        destination: destination.trim().to_owned(),
        notes: (!notes.trim().is_empty()).then(|| notes.trim().to_owned()),
        destination_type,
    })
}

/// Runs stage 1: settles the destination from the idea, `AGENTS.md`, and a
/// repository summary.
///
/// # Errors
///
/// Returns an error when the engine fails, or when
/// [`parse_destination`] cannot make sense of the response.
pub async fn run_destination(
    engine: &dyn Engine,
    class: RoleClass,
    sampling: MicroSampling,
    idea: &str,
    agents_md: &str,
    repo_summary: &str,
) -> Result<DestinationRecord> {
    let messages = destination_prompt(idea, agents_md, repo_summary);
    let request = build_request(class, messages, sampling);
    let generation = run_generation(engine, request).await?;
    parse_destination(&generation.text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use dark_engine_fake::script::Turn;
    use dark_engine_fake::{FakeEngine, Script};

    #[test]
    fn parse_destination_reads_all_three_fields() {
        let record = parse_destination(
            "DESTINATION: A frozen, versioned pack format.\n\
             NOTES: Domain: Rust, content-addressed storage.\n\
             TYPE: spec\n",
        )
        .expect("valid response");

        assert_eq!(record.destination, "A frozen, versioned pack format.");
        assert_eq!(
            record.notes.as_deref(),
            Some("Domain: Rust, content-addressed storage.")
        );
        assert_eq!(record.destination_type, DestinationType::Spec);
    }

    #[test]
    fn parse_destination_accepts_a_blank_notes_field() {
        let record = parse_destination("DESTINATION: Something.\nNOTES:\nTYPE: decision\n")
            .expect("valid response");
        assert_eq!(record.notes, None);
    }

    #[test]
    fn parse_destination_is_case_insensitive_on_tags_and_type() {
        let record =
            parse_destination("destination: Something.\ntype: IN_PLACE\n").expect("valid response");
        assert_eq!(record.destination_type, DestinationType::InPlace);
    }

    #[test]
    fn a_missing_destination_field_is_rejected() {
        let err = parse_destination("NOTES: only notes\nTYPE: spec\n")
            .expect_err("a response with no destination must fail");
        assert_eq!(err.code, ErrCode::EngineGenerate);
    }

    #[test]
    fn an_unrecognised_type_is_rejected() {
        let err = parse_destination("DESTINATION: x\nTYPE: whatever\n")
            .expect_err("an unknown type must fail");
        assert_eq!(err.code, ErrCode::EngineGenerate);
        assert!(err.message.contains("whatever"));
    }

    #[tokio::test]
    async fn run_destination_drives_one_generation() {
        let engine = FakeEngine::new(Script {
            turns: vec![Turn {
                text: "DESTINATION: A retry policy for the pack fetcher.\nTYPE: decision\n"
                    .to_owned(),
                ..Default::default()
            }],
            ..Default::default()
        });

        let record = run_destination(
            &engine,
            RoleClass::Architect,
            MicroSampling::deliberate(),
            "we need a retry policy",
            "",
            "",
        )
        .await
        .expect("stage 1 succeeds");

        assert_eq!(record.destination, "A retry policy for the pack fetcher.");
        assert_eq!(record.destination_type, DestinationType::Decision);
        assert_eq!(engine.seen_requests().len(), 1);
    }
}
