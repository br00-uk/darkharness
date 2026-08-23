//! The `heading-v1` algorithm.
//!
//! [`run`] is the whole algorithm: split the document into leaf sections by
//! heading, split any section that is too large into blocks that respect
//! fence boundaries, merge any chunk that is too small into a sibling, then
//! assign ordinals and identifiers. Every other item in this module is a
//! step of that pipeline, in the order `run` calls them.

use dark_contract::Result;

use crate::chunk::id;
use crate::chunk::markdown::{Line, classify_lines, fence_marker};
use crate::chunk::{Chunk, MAX_TOKENS, MIN_TOKENS, TokenCounter};
use crate::ingest::Document;

/// The separator that joins breadcrumb segments.
///
/// U+203A SINGLE RIGHT-POINTING ANGLE QUOTATION MARK, matching the
/// specification's own example: `tokio › runtime › Builder ›
/// worker_threads`.
pub const BREADCRUMB_SEPARATOR: &str = " › ";

/// Runs `heading-v1` over `doc`, producing its final chunks.
///
/// `pack_id` is the pack identifier that [`id::compute`] folds into every
/// chunk identifier, for example `tokio@1.47.0`. The breadcrumb's own
/// first segment is the library name read off the front of `pack_id` (the
/// text before `@`), so a chunk from `tokio@1.47.0` gets a breadcrumb
/// starting `tokio › …`. `G3`'s example breadcrumb names no other source
/// for that first segment, so this is `heading-v1`'s reading of Do 6 and
/// Do 9 together: see the chunk module's docs for the note on this
/// choice.
///
/// # Errors
///
/// Returns whatever `counter.count` returns. A [`TokenCounter`] backed by
/// [`dark_contract::Engine::tokenize`] fails when no tokenizer is loaded
/// for the given role class.
pub fn run(counter: &dyn TokenCounter, pack_id: &str, doc: &Document) -> Result<Vec<Chunk>> {
    let library = pack_id.split('@').next().unwrap_or(pack_id).to_owned();
    let root = vec![library, doc.title.clone()];

    let sections = split_into_sections(root, &doc.body);

    let mut raw = Vec::new();
    for (breadcrumb, content) in sections {
        raw.extend(split_section(counter, &breadcrumb, &content)?);
    }

    let merged = merge_undersized(raw);
    Ok(finalize(pack_id, doc.url.as_deref(), merged))
}

/// One chunk before its ordinal and identifier are assigned.
struct RawChunk {
    /// The full ancestor chain, first segment is the library name.
    breadcrumb: Vec<String>,
    /// The Markdown content, with headings stripped.
    content: String,
    /// The token count of `content`, from the same [`TokenCounter`] that
    /// built it.
    tokens: usize,
    /// Set when this chunk is a single block over the maximum that
    /// `heading-v1` refused to split further.
    oversize: bool,
    /// Set by [`merge_undersized`] once it has confirmed this chunk has no
    /// sibling that can absorb it. Excludes it from further merge
    /// attempts without altering `tokens`, which [`finalize`] still reads
    /// as the true count.
    settled: bool,
}

impl RawChunk {
    /// Returns the breadcrumb with its own last segment removed: the
    /// breadcrumb that every true sibling of this chunk also has.
    fn parent_breadcrumb(&self) -> &[String] {
        let len = self.breadcrumb.len();
        &self.breadcrumb[..len.saturating_sub(1)]
    }
}

/// Splits `body` into leaf sections: `(breadcrumb, content)` pairs, where
/// `content` is the text that falls directly under the deepest heading
/// active at that point, headings inside excluded.
///
/// `root` seeds the breadcrumb stack and never pops: it carries no heading
/// level of its own, so no heading in `body` closes it.
fn split_into_sections(root: Vec<String>, body: &str) -> Vec<(Vec<String>, String)> {
    let lines = classify_lines(body);
    let mut sections = Vec::new();
    let mut levels: Vec<u8> = vec![0; root.len()];
    let mut stack = root;
    let mut current = String::new();

    for line in lines {
        match line {
            Line::Heading { level, text } => {
                flush_section(&mut sections, &stack, &mut current);
                while levels.last().is_some_and(|&top| top >= level) {
                    stack.pop();
                    levels.pop();
                }
                stack.push(text.to_owned());
                levels.push(level);
            }
            Line::Text(text) => {
                current.push_str(text);
                current.push('\n');
            }
        }
    }
    flush_section(&mut sections, &stack, &mut current);
    sections
}

/// Pushes `(stack, content)` onto `sections` when `content` holds more
/// than whitespace, and clears `content` either way.
fn flush_section(
    sections: &mut Vec<(Vec<String>, String)>,
    stack: &[String],
    content: &mut String,
) {
    if content.trim().is_empty() {
        content.clear();
    } else {
        sections.push((stack.to_vec(), std::mem::take(content)));
    }
}

/// One unit that block-splitting never divides.
enum Block {
    /// Prose: one or more non-blank lines with no fence marker.
    Paragraph(String),
    /// A whole fenced code block, opening and closing delimiters included.
    Code(String),
}

impl Block {
    fn text(&self) -> &str {
        match self {
            Self::Paragraph(text) | Self::Code(text) => text,
        }
    }
}

/// Splits `content` into [`Block`]s: paragraphs, separated on blank lines,
/// and whole fenced code blocks that a paragraph boundary never enters.
///
/// This is `heading-v1`'s guarantee that a fenced code block is never
/// split (Do 5): a code block becomes exactly one [`Block::Code`], and
/// [`split_section`] only ever flushes or holds a block whole, never a
/// fragment of one.
fn split_into_blocks(content: &str) -> Vec<Block> {
    let mut blocks = Vec::new();
    let mut current = String::new();
    let mut fence_char: Option<char> = None;

    for line in content.lines() {
        if let Some(marker) = fence_marker(line) {
            match fence_char {
                None => {
                    flush_paragraph(&mut blocks, &mut current);
                    fence_char = Some(marker);
                    current.push_str(line);
                    current.push('\n');
                }
                Some(open) if open == marker => {
                    current.push_str(line);
                    current.push('\n');
                    blocks.push(Block::Code(std::mem::take(&mut current)));
                    fence_char = None;
                }
                Some(_) => {
                    current.push_str(line);
                    current.push('\n');
                }
            }
            continue;
        }
        if fence_char.is_some() {
            current.push_str(line);
            current.push('\n');
            continue;
        }
        if line.trim().is_empty() {
            flush_paragraph(&mut blocks, &mut current);
        } else {
            current.push_str(line);
            current.push('\n');
        }
    }
    // An unterminated fence at end of input is malformed Markdown.
    // `heading-v1` assumes a well-formed document (every opened fence
    // closes); this still emits whatever text was collected, as a `Code`
    // block, rather than silently drop it, but it cannot manufacture a
    // closing fence that was never there.
    if fence_char.is_some() {
        blocks.push(Block::Code(std::mem::take(&mut current)));
    } else {
        flush_paragraph(&mut blocks, &mut current);
    }
    blocks
}

fn flush_paragraph(blocks: &mut Vec<Block>, current: &mut String) {
    if current.trim().is_empty() {
        current.clear();
    } else {
        blocks.push(Block::Paragraph(std::mem::take(current)));
    }
}

/// Splits one leaf section into one or more [`RawChunk`]s, greedily
/// filling toward [`MAX_TOKENS`].
///
/// A block whose own token count already exceeds [`MAX_TOKENS`] — in
/// practice, a large fenced code block — becomes its own chunk, marked
/// `oversize`, per Do 5. An oversized plain-text block (no fence in
/// sight, just a very long paragraph) gets the same treatment:
/// `heading-v1` states a rule for a code block over the maximum, and no
/// rule at all for a plain block over the maximum, so this extends the
/// stated rule to the unstated case rather than leave such a block with
/// nowhere to go.
fn split_section(
    counter: &dyn TokenCounter,
    breadcrumb: &[String],
    content: &str,
) -> Result<Vec<RawChunk>> {
    let blocks = split_into_blocks(content);
    let mut chunks = Vec::new();
    let mut current = String::new();
    let mut current_tokens = 0usize;

    for block in blocks {
        let block_text = block.text();
        if block_text.trim().is_empty() {
            continue;
        }
        let block_tokens = counter.count(block_text)?;

        if block_tokens > MAX_TOKENS {
            flush_accumulated(&mut chunks, breadcrumb, &mut current, &mut current_tokens);
            chunks.push(RawChunk {
                breadcrumb: breadcrumb.to_vec(),
                content: block_text.to_owned(),
                tokens: block_tokens,
                oversize: true,
                settled: false,
            });
            continue;
        }

        if current_tokens > 0 && current_tokens + block_tokens > MAX_TOKENS {
            flush_accumulated(&mut chunks, breadcrumb, &mut current, &mut current_tokens);
        }
        if !current.is_empty() {
            current.push('\n');
        }
        current.push_str(block_text);
        current_tokens += block_tokens;
    }
    flush_accumulated(&mut chunks, breadcrumb, &mut current, &mut current_tokens);
    Ok(chunks)
}

/// Flushes the content accumulated so far as one [`RawChunk`], when it
/// holds more than whitespace.
fn flush_accumulated(
    chunks: &mut Vec<RawChunk>,
    breadcrumb: &[String],
    current: &mut String,
    current_tokens: &mut usize,
) {
    if current.trim().is_empty() {
        current.clear();
    } else {
        chunks.push(RawChunk {
            breadcrumb: breadcrumb.to_vec(),
            content: std::mem::take(current),
            tokens: *current_tokens,
            oversize: false,
            settled: false,
        });
    }
    *current_tokens = 0;
}

/// Merges every chunk under [`MIN_TOKENS`] into a sibling — a chunk whose
/// parent breadcrumb ([`RawChunk::parent_breadcrumb`]) matches the small
/// chunk's own — per Do 4.
///
/// This looks forward first, then backward, for the nearest sibling in
/// document order, and merges by prepending the small chunk's content onto
/// the sibling's; the sibling keeps its own breadcrumb, its own position,
/// and absorbs the token count. A chunk with no sibling at all (an only
/// child, or the whole document) is left as it is: there is nothing to
/// merge it into. A merge that would push the receiving chunk over
/// [`MAX_TOKENS`] is skipped in favour of the harder ceiling, and the
/// search tries the next candidate sibling instead.
///
/// This runs to a fixed point. Merging can leave a newly-enlarged chunk
/// still under the minimum, when both it and its donor started small, so
/// the pass repeats until one full pass makes no change.
fn merge_undersized(mut chunks: Vec<RawChunk>) -> Vec<RawChunk> {
    loop {
        let is_candidate = |c: &RawChunk| c.tokens < MIN_TOKENS && !c.oversize && !c.settled;
        let Some(small_index) = chunks.iter().position(is_candidate) else {
            return chunks;
        };
        let parent = chunks[small_index].parent_breadcrumb().to_vec();

        let forward = chunks
            .iter()
            .enumerate()
            .skip(small_index + 1)
            .find(|(_, c)| c.parent_breadcrumb() == parent.as_slice())
            .map(|(i, _)| i);
        let backward = chunks[..small_index]
            .iter()
            .enumerate()
            .rev()
            .find(|(_, c)| c.parent_breadcrumb() == parent.as_slice())
            .map(|(i, _)| i);

        let target = [forward, backward]
            .into_iter()
            .flatten()
            .find(|&i| chunks[small_index].tokens + chunks[i].tokens <= MAX_TOKENS);

        let Some(target) = target else {
            // No sibling can absorb this chunk without breaking the
            // maximum, or this chunk has no sibling at all. Leave it
            // exactly as it is and exclude it from the next search, so
            // the loop makes progress instead of re-selecting it forever.
            chunks[small_index].settled = true;
            continue;
        };

        let donor = chunks.remove(small_index);
        let target = if target > small_index {
            target - 1
        } else {
            target
        };
        if target < small_index {
            chunks[target].content = format!("{}\n\n{}", donor.content, chunks[target].content);
        } else {
            chunks[target].content = format!("{}\n\n{}", chunks[target].content, donor.content);
        }
        chunks[target].tokens += donor.tokens;
    }
}

/// Assigns ordinals and identifiers, producing the final [`Chunk`] values
/// in document order.
fn finalize(pack_id: &str, url: Option<&str>, chunks: Vec<RawChunk>) -> Vec<Chunk> {
    chunks
        .into_iter()
        .enumerate()
        .map(|(index, raw)| {
            let ordinal = u32::try_from(index).unwrap_or(u32::MAX);
            let breadcrumb = raw.breadcrumb.join(BREADCRUMB_SEPARATOR);
            let chunk_id = id::compute(pack_id, &breadcrumb, ordinal);
            let anchor = raw
                .breadcrumb
                .last()
                .map(|s| slugify(s))
                .unwrap_or_default();
            let chunk_url = url
                .filter(|_| !anchor.is_empty())
                .map(|u| format!("{u}#{anchor}"));
            let body = raw.content.trim().to_owned();
            let embed_text = format!("{breadcrumb}\n\n{body}");
            Chunk {
                chunk_id,
                ordinal,
                breadcrumb,
                url: chunk_url,
                body,
                embed_text,
                tokens: raw.tokens,
                oversize: raw.oversize,
            }
        })
        .collect()
}

/// Converts heading text into a URL anchor: lowercase, non-alphanumeric
/// runs become one hyphen, and the ends are trimmed of hyphens. This
/// mirrors GitHub's own heading-anchor convention closely enough for a
/// documentation cross-reference to resolve.
fn slugify(text: &str) -> String {
    let mut out = String::new();
    let mut last_was_hyphen = true;
    for c in text.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            last_was_hyphen = false;
        } else if !last_was_hyphen {
            out.push('-');
            last_was_hyphen = true;
        }
    }
    out.trim_end_matches('-').to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunk::markdown::has_balanced_fences;

    #[test]
    fn split_into_blocks_keeps_a_fenced_code_block_whole() {
        let content = "intro text\n\n```rust\nfn f() {}\n```\n\noutro text\n";
        let blocks = split_into_blocks(content);
        assert_eq!(blocks.len(), 3);
        assert!(matches!(blocks[1], Block::Code(_)));
        assert!(blocks[1].text().contains("```"));
        assert!(has_balanced_fences(blocks[1].text()));
    }

    #[test]
    fn split_into_sections_builds_a_breadcrumb_stack() {
        let sections = split_into_sections(
            vec!["tokio".to_owned()],
            "# Runtime\nintro\n## Builder\nbuilder text\n### worker_threads\nleaf text\n",
        );
        let breadcrumbs: Vec<Vec<String>> = sections.into_iter().map(|(b, _)| b).collect();
        assert_eq!(
            breadcrumbs,
            vec![
                vec!["tokio".to_owned(), "Runtime".to_owned()],
                vec![
                    "tokio".to_owned(),
                    "Runtime".to_owned(),
                    "Builder".to_owned()
                ],
                vec![
                    "tokio".to_owned(),
                    "Runtime".to_owned(),
                    "Builder".to_owned(),
                    "worker_threads".to_owned(),
                ],
            ]
        );
    }

    #[test]
    fn sibling_sections_share_a_parent_breadcrumb() {
        let sections = split_into_sections(
            vec!["tokio".to_owned()],
            "# Runtime\n## One\ntext one\n## Two\ntext two\n",
        );
        let a = RawChunk {
            breadcrumb: sections[0].0.clone(),
            content: String::new(),
            tokens: 0,
            oversize: false,
            settled: false,
        };
        let b = RawChunk {
            breadcrumb: sections[1].0.clone(),
            content: String::new(),
            tokens: 0,
            oversize: false,
            settled: false,
        };
        assert_eq!(a.parent_breadcrumb(), b.parent_breadcrumb());
    }

    #[test]
    fn slugify_lowercases_and_hyphenates() {
        assert_eq!(slugify("worker_threads"), "worker-threads");
        assert_eq!(slugify("Builder"), "builder");
        assert_eq!(slugify("  Multiple   Spaces  "), "multiple-spaces");
    }
}
