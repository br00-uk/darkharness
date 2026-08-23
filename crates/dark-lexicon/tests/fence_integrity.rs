//! `G3`'s own verify command: `cargo nextest run -p dark-lexicon --test
//! fence_integrity`.
//!
//! "Done when": no chunk contains an unbalanced code fence. This calls
//! `heading-v1` (through `chunk::chunk_with_counter`, the public,
//! `Engine`-free entry point the chunk module's docs explain — see
//! `crate::chunk`'s module docs for why `tests/` cannot construct a
//! `dyn Engine`) over documents built specifically to stress fence
//! handling: a code block that fits inside one chunk, one that alone
//! exceeds the maximum and must become an oversize chunk on its own, code
//! blocks back to back, a code block that contains a line that itself
//! looks like a fence of the other character, and a heading-like `#` line
//! living inside a fence. Every chunk `heading-v1` returns, from every one
//! of these documents, must pass
//! `dark_lexicon::chunk::markdown::has_balanced_fences`.

use dark_contract::Result;
use dark_lexicon::chunk::markdown::has_balanced_fences;
use dark_lexicon::chunk::{self, Chunk, MAX_TOKENS, TokenCounter};
use dark_lexicon::ingest::Document;

/// One token per whitespace-separated word — see `pack_roundtrip.rs` for
/// why a fixture counter this simple is enough to prove a structural
/// property like fence balance, which does not depend on what a "token"
/// is.
struct WordCounter;
impl TokenCounter for WordCounter {
    fn count(&self, text: &str) -> Result<usize> {
        Ok(text.split_whitespace().count())
    }
}

fn assert_every_chunk_has_balanced_fences(chunks: &[Chunk], case: &str) {
    assert!(!chunks.is_empty(), "{case}: produced no chunks at all");
    for chunk in chunks {
        assert!(
            has_balanced_fences(&chunk.body),
            "{case}: chunk {} (breadcrumb '{}') has an unbalanced fence:\n{}",
            chunk.ordinal,
            chunk.breadcrumb,
            chunk.body
        );
        assert!(
            has_balanced_fences(&chunk.embed_text),
            "{case}: chunk {}'s embed_text has an unbalanced fence",
            chunk.ordinal
        );
    }
}

fn run(title: &str, body: &str) -> Vec<Chunk> {
    let doc = Document::new("doc.md", title, body);
    chunk::chunk_with_counter(&WordCounter, "fixture@1.0.0", &doc).expect("chunking must not fail")
}

#[test]
fn a_small_fenced_code_block_stays_intact_in_its_chunk() {
    let body = "# Section\nHere is an example.\n\n```rust\nfn main() {\n    println!(\"hi\");\n}\n```\n\nThat was the example.\n";
    let chunks = run("Lib", body);
    assert_every_chunk_has_balanced_fences(&chunks, "small fenced block");
    let with_code = chunks
        .iter()
        .find(|c| c.body.contains("```"))
        .expect("a chunk with the code block");
    assert!(with_code.body.contains("fn main()"));
}

#[test]
fn a_code_block_over_the_maximum_becomes_one_oversize_chunk_with_balanced_fences() {
    let big_code = format!("```text\n{}\n```\n", "a line of code\n".repeat(950));
    let body = format!(
        "# Section\nSome introduction text before the huge block.\n\n{big_code}\n\nAnd some text after it.\n"
    );
    let chunks = run("Lib", &body);
    assert_every_chunk_has_balanced_fences(&chunks, "oversize code block");

    let oversize_chunks: Vec<&Chunk> = chunks.iter().filter(|c| c.oversize).collect();
    assert_eq!(
        oversize_chunks.len(),
        1,
        "exactly one chunk must be marked oversize"
    );
    assert!(oversize_chunks[0].tokens > MAX_TOKENS);
    assert!(oversize_chunks[0].body.trim_start().starts_with("```text"));
    assert!(oversize_chunks[0].body.trim_end().ends_with("```"));
}

#[test]
fn back_to_back_fenced_code_blocks_each_stay_balanced() {
    let body = "# Section\nintro\n\n```rust\nfn a() {}\n```\n\n```python\ndef b(): pass\n```\n\n```go\nfunc c() {}\n```\n\noutro\n";
    let chunks = run("Lib", body);
    assert_every_chunk_has_balanced_fences(&chunks, "back-to-back code blocks");
    let joined_bodies: String = chunks
        .iter()
        .map(|c| c.body.as_str())
        .collect::<Vec<_>>()
        .join("");
    assert!(joined_bodies.contains("fn a()"));
    assert!(joined_bodies.contains("def b()"));
    assert!(joined_bodies.contains("func c()"));
}

#[test]
fn a_fence_character_appearing_inside_a_different_fence_does_not_close_it_early() {
    // A backtick fence containing a line of tildes must stay open until
    // its own matching backtick fence closes it.
    let body = "# Section\n```rust\nlet s = \"~~~ this looks like a fence but is not one ~~~\";\n```\nafter\n";
    let chunks = run("Lib", body);
    assert_every_chunk_has_balanced_fences(&chunks, "mismatched fence character inside a block");
}

#[test]
fn a_hash_line_inside_a_fence_never_starts_a_new_section_or_breaks_the_fence() {
    let body = "# Real Section\n```python\n# this looks like a heading but is a comment\nprint('hi')\n```\nafter the block\n";
    let chunks = run("Lib", body);
    assert_every_chunk_has_balanced_fences(&chunks, "hash-comment inside a fence");
    // The whole document collapses to sections keyed only on the one real
    // heading: the in-fence `#` line must not have produced a second,
    // spurious section.
    let breadcrumbs: std::collections::HashSet<&str> =
        chunks.iter().map(|c| c.breadcrumb.as_str()).collect();
    assert_eq!(
        breadcrumbs.len(),
        1,
        "an in-fence '#' must not create a new section"
    );
}

#[test]
fn many_documents_with_varied_fence_shapes_all_pass() {
    let cases: &[(&str, &str)] = &[
        (
            "no fences at all",
            "# Section\njust some plain prose with no code in it whatsoever\n",
        ),
        (
            "a tilde fence",
            "# Section\n~~~\ncode inside a tilde fence\n~~~\nafter\n",
        ),
        (
            "an indented fence",
            "# Section\n   ```\n   indented code\n   ```\nafter\n",
        ),
        (
            "nested headings around a code block",
            "# One\nintro\n## Two\n```rust\nfn f() {}\n```\n### Three\nmore text down here to pad it out a bit\n",
        ),
    ];
    for (name, body) in cases {
        let chunks = run("Lib", body);
        assert_every_chunk_has_balanced_fences(&chunks, name);
    }
}
