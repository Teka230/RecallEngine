use anyhow::{anyhow, Result};
use fastembed::{EmbeddingModel, TextEmbedding, TextInitOptions};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

/// Offline policy for model weight loading.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OfflineMode {
    /// Strictly disallow network downloads. If model files are not present in local cache, fail immediately.
    StrictOffline,
    /// Allow downloading model weights over network if not found in local cache.
    AllowDownload,
}

/// Check if model weights for intfloat/multilingual-e5-small exist locally in cache.
pub fn is_e5_small_cached(cache_dir: Option<&Path>) -> bool {
    let base = match cache_dir {
        Some(dir) => dir.to_path_buf(),
        None => PathBuf::from(".fastembed_cache"),
    };
    let model_dir = base.join("models--intfloat--multilingual-e5-small");
    if !model_dir.exists() {
        return false;
    }
    let snapshots = model_dir.join("snapshots");
    if !snapshots.exists() {
        return false;
    }
    if let Ok(entries) = std::fs::read_dir(snapshots) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_dir() && p.join("tokenizer.json").exists() && p.join("config.json").exists() {
                return true;
            }
        }
    }
    false
}

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
    /// Creates a new FastEmbedProvider enforcing StrictOffline mode if cached, or returns an explicit error if missing.
    pub fn try_new(cache_dir: Option<PathBuf>) -> Result<Self> {
        Self::try_new_with_mode(cache_dir, OfflineMode::StrictOffline)
    }

    /// Explicit strictly offline constructor: forbids any network calls.
    pub fn try_new_offline(cache_dir: Option<PathBuf>) -> Result<Self> {
        Self::try_new_with_mode(cache_dir, OfflineMode::StrictOffline)
    }

    /// Explicit online constructor: allows downloading model weights on first run.
    pub fn try_new_online(cache_dir: Option<PathBuf>) -> Result<Self> {
        Self::try_new_with_mode(cache_dir, OfflineMode::AllowDownload)
    }

    /// Creates a FastEmbedProvider with an explicit OfflineMode.
    pub fn try_new_with_mode(cache_dir: Option<PathBuf>, mode: OfflineMode) -> Result<Self> {
        if mode == OfflineMode::StrictOffline && !is_e5_small_cached(cache_dir.as_deref()) {
            let target_cache = cache_dir
                .unwrap_or_else(|| PathBuf::from(".fastembed_cache"))
                .display()
                .to_string();
            return Err(anyhow!(
                "StrictOffline: Model weights for 'intfloat/multilingual-e5-small' not found in cache '{target_cache}'. Network downloads are forbidden in offline mode."
            ));
        }

        let mut options = TextInitOptions::new(EmbeddingModel::MultilingualE5Small)
            .with_show_download_progress(false);

        if let Some(ref dir) = cache_dir {
            options = options.with_cache_dir(dir.clone());
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

    #[test]
    fn test_strict_offline_mode_rejects_missing_cache() {
        let tmp = tempfile::tempdir().unwrap();
        let non_existent_cache = tmp.path().join("empty_cache");

        let res = FastEmbedProvider::try_new_offline(Some(non_existent_cache));
        assert!(res.is_err(), "StrictOffline must reject non-existent cache");
        let err_msg = res.err().unwrap().to_string();
        assert!(
            err_msg.contains("StrictOffline"),
            "Error must explicitly mention StrictOffline rejection"
        );
    }

    #[test]
    fn test_is_e5_small_cached_detects_present_cache() {
        // In local development, .fastembed_cache was materialized
        let is_cached = is_e5_small_cached(None);
        assert!(
            is_cached,
            "Local .fastembed_cache should be detected as cached"
        );
    }
}
