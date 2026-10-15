use crate::repository::search::Fts5SearchRepository;
use crate::search::service::escape_fts5_simple;
use crate::search::snippet::parse_snippet;
use crate::search::{CountMode, SearchMatch, SearchQuery, SearchResult, SearchRole, SearchSyntax};
use crate::semantic::embedding::EmbeddingProvider;
use crate::semantic::store::f32_slice_to_bytes;
use anyhow::Result;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Parameters for Reciprocal Rank Fusion (RRF).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct RrfParams {
    /// Smoothing constant (default: 60.0). Prevents top ranks from dominating disproportionately.
    pub k: f64,
    /// Weight multiplier for FTS5 BM25 matches (default: 1.0).
    pub fts_weight: f64,
    /// Weight multiplier for semantic vector KNN matches (default: 1.0).
    pub vec_weight: f64,
}

impl Default for RrfParams {
    fn default() -> Self {
        Self {
            k: 60.0,
            fts_weight: 1.0,
            vec_weight: 1.0,
        }
    }
}

impl RrfParams {
    /// Calculate individual component score: weight / (k + rank).
    pub fn score_component(&self, rank: usize, weight: f64) -> f64 {
        weight / (self.k + rank as f64)
    }
}

/// Enriched intermediate representation of a hybrid search match.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HybridMatch {
    pub message_id: String,
    pub content_block_id: Option<String>,
    pub conversation_id: String,
    pub conversation_title: Option<String>,
    pub ic: Option<i64>,
    pub role: String,
    pub created_at: Option<f64>,
    pub snippet_text: String,
    pub rrf_score: f64,
    pub fts_rank: Option<usize>,
    pub vec_rank: Option<usize>,
    pub vec_distance: Option<f32>,
}

impl HybridMatch {
    /// Convert to canonical SearchMatch compatible with existing readers and APIs.
    pub fn into_search_match(self) -> SearchMatch {
        SearchMatch {
            ic: self.ic,
            message_id: self.message_id,
            content_block_id: self.content_block_id,
            conversation_id: self.conversation_id,
            conversation_title: self.conversation_title,
            role: self.role,
            created_at: self.created_at,
            snippet: parse_snippet(&self.snippet_text),
            rank: self.rrf_score,
        }
    }
}

/// Raw candidate retrieved from the vector virtual table with canonical metadata.
#[derive(Debug, Clone)]
pub struct VectorCandidate {
    pub chunk_id: i64,
    pub content_block_id: String,
    pub message_id: String,
    pub ic: Option<i64>,
    pub conversation_id: String,
    pub conversation_title: Option<String>,
    pub role: String,
    pub created_at: Option<f64>,
    pub text: String,
    pub distance: f32,
}

/// Hybrid search service orchestrating FTS5 lexical retrieval and vector KNN retrieval with RRF fusion.
pub struct HybridSearchService<'a> {
    conn: &'a Connection,
    rrf_params: RrfParams,
}

impl<'a> HybridSearchService<'a> {
    pub fn new(conn: &'a Connection) -> Self {
        Self {
            conn,
            rrf_params: RrfParams::default(),
        }
    }

    pub fn with_params(conn: &'a Connection, rrf_params: RrfParams) -> Self {
        Self { conn, rrf_params }
    }

    /// Check if the semantic vector tables exist and contain at least one indexed chunk.
    pub fn is_vector_index_available(&self) -> bool {
        let has_table: rusqlite::Result<bool> = self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'semantic_vec_chunks')",
            [],
            |r| r.get(0),
        );
        if !has_table.unwrap_or(false) {
            return false;
        }

        let count: rusqlite::Result<i64> =
            self.conn
                .query_row("SELECT COUNT(*) FROM semantic_vec_chunks", [], |r| r.get(0));
        count.unwrap_or(0) > 0
    }

    /// Execute a hybrid search query fusing FTS5 and vector retrieval via RRF.
    ///
    /// If embedder is None or vector tables are unavailable, automatically falls back to pure FTS5.
    pub fn search(
        &self,
        embedder: Option<&mut dyn EmbeddingProvider>,
        query: &SearchQuery,
    ) -> Result<SearchResult> {
        // If vector index is not ready or no embedder provided, graceful fallback to FTS5
        if !self.is_vector_index_available() || embedder.is_none() {
            let mut fts_query = query.clone();
            if fts_query.syntax == SearchSyntax::Simple {
                fts_query.text = escape_fts5_simple(&fts_query.text);
            }
            let repo = Fts5SearchRepository::new(self.conn);
            let (results, exact_total) = repo.search(&fts_query, query.limit + 1)?;
            let has_more = results.len() > query.limit as usize;
            let mut final_results = results;
            if has_more {
                final_results.pop();
            }
            let (total, total_is_exact) = match query.count_mode {
                CountMode::Exact => (
                    Some(exact_total.unwrap_or(final_results.len() as u64)),
                    true,
                ),
                CountMode::None => (None, false),
            };
            return Ok(SearchResult {
                query: query.text.clone(),
                mode: "hybrid-fallback-fts5".to_string(),
                total,
                total_is_exact,
                has_more,
                limit: query.limit,
                offset: query.offset,
                results: final_results,
            });
        }

        let embedder = embedder.unwrap();

        // 1. Fetch FTS5 candidates
        let candidate_limit = (query.limit + query.offset).saturating_mul(3).max(50);
        let mut fts_query = query.clone();
        if fts_query.syntax == SearchSyntax::Simple {
            fts_query.text = escape_fts5_simple(&fts_query.text);
        }
        let fts_repo = Fts5SearchRepository::new(self.conn);
        let (fts_matches, _) = fts_repo.search(&fts_query, candidate_limit)?;

        // 2. Fetch Vector candidates
        let query_vector = embedder.embed_query(&query.text)?;
        let vec_matches =
            self.fetch_vector_candidates(&query_vector, candidate_limit as usize, query)?;

        // 3. Perform Reciprocal Rank Fusion
        let fused = self.fuse_candidates(&fts_matches, &vec_matches);
        let total_count = fused.len();

        // 4. Pagination
        let offset = query.offset as usize;
        let limit = query.limit as usize;
        let paged_matches: Vec<SearchMatch> = fused
            .into_iter()
            .skip(offset)
            .take(limit)
            .map(|h| h.into_search_match())
            .collect();

        let has_more = offset + limit < total_count;
        let (total, total_is_exact) = match query.count_mode {
            CountMode::Exact => (Some(total_count as u64), true),
            CountMode::None => (None, false),
        };

        Ok(SearchResult {
            query: query.text.clone(),
            mode: "hybrid".to_string(),
            total,
            total_is_exact,
            has_more,
            limit: query.limit,
            offset: query.offset,
            results: paged_matches,
        })
    }

    /// Retrieve raw vector candidates joined with message and conversation metadata.
    pub fn fetch_vector_candidates(
        &self,
        query_vector: &[f32],
        k: usize,
        query: &SearchQuery,
    ) -> Result<Vec<VectorCandidate>> {
        let query_bytes = f32_slice_to_bytes(query_vector);
        let mut stmt = self.conn.prepare(
            "SELECT
                c.chunk_id,
                c.content_block_id,
                c.message_id,
                m.ic,
                m.conversation_id,
                conv.title,
                m.role,
                m.create_time,
                c.text,
                v.distance
            FROM semantic_vec_items v
            JOIN semantic_vec_chunks c ON v.chunk_id = c.chunk_id
            JOIN messages m ON c.message_id = m.id
            JOIN conversations conv ON m.conversation_id = conv.id
            WHERE v.embedding MATCH ?1 AND k = ?2
              AND m.is_active = 1
              AND conv.is_active = 1
            ORDER BY v.distance ASC",
        )?;

        let role_filter = query.role.map(|r| match r {
            SearchRole::User => "user",
            SearchRole::Assistant => "assistant",
            SearchRole::System => "system",
            SearchRole::Tool => "tool",
        });

        let rows = stmt.query_map(params![query_bytes, k as i64], |row| {
            let distance: f32 = row.get(9)?;
            Ok(VectorCandidate {
                chunk_id: row.get(0)?,
                content_block_id: row.get(1)?,
                message_id: row.get(2)?,
                ic: row.get(3)?,
                conversation_id: row.get(4)?,
                conversation_title: row.get(5)?,
                role: row.get(6)?,
                created_at: row.get(7)?,
                text: row.get(8)?,
                distance,
            })
        })?;

        let mut filtered = Vec::new();
        for r in rows {
            let candidate = r?;

            if let Some(rf) = role_filter {
                if candidate.role != rf {
                    continue;
                }
            }
            if let Some(ic_min) = query.ic_min {
                if candidate.ic.map(|v| v < ic_min).unwrap_or(true) {
                    continue;
                }
            }
            if let Some(ic_max) = query.ic_max {
                if candidate.ic.map(|v| v > ic_max).unwrap_or(true) {
                    continue;
                }
            }
            if let Some(date_min) = query.date_min {
                if candidate.created_at.map(|v| v < date_min).unwrap_or(true) {
                    continue;
                }
            }
            if let Some(date_max) = query.date_max {
                if candidate.created_at.map(|v| v > date_max).unwrap_or(true) {
                    continue;
                }
            }

            filtered.push(candidate);
        }

        Ok(filtered)
    }

    /// Fuse candidate lists from FTS5 and Vector retrieval using Reciprocal Rank Fusion (RRF).
    pub fn fuse_candidates(
        &self,
        fts_matches: &[SearchMatch],
        vec_candidates: &[VectorCandidate],
    ) -> Vec<HybridMatch> {
        let mut map: HashMap<String, HybridMatch> = HashMap::new();

        // 1. Process FTS5 matches (already ranked 1..N by BM25)
        for (i, fts) in fts_matches.iter().enumerate() {
            let rank = i + 1;
            let score = self
                .rrf_params
                .score_component(rank, self.rrf_params.fts_weight);

            let entry = map
                .entry(fts.message_id.clone())
                .or_insert_with(|| HybridMatch {
                    message_id: fts.message_id.clone(),
                    content_block_id: fts.content_block_id.clone(),
                    conversation_id: fts.conversation_id.clone(),
                    conversation_title: fts.conversation_title.clone(),
                    ic: fts.ic,
                    role: fts.role.clone(),
                    created_at: fts.created_at,
                    snippet_text: fts.snippet.text.clone(),
                    rrf_score: 0.0,
                    fts_rank: None,
                    vec_rank: None,
                    vec_distance: None,
                });

            entry.fts_rank = Some(rank);
            entry.rrf_score += score;
        }

        // 2. Process Vector candidates (group by message_id, retaining best chunk distance)
        let mut best_by_message: HashMap<String, VectorCandidate> = HashMap::new();
        let mut message_order = Vec::new();

        for cand in vec_candidates {
            if let Some(existing) = best_by_message.get_mut(&cand.message_id) {
                if cand.distance < existing.distance {
                    *existing = cand.clone();
                }
            } else {
                best_by_message.insert(cand.message_id.clone(), cand.clone());
                message_order.push(cand.message_id.clone());
            }
        }

        for (i, msg_id) in message_order.iter().enumerate() {
            let rank = i + 1;
            let cand = &best_by_message[msg_id];
            let score = self
                .rrf_params
                .score_component(rank, self.rrf_params.vec_weight);

            let entry = map.entry(msg_id.clone()).or_insert_with(|| HybridMatch {
                message_id: cand.message_id.clone(),
                content_block_id: Some(cand.content_block_id.clone()),
                conversation_id: cand.conversation_id.clone(),
                conversation_title: cand.conversation_title.clone(),
                ic: cand.ic,
                role: cand.role.clone(),
                created_at: cand.created_at,
                snippet_text: cand.text.clone(),
                rrf_score: 0.0,
                fts_rank: None,
                vec_rank: None,
                vec_distance: None,
            });

            entry.vec_rank = Some(rank);
            entry.vec_distance = Some(cand.distance);
            entry.rrf_score += score;
        }

        // 3. Sort by: rrf_score DESC, created_at ASC, message_id ASC
        let mut results: Vec<HybridMatch> = map.into_values().collect();
        results.sort_by(|a, b| {
            b.rrf_score
                .partial_cmp(&a.rrf_score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| {
                    let ca = a.created_at.unwrap_or(0.0);
                    let cb = b.created_at.unwrap_or(0.0);
                    ca.partial_cmp(&cb).unwrap_or(std::cmp::Ordering::Equal)
                })
                .then_with(|| a.message_id.cmp(&b.message_id))
        });

        results
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::search::SearchSnippet;

    #[test]
    fn test_rrf_scoring_and_weights() {
        let params = RrfParams {
            k: 60.0,
            fts_weight: 1.0,
            vec_weight: 1.0,
        };

        // Rank 1 score = 1.0 / (60.0 + 1.0) = 1/61
        let score1 = params.score_component(1, 1.0);
        assert!((score1 - (1.0 / 61.0)).abs() < 1e-6);

        // Rank 2 score = 1.0 / (60.0 + 2.0) = 1/62
        let score2 = params.score_component(2, 1.0);
        assert!((score2 - (1.0 / 62.0)).abs() < 1e-6);

        assert!(score1 > score2);
    }

    #[test]
    fn test_fuse_candidates_boosts_dual_matches() {
        let conn = Connection::open_in_memory().unwrap();
        let service = HybridSearchService::new(&conn);

        let fts_matches = vec![
            SearchMatch {
                ic: Some(1),
                message_id: "msg-both".into(),
                content_block_id: Some("cb-1".into()),
                conversation_id: "conv-1".into(),
                conversation_title: Some("Title 1".into()),
                role: "assistant".into(),
                created_at: Some(100.0),
                snippet: SearchSnippet {
                    text: "Snippet 1".into(),
                    segments: vec![],
                },
                rank: -2.5,
            },
            SearchMatch {
                ic: Some(2),
                message_id: "msg-fts-only".into(),
                content_block_id: Some("cb-2".into()),
                conversation_id: "conv-1".into(),
                conversation_title: Some("Title 1".into()),
                role: "user".into(),
                created_at: Some(105.0),
                snippet: SearchSnippet {
                    text: "Snippet 2".into(),
                    segments: vec![],
                },
                rank: -2.1,
            },
        ];

        let vec_candidates = vec![
            VectorCandidate {
                chunk_id: 10,
                content_block_id: "cb-3".into(),
                message_id: "msg-vec-only".into(),
                ic: Some(3),
                conversation_id: "conv-2".into(),
                conversation_title: Some("Title 2".into()),
                role: "assistant".into(),
                created_at: Some(110.0),
                text: "Vector chunk text".into(),
                distance: 0.15,
            },
            VectorCandidate {
                chunk_id: 11,
                content_block_id: "cb-1".into(),
                message_id: "msg-both".into(),
                ic: Some(1),
                conversation_id: "conv-1".into(),
                conversation_title: Some("Title 1".into()),
                role: "assistant".into(),
                created_at: Some(100.0),
                text: "Vector chunk text for both".into(),
                distance: 0.18,
            },
        ];

        let fused = service.fuse_candidates(&fts_matches, &vec_candidates);

        assert_eq!(fused.len(), 3);
        // "msg-both" appears in both FTS5 (rank 1) and Vector (rank 2) -> its score is 1/61 + 1/62 = 0.0325
        assert_eq!(fused[0].message_id, "msg-both");
        assert_eq!(fused[0].fts_rank, Some(1));
        assert_eq!(fused[0].vec_rank, Some(2));
        assert!(fused[0].rrf_score > fused[1].rrf_score);
        assert!(fused[0].rrf_score > fused[2].rrf_score);

        // Verify other candidates are preserved
        let remaining_ids: Vec<&str> = fused[1..].iter().map(|m| m.message_id.as_str()).collect();
        assert!(remaining_ids.contains(&"msg-fts-only"));
        assert!(remaining_ids.contains(&"msg-vec-only"));
    }
}
