#![allow(clippy::too_many_arguments)]
use crate::error::Result;
use crate::output::json::JsonEnvelope;
use crate::repository::search::Fts5SearchRepository;
use crate::search::service::SearchService;
use crate::search::{CountMode, SearchQuery, SearchRole, SearchSyntax};
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
    json: bool,
) -> Result<()> {
    match run_search_inner(
        &db_path, query_text, syntax, count_mode, role, ic_min, ic_max, date_min, date_max, limit,
        offset,
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
                    println!("Found results for '{}':", search_result.query);
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
) -> Result<crate::search::SearchResult> {
    let conn = Connection::open(db_path)?;
    let repo = Fts5SearchRepository::new(&conn);
    let service = SearchService::new(repo);

    let query = SearchQuery {
        text: query_text,
        syntax,
        limit,
        offset,
        count_mode,
        role,
        ic_min,
        ic_max,
        date_min,
        date_max,
    };

    service.search(query)
}
