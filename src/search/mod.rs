pub mod service;
pub mod snippet;

use clap::ValueEnum;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SearchRole {
    User,
    Assistant,
    System,
    Tool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SearchSyntax {
    Simple,
    Fts5,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CountMode {
    None,
    Exact,
}

#[derive(Debug, Clone)]
pub struct SearchQuery {
    pub text: String,
    pub syntax: SearchSyntax,
    pub limit: u32,
    pub offset: u32,
    pub count_mode: CountMode,
    pub role: Option<SearchRole>,
    pub ic_min: Option<i64>,
    pub ic_max: Option<i64>,
    pub date_min: Option<f64>,
    pub date_max: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchSnippet {
    pub text: String,
    pub segments: Vec<SnippetSegment>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnippetSegment {
    pub text: String,
    pub highlighted: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchMatch {
    pub ic: Option<i64>,
    pub message_id: String,
    pub content_block_id: Option<String>,
    pub conversation_id: String,
    pub conversation_title: Option<String>,
    pub role: String,
    pub created_at: Option<f64>,
    pub snippet: SearchSnippet,
    pub rank: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    pub query: String,
    pub mode: String,
    pub total: Option<u64>,
    pub total_is_exact: bool,
    pub has_more: bool,
    pub limit: u32,
    pub offset: u32,
    pub results: Vec<SearchMatch>,
}
