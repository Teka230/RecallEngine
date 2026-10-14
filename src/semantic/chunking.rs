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

/// A deterministic chunk of text mapped to canonical database entities.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextChunk {
    /// Canonical content block row id.
    pub content_block_id: i64,
    /// Portable source message ID (UUID).
    pub message_id: String,
    /// Stable internal citation (IC) if message is an eligible user/assistant message.
    pub ic: Option<i64>,
    /// Zero-based chunk index within this content block.
    pub chunk_index: usize,
    /// Total number of chunks generated for this content block.
    pub total_chunks: usize,
    /// Deterministic SHA-256 hex hash of the normalized chunk text.
    pub chunk_hash: String,
    /// The actual text payload.
    pub text: String,
}

/// Compute a deterministic SHA-256 hex digest for a string.
pub fn compute_sha256(text: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(text.as_bytes());
    format!("{:x}", hasher.finalize())
}

/// Deterministically chunks text from a content block while preserving boundaries.
pub fn chunk_content_block(
    content_block_id: i64,
    message_id: &str,
    ic: Option<i64>,
    text: &str,
    options: &ChunkerOptions,
) -> Vec<TextChunk> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }

    let max_len = options.max_chunk_chars.max(64);
    let overlap = options.overlap_chars.min(max_len / 2);

    if trimmed.chars().count() <= max_len {
        let chunk_hash = compute_sha256(trimmed);
        return vec![TextChunk {
            content_block_id,
            message_id: message_id.to_string(),
            ic,
            chunk_index: 0,
            total_chunks: 1,
            chunk_hash,
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
            TextChunk {
                content_block_id,
                message_id: message_id.to_string(),
                ic,
                chunk_index,
                total_chunks,
                chunk_hash,
                text: chunk_text,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_short_text_single_chunk() {
        let opts = ChunkerOptions::default();
        let chunks = chunk_content_block(1, "msg-1", Some(42), "Court message de test.", &opts);

        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].chunk_index, 0);
        assert_eq!(chunks[0].total_chunks, 1);
        assert_eq!(chunks[0].content_block_id, 1);
        assert_eq!(chunks[0].message_id, "msg-1");
        assert_eq!(chunks[0].ic, Some(42));
        assert_eq!(chunks[0].text, "Court message de test.");
        assert_eq!(
            chunks[0].chunk_hash,
            compute_sha256("Court message de test.")
        );
    }

    #[test]
    fn test_empty_text_returns_empty() {
        let opts = ChunkerOptions::default();
        let chunks = chunk_content_block(1, "msg-1", Some(42), "   ", &opts);
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

        let chunks1 = chunk_content_block(10, "msg-10", Some(100), text, &opts);
        let chunks2 = chunk_content_block(10, "msg-10", Some(100), text, &opts);

        assert_eq!(chunks1, chunks2, "Chunking must be 100% deterministic");
        assert!(chunks1.len() >= 3);

        for (i, c) in chunks1.iter().enumerate() {
            assert_eq!(c.chunk_index, i);
            assert_eq!(c.total_chunks, chunks1.len());
            assert_eq!(c.content_block_id, 10);
            assert_eq!(c.message_id, "msg-10");
            assert_eq!(c.ic, Some(100));
            assert_eq!(c.chunk_hash, compute_sha256(&c.text));
        }
    }
}
