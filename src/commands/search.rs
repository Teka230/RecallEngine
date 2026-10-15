#![allow(clippy::too_many_arguments)]
use crate::error::{RecallError, Result};
use crate::output::json::JsonEnvelope;
use crate::repository::search::Fts5SearchRepository;
use crate::search::service::SearchService;
use crate::search::{CountMode, SearchMode, SearchQuery, SearchRole, SearchSyntax};
use crate::semantic::embedding::{is_e5_small_cached, EmbeddingProvider, FastEmbedProvider};
use crate::semantic::hybrid::HybridSearchService;
use crate::semantic::store::ensure_sqlite_vec_registered;
use rusqlite::Connection;
use std::path::PathBuf;

pub fn run_search(
    db_path: PathBuf,
    query_text: String,
    syntax: SearchSyntax,
    count_mode: CountMode,
    role: Option<SearchRole>,
    ic_min: Option<i64>,
    ic_max: Option<i64>,
    date_min: Option<f64>,
    date_max: Option<f64>,
    limit: u32,
    offset: u32,
    mode: SearchMode,
    json: bool,
) -> Result<()> {
    match run_search_inner(
        &db_path, query_text, syntax, count_mode, role, ic_min, ic_max, date_min, date_max, limit,
        offset, mode,
    ) {
        Ok(search_result) => {
            if json {
                let env = JsonEnvelope::new(search_result);
                let json_str = serde_json::to_string(&env).unwrap_or_else(|_| "{}".to_string());
                println!("{}", json_str);
            } else {
                // Text formatting for standard console output
                if search_result.results.is_empty() {
                    println!("No results found for '{}'.", search_result.query);
                } else {
                    println!(
                        "Found results for '{}' (mode: {}):",
                        search_result.query, search_result.mode
                    );
                    for match_res in &search_result.results {
                        println!("- Message ID: {}", match_res.message_id);
                        if let Some(ic) = match_res.ic {
                            println!("  IC: {}", ic);
                        }
                        println!("  Role: {}", match_res.role);
                        println!("  Snippet: {}", match_res.snippet.text);
                        println!("  Rank: {}", match_res.rank);
                        println!();
                    }
                    if search_result.has_more {
                        println!("...and more results.");
                    }
                }
            }
            Ok(())
        }
        Err(e) => {
            if json {
                // In a real app we'd map this to a proper JSON error envelope.
                eprintln!("{}", e);
            }
            Err(e)
        }
    }
}

fn run_search_inner(
    db_path: &PathBuf,
    query_text: String,
    syntax: SearchSyntax,
    count_mode: CountMode,
    role: Option<SearchRole>,
    ic_min: Option<i64>,
    ic_max: Option<i64>,
    date_min: Option<f64>,
    date_max: Option<f64>,
    limit: u32,
    offset: u32,
    mode: SearchMode,
) -> Result<crate::search::SearchResult> {
    ensure_sqlite_vec_registered();
    let conn = Connection::open(db_path)?;

    let query = SearchQuery {
        text: query_text,
        syntax,
        mode,
        limit,
        offset,
        count_mode,
        role,
        ic_min,
        ic_max,
        date_min,
        date_max,
    };

    match mode {
        SearchMode::Lexical => {
            let repo = Fts5SearchRepository::new(&conn);
            let service = SearchService::new(repo);
            service.search(query)
        }
        SearchMode::Hybrid => {
            let hybrid_service = HybridSearchService::new(&conn);
            let mut embedder = if is_e5_small_cached(None) {
                FastEmbedProvider::try_new_offline(None).ok()
            } else {
                None
            };
            Ok(hybrid_service.search(
                embedder.as_mut().map(|e| e as &mut dyn EmbeddingProvider),
                &query,
            )?)
        }
        SearchMode::Semantic => {
            let hybrid_service = HybridSearchService::new(&conn);
            if !hybrid_service.is_vector_index_available() {
                return Err(RecallError::msg(
                    "Semantic search index not found or empty. Run 'recall embed' first.",
                ));
            }
            if !is_e5_small_cached(None) {
                return Err(RecallError::msg(
                    "Model cache for 'intfloat/multilingual-e5-small' not found. Run 'recall embed' to set up.",
                ));
            }
            let mut embedder = FastEmbedProvider::try_new_offline(None)?;
            let query_vector = embedder.embed_query(&query.text)?;
            let candidates = hybrid_service.fetch_vector_candidates(
                &query_vector,
                (limit + offset) as usize,
                &query,
            )?;
            let total_count = candidates.len();
            let paged: Vec<crate::search::SearchMatch> = candidates
                .into_iter()
                .skip(offset as usize)
                .take(limit as usize)
                .enumerate()
                .map(|(i, c)| crate::search::SearchMatch {
                    ic: c.ic,
                    message_id: c.message_id,
                    content_block_id: Some(c.content_block_id),
                    conversation_id: c.conversation_id,
                    conversation_title: c.conversation_title,
                    role: c.role,
                    created_at: c.created_at,
                    snippet: crate::search::snippet::parse_snippet(&c.text),
                    rank: (i + 1) as f64,
                })
                .collect();

            let (total, total_is_exact) = match query.count_mode {
                CountMode::Exact => (Some(total_count as u64), true),
                CountMode::None => (None, false),
            };

            Ok(crate::search::SearchResult {
                query: query.text,
                mode: "semantic".to_string(),
                total,
                total_is_exact,
                has_more: (offset as usize + limit as usize) < total_count,
                limit: query.limit,
                offset: query.offset,
                results: paged,
            })
        }
    }
}
