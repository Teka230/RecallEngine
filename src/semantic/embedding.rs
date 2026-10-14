use anyhow::{anyhow, Result};
use fastembed::{EmbeddingModel, TextEmbedding, TextInitOptions};
use sha2::{Digest, Sha256};
use std::path::PathBuf;

/// Trait defining an embedding provider for passages and queries.
pub trait EmbeddingProvider {
    /// Generate embeddings for a batch of passage documents.
    fn embed_passages(&mut self, texts: &[&str]) -> Result<Vec<Vec<f32>>>;
    /// Generate an embedding for a search query.
    fn embed_query(&mut self, text: &str) -> Result<Vec<f32>>;
    /// Dimensionality of the vectors produced by this model.
    fn dimension(&self) -> usize;
    /// Identifier or name of the model.
    fn model_id(&self) -> &str;
}

/// FastEmbed provider using intfloat/multilingual-e5-small (384 dimensions).
pub struct FastEmbedProvider {
    model: TextEmbedding,
    model_id: String,
    dimension: usize,
}

impl FastEmbedProvider {
    /// Creates a new FastEmbedProvider with optional custom cache dir.
    pub fn try_new(cache_dir: Option<PathBuf>) -> Result<Self> {
        let mut options = TextInitOptions::new(EmbeddingModel::MultilingualE5Small)
            .with_show_download_progress(false);

        if let Some(dir) = cache_dir {
            options = options.with_cache_dir(dir);
        }

        let model = TextEmbedding::try_new(options)
            .map_err(|e| anyhow!("Failed to initialize FastEmbed model: {e}"))?;

        Ok(Self {
            model,
            model_id: "intfloat/multilingual-e5-small".to_string(),
            dimension: 384,
        })
    }
}

impl EmbeddingProvider for FastEmbedProvider {
    fn embed_passages(&mut self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }

        // E5 requires "passage: " prefix for asymmetric passage retrieval
        let prefixed: Vec<String> = texts
            .iter()
            .map(|t| format!("passage: {}", t.trim()))
            .collect();
        let input: Vec<&str> = prefixed.iter().map(|s| s.as_str()).collect();

        self.model
            .embed(input, None)
            .map_err(|e| anyhow!("Failed to embed passages: {e}"))
    }

    fn embed_query(&mut self, text: &str) -> Result<Vec<f32>> {
        // E5 requires "query: " prefix for asymmetric query retrieval
        let prefixed = format!("query: {}", text.trim());
        let results = self
            .model
            .embed(vec![prefixed.as_str()], None)
            .map_err(|e| anyhow!("Failed to embed query: {e}"))?;

        results
            .into_iter()
            .next()
            .ok_or_else(|| anyhow!("Empty embedding result for query"))
    }

    fn dimension(&self) -> usize {
        self.dimension
    }

    fn model_id(&self) -> &str {
        &self.model_id
    }
}

/// Hermetic mock provider for reproducible offline tests.
pub struct MockDeterministicProvider {
    dimension: usize,
    model_id: String,
}

impl MockDeterministicProvider {
    pub fn new(dimension: usize) -> Self {
        Self {
            dimension,
            model_id: "mock/deterministic-e5-small".to_string(),
        }
    }

    /// Produce a deterministic pseudo-random unit vector based on input string and role prefix.
    fn vector_for(text: &str, prefix: &str, dimension: usize) -> Vec<f32> {
        let mut raw = Vec::with_capacity(dimension);
        let base_input = format!("{prefix}:{text}");

        let mut current_hash = {
            let mut hasher = Sha256::new();
            hasher.update(base_input.as_bytes());
            hasher.finalize()
        };

        for i in 0..dimension {
            if i > 0 && i % 32 == 0 {
                let mut hasher = Sha256::new();
                hasher.update(current_hash);
                hasher.update((i as u64).to_le_bytes());
                current_hash = hasher.finalize();
            }
            let byte = current_hash[i % 32];
            let val = ((byte as f32) / 127.5) - 1.0;
            raw.push(val);
        }

        // Normalize to unit length (L2 norm = 1.0)
        let norm_sq: f32 = raw.iter().map(|x| x * x).sum();
        let norm = norm_sq.sqrt().max(1e-9);
        raw.into_iter().map(|x| x / norm).collect()
    }
}

impl Default for MockDeterministicProvider {
    fn default() -> Self {
        Self::new(384)
    }
}

impl EmbeddingProvider for MockDeterministicProvider {
    fn embed_passages(&mut self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        Ok(texts
            .iter()
            .map(|t| Self::vector_for(t, "passage", self.dimension))
            .collect())
    }

    fn embed_query(&mut self, text: &str) -> Result<Vec<f32>> {
        Ok(Self::vector_for(text, "query", self.dimension))
    }

    fn dimension(&self) -> usize {
        self.dimension
    }

    fn model_id(&self) -> &str {
        &self.model_id
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mock_provider_deterministic_and_normalized() {
        let mut mock = MockDeterministicProvider::default();
        let p1 = mock.embed_passages(&["Architecture RecallEngine"]).unwrap();
        let p2 = mock.embed_passages(&["Architecture RecallEngine"]).unwrap();

        assert_eq!(p1, p2);
        assert_eq!(p1[0].len(), 384);

        // Check unit normalization
        let norm: f32 = p1[0].iter().map(|x| x * x).sum();
        assert!((norm - 1.0).abs() < 1e-4);

        let q = mock.embed_query("Architecture").unwrap();
        assert_eq!(q.len(), 384);
    }
}
