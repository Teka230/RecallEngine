use crate::error::Result;
use crate::search::snippet::parse_snippet;
use crate::search::{CountMode, SearchMatch, SearchQuery};
use rusqlite::{Connection, Row};

pub struct Fts5SearchRepository<'a> {
    pub connection: &'a Connection,
}

impl<'a> Fts5SearchRepository<'a> {
    pub fn new(connection: &'a Connection) -> Self {
        Self { connection }
    }

    pub fn search(
        &self,
        query: &SearchQuery,
        limit: u32,
    ) -> Result<(Vec<SearchMatch>, Option<u64>)> {
        let (where_clause, params) = self.build_where_clause(query);

        let sql = format!(
            "
WITH fts_matches AS (
    SELECT
        m.ic,
        m.id AS message_id,
        cb.id AS content_block_id,
        c.id AS conversation_id,
        c.title AS conversation_title,
        m.role,
        m.create_time,
        snippet(
            content_blocks_fts,
            0,
            char(1),
            char(2),
            '…',
            64
        ) AS raw_snippet,
        bm25(content_blocks_fts) AS rank,
        cb.ordinal,
        cb.rowid
    FROM content_blocks_fts
    JOIN content_blocks cb
        ON content_blocks_fts.rowid = cb.rowid
    JOIN messages m
        ON cb.message_id = m.id
    JOIN conversations c
        ON m.conversation_id = c.id
    WHERE {}
),
ranked_matches AS (
    SELECT
        *,
        ROW_NUMBER() OVER (
            PARTITION BY message_id
            ORDER BY
                rank ASC,
                ordinal ASC,
                rowid ASC
        ) AS message_match_order
    FROM fts_matches
)
SELECT *
FROM ranked_matches
WHERE message_match_order = 1
ORDER BY
    rank ASC,
    create_time ASC,
    message_id ASC
LIMIT ? OFFSET ?;
            ",
            where_clause
        );

        let mut stmt = self.connection.prepare(&sql)?;

        // Prepare params for bindings
        let mut bind_params: Vec<&dyn rusqlite::ToSql> = Vec::new();
        // first bind ? in WHERE
        for p in &params {
            bind_params.push(p.as_ref());
        }

        let limit_param = limit as i64;
        let offset_param = query.offset as i64;
        bind_params.push(&limit_param);
        bind_params.push(&offset_param);

        let mut rows = stmt.query(rusqlite::params_from_iter(bind_params))?;

        let mut results = Vec::new();
        while let Some(row) = rows.next()? {
            results.push(self.row_to_search_match(row)?);
        }

        let mut exact_total = None;
        if query.count_mode == CountMode::Exact {
            let count_sql = format!(
                "
SELECT COUNT(DISTINCT m.id)
FROM content_blocks_fts
JOIN content_blocks cb ON content_blocks_fts.rowid = cb.rowid
JOIN messages m ON cb.message_id = m.id
WHERE {}
                ",
                where_clause
            );

            let mut count_stmt = self.connection.prepare(&count_sql)?;
            let mut count_bind_params: Vec<&dyn rusqlite::ToSql> = Vec::new();
            for p in &params {
                count_bind_params.push(p.as_ref());
            }
            let total: i64 = count_stmt
                .query_row(rusqlite::params_from_iter(count_bind_params), |r| r.get(0))?;
            exact_total = Some(total as u64);
        }

        Ok((results, exact_total))
    }

    fn build_where_clause(&self, query: &SearchQuery) -> (String, Vec<Box<dyn rusqlite::ToSql>>) {
        let mut clauses = vec!["content_blocks_fts MATCH ?".to_string()];
        let mut params: Vec<Box<dyn rusqlite::ToSql>> = vec![Box::new(query.text.clone())];

        if let Some(role) = &query.role {
            clauses.push("m.role = ?".to_string());
            let role_str = match role {
                crate::search::SearchRole::User => "user",
                crate::search::SearchRole::Assistant => "assistant",
                crate::search::SearchRole::System => "system",
                crate::search::SearchRole::Tool => "tool",
            };
            params.push(Box::new(role_str.to_string()));
        }

        if let Some(ic_min) = query.ic_min {
            clauses.push("m.ic >= ?".to_string());
            params.push(Box::new(ic_min));
        }
        if let Some(ic_max) = query.ic_max {
            clauses.push("m.ic <= ?".to_string());
            params.push(Box::new(ic_max));
        }

        if let Some(date_min) = query.date_min {
            clauses.push("m.create_time >= ?".to_string());
            params.push(Box::new(date_min));
        }
        if let Some(date_max) = query.date_max {
            clauses.push("m.create_time < ?".to_string());
            params.push(Box::new(date_max));
        }

        (clauses.join(" AND "), params)
    }

    fn row_to_search_match(&self, row: &Row) -> Result<SearchMatch> {
        let ic: Option<i64> = row.get("ic")?;
        let message_id: String = row.get("message_id")?;
        let content_block_id: Option<String> = row.get("content_block_id")?;
        let conversation_id: String = row.get("conversation_id")?;
        let conversation_title: Option<String> = row.get("conversation_title")?;
        let role: String = row.get("role")?;
        let created_at: Option<f64> = row.get("create_time")?;

        let raw_snippet: String = row.get("raw_snippet")?;
        let snippet = parse_snippet(&raw_snippet);

        let rank: f64 = row.get("rank")?;

        Ok(SearchMatch {
            ic,
            message_id,
            content_block_id,
            conversation_id,
            conversation_title,
            role,
            created_at,
            snippet,
            rank,
        })
    }
}
