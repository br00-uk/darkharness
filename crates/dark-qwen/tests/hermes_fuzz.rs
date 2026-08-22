//! Fuzzes the Hermes-style `<tool_call>` parser with malformed input.
//!
//! Generates 200 samples deterministically from a fixed-seed linear
//! congruential generator, so a failure reproduces exactly. See task unit
//! `I3`: 200 malformed samples must produce no panic, and the samples that
//! are recoverable must extract correctly.

use dark_contract::ToolSchema;
use dark_qwen::toolcall::interpret_stream;

/// A tiny deterministic pseudo-random generator.
///
/// This is not for anything security-sensitive. It exists only so the
/// fuzz corpus is the same on every run, on every machine, which Rule 29 to
/// Rule 32 (determinism discipline) call for even outside `/explore`: a
/// fuzz failure has to reproduce.
struct Lcg(u64);

impl Lcg {
    fn new(seed: u64) -> Self {
        Self(seed)
    }

    fn next_u64(&mut self) -> u64 {
        // The constants are the ones glibc's `rand` derivative uses.
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0
    }

    fn next_range(&mut self, bound: usize) -> usize {
        let bound_u64 = u64::try_from(bound).unwrap_or(u64::MAX).max(1);
        let value = self.next_u64() % bound_u64;
        usize::try_from(value).unwrap_or(usize::MAX)
    }

    fn choice<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.next_range(items.len())]
    }
}

fn schemas() -> Vec<ToolSchema> {
    vec![
        ToolSchema {
            name: "read_file".to_owned(),
            description: "Reads a file.".to_owned(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string"},
                    "limit": {"type": "integer"},
                    "verbose": {"type": "boolean"}
                },
                "required": ["path"]
            }),
            tier: 1,
            mutating: false,
        },
        ToolSchema {
            name: "grep".to_owned(),
            description: "Searches a file for a pattern.".to_owned(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "pattern": {"type": "string"}
                },
                "required": ["pattern"]
            }),
            tier: 1,
            mutating: false,
        },
    ]
}

/// Whether a generated sample is expected to extract into at least one
/// successful call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expect {
    /// The sample must extract into exactly one successful call.
    OneCall,
    /// The sample must extract into exactly two successful calls.
    TwoCalls,
    /// The sample must extract into a call, but the harness must refuse it
    /// with a named reason rather than run it.
    Refused,
    /// No claim: only that the parser must not panic.
    Unspecified,
}

struct Sample {
    text: String,
    expect: Expect,
}

/// Builds one malformed or borderline sample, chosen deterministically from
/// `category` and randomised with `rng`.
fn generate_sample(rng: &mut Lcg, category: usize) -> Sample {
    match category % 12 {
        // A plain, well-formed call.
        0 => Sample {
            text: r#"<tool_call>{"name": "read_file", "arguments": {"path": "a.rs"}}</tool_call>"#
                .to_owned(),
            expect: Expect::OneCall,
        },
        // Wrapped in a Markdown code fence.
        1 => {
            let lang = *rng.choice(&["json", ""]);
            Sample {
                text: format!(
                    "```{lang}\n<tool_call>{{\"name\": \"read_file\", \"arguments\": {{\"path\": \"a.rs\"}}}}</tool_call>\n```"
                ),
                expect: Expect::OneCall,
            }
        }
        // Double-encoded: the whole object is escaped inside a JSON string.
        2 => Sample {
            text: r#"<tool_call>"{\"name\": \"read_file\", \"arguments\": {\"path\": \"a.rs\"}}"</tool_call>"#
                .to_owned(),
            expect: Expect::OneCall,
        },
        // A stringly-typed integer that should coerce.
        3 => {
            let n = rng.next_range(9999);
            Sample {
                text: format!(
                    r#"<tool_call>{{"name": "read_file", "arguments": {{"path": "a.rs", "limit": "{n}"}}}}</tool_call>"#
                ),
                expect: Expect::OneCall,
            }
        }
        // A stringly-typed boolean that should coerce.
        4 => {
            let b = *rng.choice(&["true", "false"]);
            Sample {
                text: format!(
                    r#"<tool_call>{{"name": "read_file", "arguments": {{"path": "a.rs", "verbose": "{b}"}}}}</tool_call>"#
                ),
                expect: Expect::OneCall,
            }
        }
        // A required field is missing. Must refuse, never invent it.
        5 => Sample {
            text: r#"<tool_call>{"name": "read_file", "arguments": {"limit": 10}}</tool_call>"#
                .to_owned(),
            expect: Expect::Refused,
        },
        // The stream ends mid-object: unclosed tag at end of stream.
        6 => {
            let cut = rng.next_range(30) + 5;
            let full = r#"<tool_call>{"name": "read_file", "arguments": {"path": "a-very-long-path.rs"}}</tool_call>"#;
            let boundary = (0..=full.len())
                .rev()
                .find(|&i| full.is_char_boundary(i) && i <= full.len().saturating_sub(cut))
                .unwrap_or(0);
            Sample {
                text: full[..boundary].to_owned(),
                expect: Expect::Unspecified,
            }
        }
        // Random noise bytes mixed into an otherwise plausible call.
        7 => {
            let noise: String = (0..rng.next_range(6))
                .map(|_| {
                    let offset = u32::try_from(rng.next_range(90)).unwrap_or(0);
                    char::from_u32(0x20 + offset).unwrap_or('?')
                })
                .collect();
            Sample {
                text: format!(
                    r#"<tool_call>{{"name": "read_file"{noise}, "arguments": {{"path": "a.rs"}}}}</tool_call>"#
                ),
                expect: Expect::Unspecified,
            }
        }
        // Prose on both sides of the block.
        8 => Sample {
            text: r#"Let me check that file for you. <tool_call>{"name": "read_file", "arguments": {"path": "a.rs"}}</tool_call> There it is."#
                .to_owned(),
            expect: Expect::OneCall,
        },
        // Two calls back to back in one message.
        9 => Sample {
            text: r#"<tool_call>{"name": "read_file", "arguments": {"path": "a.rs"}}</tool_call><tool_call>{"name": "grep", "arguments": {"pattern": "fn main"}}</tool_call>"#
                .to_owned(),
            expect: Expect::TwoCalls,
        },
        // A nested brace inside a string argument.
        10 => Sample {
            text: r#"<tool_call>{"name": "grep", "arguments": {"pattern": "fn f() { return {1}; }"}}</tool_call>"#
                .to_owned(),
            expect: Expect::OneCall,
        },
        // An unknown tool name.
        _ => Sample {
            text: r#"<tool_call>{"name": "delete_universe", "arguments": {}}</tool_call>"#
                .to_owned(),
            expect: Expect::Refused,
        },
    }
}

#[test]
fn two_hundred_malformed_samples_never_panic_and_recover_when_they_can() {
    let schemas = schemas();
    let mut rng = Lcg::new(0xD00D_FEED_CAFE_0001);

    for i in 0..200 {
        let category = rng.next_range(12);
        let sample = generate_sample(&mut rng, category);

        // The whole point of this test: this call must never panic, no
        // matter how the sample is malformed.
        let (_, outcomes) = interpret_stream(&sample.text, &schemas);

        match sample.expect {
            Expect::OneCall => {
                assert_eq!(
                    outcomes.len(),
                    1,
                    "sample {i} (category {category}) {:?}: expected one outcome",
                    sample.text
                );
                assert!(
                    outcomes[0].call().is_some(),
                    "sample {i} (category {category}) {:?}: expected a recovered call, got {:?}",
                    sample.text,
                    outcomes[0]
                        .failure_reply()
                        .map(dark_contract::Message::text_content)
                );
            }
            Expect::TwoCalls => {
                assert_eq!(
                    outcomes.len(),
                    2,
                    "sample {i} (category {category}) {:?}: expected two outcomes",
                    sample.text
                );
                assert!(outcomes.iter().all(|o| o.call().is_some()));
            }
            Expect::Refused => {
                assert_eq!(outcomes.len(), 1);
                assert!(
                    outcomes[0].call().is_none(),
                    "sample {i} (category {category}) {:?}: expected a refusal",
                    sample.text
                );
                let reply = outcomes[0]
                    .failure_reply()
                    .expect("a refused call still answers with a Role::Tool reply");
                assert_eq!(reply.role, dark_contract::Role::Tool);
                assert!(!reply.text_content().is_empty());
            }
            Expect::Unspecified => {
                // No panic is the whole assertion. A malformed-beyond-repair
                // sample may or may not recover; if it does, the recovered
                // call must still be self-consistent.
                if let Some(call) = outcomes
                    .first()
                    .and_then(dark_qwen::toolcall::Interpreted::call)
                {
                    assert!(!call.name.is_empty());
                }
            }
        }
    }
}

#[test]
fn the_generator_itself_is_deterministic() {
    let mut a = Lcg::new(42);
    let mut b = Lcg::new(42);
    for _ in 0..50 {
        assert_eq!(a.next_u64(), b.next_u64());
    }
}
