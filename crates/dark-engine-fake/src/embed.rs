//! Fake embeddings.
//!
//! The vector for a text is a hashed bag of words. Two texts that share words
//! therefore land close together, and a test controls the cosine similarity by
//! choosing how much vocabulary two texts share. This is what makes retrieval
//! tests meaningful without a real embedding model.
//!
//! A query and a document embed slightly differently, as an asymmetric model
//! does. The difference is a small bias added after the bag of words, not a
//! different hash: query and document vectors must stay in one comparable
//! space, or reranking a document against a query would score noise.

use dark_contract::EmbedPurpose;

use crate::script::EmbedSpec;

/// How strongly the purpose bias pulls a vector.
///
/// Large enough that a query and a document differ. Small enough that shared
/// vocabulary still decides the ranking.
const PURPOSE_WEIGHT: f32 = 0.1;

/// Builds the vector for one text.
///
/// A fixed vector in the specification wins over the hashed value.
pub(crate) fn embed_text(spec: &EmbedSpec, text: &str, purpose: EmbedPurpose) -> Vec<f32> {
    let dim = spec.dim.max(1);

    if let Some(fixed) = spec.fixed.iter().find(|entry| entry.text == text) {
        let mut vector = fixed.vector.clone();
        vector.resize(dim, 0.0);
        normalise(&mut vector);
        return vector;
    }

    let mut vector = vec![0.0_f32; dim];
    let mut words_seen = 0_usize;

    for word in words(text) {
        words_seen += 1;
        let hash = hash_bytes(word.bytes());
        let bucket = bucket_of(hash, dim);
        // The sign comes from a separate bit, so unrelated words that share a
        // bucket cancel instead of always reinforcing.
        let sign = if hash & (1 << 63) == 0 { 1.0 } else { -1.0 };
        vector[bucket] += sign;
    }

    // A text with no words has no vector. Adding the bias here would make
    // every empty text look alike and non-zero.
    if words_seen == 0 {
        return vector;
    }

    normalise(&mut vector);

    for (value, bias) in vector.iter_mut().zip(purpose_bias(dim, purpose)) {
        *value += PURPOSE_WEIGHT * bias;
    }
    normalise(&mut vector);

    vector
}

/// Returns the fixed bias vector for a purpose.
#[allow(clippy::cast_precision_loss)]
fn purpose_bias(dim: usize, purpose: EmbedPurpose) -> Vec<f32> {
    let seed: &[u8] = match purpose {
        EmbedPurpose::Query => b"query",
        EmbedPurpose::Document => b"document",
    };

    let mut bias: Vec<f32> = (0..dim)
        .map(|index| {
            let hash = hash_bytes(seed.iter().copied().chain(index.to_le_bytes()));
            // Map the low bits to the range -1.0 to 1.0.
            ((hash % 2000) as f32 / 1000.0) - 1.0
        })
        .collect();
    normalise(&mut bias);
    bias
}

/// Splits text into lowercase words.
fn words(text: &str) -> impl Iterator<Item = String> + '_ {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(str::to_lowercase)
}

/// Hashes bytes with FNV-1a.
///
/// This is not a good hash for security. It is a good hash for a test double:
/// short, deterministic, and identical on every platform.
fn hash_bytes(bytes: impl Iterator<Item = u8>) -> u64 {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;

    let mut hash = OFFSET;
    for byte in bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(PRIME);
    }
    hash
}

/// Maps a hash onto a bucket index.
fn bucket_of(hash: u64, dim: usize) -> usize {
    let dim_u64 = dim as u64;
    usize::try_from(hash % dim_u64).unwrap_or(0)
}

/// Scales a vector to unit length. A zero vector is left alone.
fn normalise(vector: &mut [f32]) {
    let norm = vector.iter().map(|value| value * value).sum::<f32>().sqrt();
    if norm > f32::EPSILON {
        for value in vector.iter_mut() {
            *value /= norm;
        }
    }
}

/// Returns the cosine similarity of two vectors of the same length.
///
/// Both inputs are already unit length, so this is their dot product.
pub(crate) fn cosine(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> EmbedSpec {
        EmbedSpec {
            dim: 256,
            fixed: Vec::new(),
        }
    }

    fn doc(text: &str) -> Vec<f32> {
        embed_text(&spec(), text, EmbedPurpose::Document)
    }

    fn query(text: &str) -> Vec<f32> {
        embed_text(&spec(), text, EmbedPurpose::Query)
    }

    #[test]
    fn a_vector_has_unit_length() {
        let norm = doc("tokio runtime builder")
            .iter()
            .map(|v| v * v)
            .sum::<f32>()
            .sqrt();
        assert!((norm - 1.0).abs() < 1e-5, "norm was {norm}");
    }

    #[test]
    fn the_same_text_gives_the_same_vector() {
        assert_eq!(doc("worker threads"), doc("worker threads"));
    }

    #[test]
    fn shared_words_raise_the_similarity() {
        // This is the property retrieval tests rely on.
        let base = doc("tokio runtime worker threads");
        let close = doc("tokio runtime worker threads configuration");
        let far = doc("postgres replication lag monitoring");

        let near_score = cosine(&base, &close);
        let far_score = cosine(&base, &far);

        assert!(near_score > 0.7, "expected a high score, got {near_score}");
        assert!(far_score < 0.3, "expected a low score, got {far_score}");
    }

    #[test]
    fn a_query_still_matches_its_document() {
        // Reranking scores a query against documents, so the two purposes must
        // share one space. A per-purpose hash would make this score noise.
        let q = query("tokio runtime worker threads");
        let relevant = doc("tokio runtime worker threads configuration");
        let irrelevant = doc("postgres replication lag monitoring");

        let hit = cosine(&q, &relevant);
        let miss = cosine(&q, &irrelevant);

        assert!(hit > 0.6, "a query must match its document, got {hit}");
        assert!(hit > miss + 0.3, "hit {hit} must clearly beat miss {miss}");
    }

    #[test]
    fn a_text_matches_itself_exactly() {
        let vector = doc("tokio");
        assert!((cosine(&vector, &vector) - 1.0).abs() < 1e-5);
    }

    #[test]
    fn the_purpose_changes_the_vector() {
        // An asymmetric model embeds a query differently from a document.
        assert_ne!(query("worker threads"), doc("worker threads"));
    }

    #[test]
    fn case_and_punctuation_do_not_matter() {
        assert_eq!(doc("Worker, Threads!"), doc("worker threads"));
    }

    #[test]
    fn a_fixed_vector_overrides_the_hash() {
        let spec = EmbedSpec {
            dim: 4,
            fixed: vec![crate::script::FixedVector {
                text: "anchor".into(),
                vector: vec![3.0, 4.0, 0.0, 0.0],
            }],
        };
        let vector = embed_text(&spec, "anchor", EmbedPurpose::Document);
        // 3-4-5 triangle, so the unit form is 0.6 and 0.8.
        assert!((vector[0] - 0.6).abs() < 1e-5, "got {vector:?}");
        assert!((vector[1] - 0.8).abs() < 1e-5, "got {vector:?}");
    }

    #[test]
    fn an_empty_text_gives_a_zero_vector() {
        assert!(doc("").iter().all(|v| *v == 0.0));
    }
}
