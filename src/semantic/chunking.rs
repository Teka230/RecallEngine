use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Configuration options for deterministic chunking.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChunkerOptions {
    /// Maximum character length of a single chunk (default: 512).
    pub max_chunk_chars: usize,
    /// Overlap in characters between consecutive chunks (default: 64).
    pub overlap_chars: usize,
}

impl Default for ChunkerOptions {
    fn default() -> Self {
        Self {
            max_chunk_chars: 512,
            overlap_chars: 64,
        }
    }
}

impl ChunkerOptions {
    /// Canonical version identifier for these chunker options.
    pub fn version(&self) -> String {
        format!("v1-{}-{}", self.max_chunk_chars, self.overlap_chars)
    }
}

/// A deterministic chunk of text mapped to canonical database entities.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextChunk {
    /// Canonical content block ID (corresponds to content_blocks.id TEXT).
    pub content_block_id: String,
    /// Portable source message ID (UUID).
    pub message_id: String,
    /// Stable internal citation (IC) if message is an eligible user/assistant message.
    pub ic: Option<i64>,
    /// Zero-based chunk index within this content block.
    pub chunk_index: usize,
    /// Total number of chunks generated for this content block.
    pub total_chunks: usize,
    /// Version identifier for chunker settings (e.g. "v1-512-64").
    pub chunker_version: String,
    /// Model ID for which this chunk and vector are intended.
    pub model_id: String,
    /// Deterministic SHA-256 hex hash of the normalized chunk text.
    pub chunk_hash: String,
    /// Composite fingerprint: sha256(model_id:chunker_version:chunk_hash).
    pub fingerprint: String,
    /// The actual text payload.
    pub text: String,
}

/// Compute a deterministic SHA-256 hex digest for a string.
pub fn compute_sha256(text: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(text.as_bytes());
    format!("{:x}", hasher.finalize())
}

/// Compute a composite fingerprint combining model identity, chunker configuration, and chunk text hash.
pub fn compute_chunk_fingerprint(
    model_id: &str,
    chunker_version: &str,
    chunk_hash: &str,
) -> String {
    let input = format!("{model_id}:{chunker_version}:{chunk_hash}");
    compute_sha256(&input)
}

/// Deterministically chunks text from a content block while preserving boundaries.
pub fn chunk_content_block(
    content_block_id: &str,
    message_id: &str,
    ic: Option<i64>,
    text: &str,
    options: &ChunkerOptions,
    model_id: &str,
) -> Vec<TextChunk> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }

    let chunker_version = options.version();
    let max_len = options.max_chunk_chars.max(64);
    let overlap = options.overlap_chars.min(max_len / 2);

    if trimmed.chars().count() <= max_len {
        let chunk_hash = compute_sha256(trimmed);
        let fingerprint = compute_chunk_fingerprint(model_id, &chunker_version, &chunk_hash);
        return vec![TextChunk {
            content_block_id: content_block_id.to_string(),
            message_id: message_id.to_string(),
            ic,
            chunk_index: 0,
            total_chunks: 1,
            chunker_version,
            model_id: model_id.to_string(),
            chunk_hash,
            fingerprint,
            text: trimmed.to_string(),
        }];
    }

    // Split text into coherent paragraphs and sentences
    let mut raw_chunks = Vec::new();
    let chars: Vec<char> = trimmed.chars().collect();
    let total_len = chars.len();
    let mut start = 0;

    while start < total_len {
        let mut end = (start + max_len).min(total_len);

        if end < total_len {
            // Find a natural boundary (newline, punctuation, space)
            let window_start = (start + max_len / 2).min(end);
            let mut split_point = None;

            // Priority 1: double newline
            for idx in (window_start..end).rev() {
                if idx + 1 < total_len && chars[idx] == '\n' && chars[idx + 1] == '\n' {
                    split_point = Some(idx + 2);
                    break;
                }
            }

            // Priority 2: sentence ending punctuation
            if split_point.is_none() {
                for idx in (window_start..end).rev() {
                    if (chars[idx] == '.'
                        || chars[idx] == '!'
                        || chars[idx] == '?'
                        || chars[idx] == '\n')
                        && (idx + 1 == total_len || chars[idx + 1].is_whitespace())
                    {
                        split_point = Some(idx + 1);
                        break;
                    }
                }
            }

            // Priority 3: any whitespace
            if split_point.is_none() {
                for idx in (window_start..end).rev() {
                    if chars[idx].is_whitespace() {
                        split_point = Some(idx + 1);
                        break;
                    }
                }
            }

            if let Some(sp) = split_point {
                end = sp;
            }
        }

        let chunk_str: String = chars[start..end].iter().collect();
        let chunk_trimmed = chunk_str.trim().to_string();
        if !chunk_trimmed.is_empty() {
            raw_chunks.push(chunk_trimmed);
        }

        if end >= total_len {
            break;
        }

        // Advance start considering overlap
        let next_start = if end > overlap { end - overlap } else { end };

        if next_start <= start {
            start = end;
        } else {
            start = next_start;
        }
    }

    let total_chunks = raw_chunks.len();
    raw_chunks
        .into_iter()
        .enumerate()
        .map(|(chunk_index, chunk_text)| {
            let chunk_hash = compute_sha256(&chunk_text);
            let fingerprint = compute_chunk_fingerprint(model_id, &chunker_version, &chunk_hash);
            TextChunk {
                content_block_id: content_block_id.to_string(),
                message_id: message_id.to_string(),
                ic,
                chunk_index,
                total_chunks,
                chunker_version: chunker_version.clone(),
                model_id: model_id.to_string(),
                chunk_hash,
                fingerprint,
                text: chunk_text,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_MODEL: &str = "test-model/e5-small";

    #[test]
    fn test_short_text_single_chunk() {
        let opts = ChunkerOptions::default();
        let chunks = chunk_content_block(
            "cb-uuid-1",
            "msg-1",
            Some(42),
            "Court message de test.",
            &opts,
            TEST_MODEL,
        );

        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].chunk_index, 0);
        assert_eq!(chunks[0].total_chunks, 1);
        assert_eq!(chunks[0].content_block_id, "cb-uuid-1");
        assert_eq!(chunks[0].message_id, "msg-1");
        assert_eq!(chunks[0].ic, Some(42));
        assert_eq!(chunks[0].text, "Court message de test.");
        assert_eq!(
            chunks[0].chunk_hash,
            compute_sha256("Court message de test.")
        );
        assert_eq!(
            chunks[0].fingerprint,
            compute_chunk_fingerprint(TEST_MODEL, &opts.version(), &chunks[0].chunk_hash)
        );
    }

    #[test]
    fn test_empty_text_returns_empty() {
        let opts = ChunkerOptions::default();
        let chunks = chunk_content_block("cb-uuid-1", "msg-1", Some(42), "   ", &opts, TEST_MODEL);
        assert!(chunks.is_empty());
    }

    #[test]
    fn test_deterministic_multi_chunk_with_overlap() {
        let opts = ChunkerOptions {
            max_chunk_chars: 100,
            overlap_chars: 20,
        };

        let text = "Premier paragraphe expliquant l'architecture de RecallEngine.\n\n\
                    Deuxieme paragraphe detaillant le mecanisme de recherche plein texte FTS5 et les identifiants stables.\n\n\
                    Troisieme paragraphe sur l'integration des embeddings vectoriels.";

        let chunks1 =
            chunk_content_block("cb-uuid-10", "msg-10", Some(100), text, &opts, TEST_MODEL);
        let chunks2 =
            chunk_content_block("cb-uuid-10", "msg-10", Some(100), text, &opts, TEST_MODEL);

        assert_eq!(chunks1, chunks2, "Chunking must be 100% deterministic");
        assert!(chunks1.len() >= 3);

        for (i, c) in chunks1.iter().enumerate() {
            assert_eq!(c.chunk_index, i);
            assert_eq!(c.total_chunks, chunks1.len());
            assert_eq!(c.content_block_id, "cb-uuid-10");
            assert_eq!(c.message_id, "msg-10");
            assert_eq!(c.ic, Some(100));
            assert_eq!(c.chunk_hash, compute_sha256(&c.text));
            assert_eq!(
                c.fingerprint,
                compute_chunk_fingerprint(TEST_MODEL, &opts.version(), &c.chunk_hash)
            );
        }
    }
}
