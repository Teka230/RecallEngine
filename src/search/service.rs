use super::{CountMode, SearchQuery, SearchResult, SearchSyntax};
use crate::error::Result;
use crate::repository::search::Fts5SearchRepository;

pub struct SearchService<'a> {
    repo: Fts5SearchRepository<'a>,
}

impl<'a> SearchService<'a> {
    pub fn new(repo: Fts5SearchRepository<'a>) -> Self {
        Self { repo }
    }

    pub fn search(&self, mut query: SearchQuery) -> Result<SearchResult> {
        let original_query = query.text.clone();

        // Escape query if syntax is Simple
        if query.syntax == SearchSyntax::Simple {
            query.text = escape_fts5_simple(&query.text);
        }

        // Request limit + 1 to check for has_more
        let request_limit = query.limit + 1;

        let (mut results, exact_total) = self.repo.search(&query, request_limit)?;

        let has_more = results.len() > query.limit as usize;

        // Remove the extra item if we asked for limit + 1
        if has_more {
            results.pop();
        }

        let (total, total_is_exact) = match query.count_mode {
            CountMode::Exact => (Some(exact_total.unwrap_or(0)), true),
            CountMode::None => (None, false),
        };

        Ok(SearchResult {
            query: original_query,
            mode: match query.syntax {
                SearchSyntax::Simple => "fts5-simple".to_string(),
                SearchSyntax::Fts5 => "fts5".to_string(),
            },
            total,
            total_is_exact,
            has_more,
            limit: query.limit,
            offset: query.offset,
            results,
        })
    }
}

/// Escapes a simple query into a safe FTS5 MATCH query.
/// Example: `moteur de recherche` -> `"moteur" AND "de" AND "recherche"`
fn escape_fts5_simple(text: &str) -> String {
    let terms: Vec<String> = text
        .split_whitespace()
        .map(|term| {
            // Escape double quotes by doubling them, which is the SQLite FTS5 standard
            let escaped_term = term.replace("\"", "\"\"");
            format!("\"{}\"", escaped_term)
        })
        .collect();

    terms.join(" AND ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_escape_fts5_simple() {
        assert_eq!(escape_fts5_simple("hello world"), "\"hello\" AND \"world\"");
        assert_eq!(
            escape_fts5_simple("moteur de recherche"),
            "\"moteur\" AND \"de\" AND \"recherche\""
        );
        assert_eq!(
            escape_fts5_simple("avec \"guillemets\""),
            "\"avec\" AND \"\"\"guillemets\"\"\""
        );
    }
}
