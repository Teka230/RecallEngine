use std::{fmt, str::FromStr};

use clap::ValueEnum;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::error::{RecallError, Result};
use crate::storage::TRUSTED_REFERENCE_ROLE_PREDICATE;

/// Roles eligible for public IC references. Other source roles remain in the
/// canonical DB but are not addressable through the IC reference scheme.
pub fn is_reference_role(role: Option<&str>) -> bool {
    matches!(
        role.map(|value| value.trim().to_ascii_lowercase())
            .as_deref(),
        Some("user") | Some("assistant")
    )
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, ValueEnum, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ContextScope {
    Conversation,
    Corpus,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ReferencedMessage {
    pub id: String,
    pub message_id: String,
    pub ic: i64,
    pub reference: String,
    pub conversation_id: String,
    pub conversation_title: String,
    pub role: String,
    pub content: String,
    pub created_at: Option<f64>,
    pub timestamp: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MessageReference {
    pub ic: i64,
    pub message_id: String,
}

impl MessageReference {
    pub fn new(ic: i64, message_id: impl Into<String>) -> Result<Self> {
        let message_id = message_id.into();
        if ic <= 0 {
            return Err(RecallError::msg("IC must be a positive integer"));
        }
        if message_id.trim().is_empty() {
            return Err(RecallError::msg("message ID must not be empty"));
        }
        Ok(Self { ic, message_id })
    }

    pub fn human(&self) -> String {
        format!("[IC:{} | msg:{}]", self.ic, self.message_id)
    }

    pub fn token(&self) -> String {
        format!(
            "ref:ic/{}/uuid/{}",
            self.ic,
            percent_encode(self.message_id.as_bytes())
        )
    }
}

impl fmt::Display for MessageReference {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.human())
    }
}

impl FromStr for MessageReference {
    type Err = RecallError;

    fn from_str(value: &str) -> Result<Self> {
        let value = value.trim();
        if let Some(inner) = value
            .strip_prefix("[IC:")
            .and_then(|rest| rest.strip_suffix(']'))
        {
            let (ic, message_id) = inner
                .split_once(" | msg:")
                .ok_or_else(|| RecallError::msg("invalid composite reference"))?;
            return Self::new(parse_ic(ic)?, message_id.to_owned());
        }
        if let Some(inner) = value.strip_prefix("ref:ic/") {
            let (ic, encoded_message_id) = inner
                .split_once("/uuid/")
                .ok_or_else(|| RecallError::msg("invalid composite reference token"))?;
            return Self::new(parse_ic(ic)?, percent_decode(encoded_message_id)?);
        }
        Err(RecallError::msg("invalid composite reference"))
    }
}

fn parse_ic(value: &str) -> Result<i64> {
    value
        .parse::<i64>()
        .map_err(|_| RecallError::msg("IC must be a positive integer"))
}

fn percent_encode(value: &[u8]) -> String {
    let mut encoded = String::new();
    for byte in value {
        if byte.is_ascii_alphanumeric() || matches!(*byte, b'-' | b'.' | b'_' | b'~') {
            encoded.push(char::from(*byte));
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

fn percent_decode(value: &str) -> Result<String> {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != b'%' {
            decoded.push(bytes[index]);
            index += 1;
            continue;
        }
        if index + 2 >= bytes.len() {
            return Err(RecallError::msg("invalid percent-encoded message ID"));
        }
        let hex = std::str::from_utf8(&bytes[index + 1..index + 3])
            .map_err(|_| RecallError::msg("invalid percent-encoded message ID"))?;
        decoded.push(
            u8::from_str_radix(hex, 16)
                .map_err(|_| RecallError::msg("invalid percent-encoded message ID"))?,
        );
        index += 3;
    }
    String::from_utf8(decoded).map_err(|_| RecallError::msg("message ID is not valid UTF-8"))
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct IcRange {
    pub min_ic: i64,
    pub max_ic: i64,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct IcContext {
    pub center_ic: i64,
    pub scope: ContextScope,
    pub before_requested: usize,
    pub after_requested: usize,
    pub messages: Vec<ReferencedMessage>,
    pub range: IcRange,
}

pub fn get_active_message_by_ic(conn: &Connection, ic: i64) -> Result<Option<ReferencedMessage>> {
    let mut statement = conn.prepare(&format!(
        "SELECT m.id, m.ic, m.conversation_id, COALESCE(c.title, 'Untitled'),
                COALESCE(m.role, 'unknown'),
                COALESCE((
                    SELECT GROUP_CONCAT(text_content, char(10))
                    FROM (
                        SELECT cb.text_content
                        FROM content_blocks cb
                        WHERE cb.message_id = m.id
                        ORDER BY cb.ordinal
                    ) ordered_blocks
                ), ''),
                m.create_time, m.timestamp
         FROM messages m
         JOIN conversations c ON c.id = m.conversation_id
         WHERE m.ic = ?1 AND m.is_active = 1 AND c.is_active = 1 AND {TRUSTED_REFERENCE_ROLE_PREDICATE}
         LIMIT 1"
    ))?;
    let row = statement
        .query_row([ic], map_referenced_message)
        .optional()?;
    Ok(row)
}

pub fn get_active_message_by_id(
    conn: &Connection,
    message_id: &str,
) -> Result<Option<ReferencedMessage>> {
    let mut statement = conn.prepare(&format!(
        "SELECT m.id, m.ic, m.conversation_id, COALESCE(c.title, 'Untitled'),
                COALESCE(m.role, 'unknown'),
                COALESCE((
                    SELECT GROUP_CONCAT(text_content, char(10))
                    FROM (
                        SELECT cb.text_content
                        FROM content_blocks cb
                        WHERE cb.message_id = m.id
                        ORDER BY cb.ordinal
                    ) ordered_blocks
                ), ''),
                m.create_time, m.timestamp
         FROM messages m
         JOIN conversations c ON c.id = m.conversation_id
         WHERE m.id = ?1 AND m.is_active = 1 AND c.is_active = 1 AND {TRUSTED_REFERENCE_ROLE_PREDICATE}
         LIMIT 1"
    ))?;
    Ok(statement
        .query_row([message_id], map_referenced_message)
        .optional()?)
}

pub fn resolve_message_reference(
    conn: &Connection,
    reference: &MessageReference,
) -> Result<Option<ReferencedMessage>> {
    let Some(message) = get_active_message_by_id(conn, &reference.message_id)? else {
        return Ok(None);
    };
    if message.ic != reference.ic {
        return Err(RecallError::msg(format!(
            "reference mismatch: IC {} does not identify message {}",
            reference.ic, reference.message_id
        )));
    }
    Ok(Some(message))
}

pub fn get_ic_context(
    conn: &Connection,
    center_ic: i64,
    before: usize,
    after: usize,
    scope: ContextScope,
) -> Result<Option<IcContext>> {
    let Some(center) = get_active_message_by_ic(conn, center_ic)? else {
        return Ok(None);
    };

    let conversation_id =
        (scope == ContextScope::Conversation).then_some(center.conversation_id.clone());
    let mut previous = fetch_neighbors(conn, center_ic, before, false, conversation_id.as_deref())?;
    previous.reverse();
    let following = fetch_neighbors(conn, center_ic, after, true, conversation_id.as_deref())?;

    let mut messages = Vec::with_capacity(previous.len() + 1 + following.len());
    messages.extend(previous);
    messages.push(center);
    messages.extend(following);

    let min_ic = messages
        .iter()
        .map(|message| message.ic)
        .min()
        .unwrap_or(center_ic);
    let max_ic = messages
        .iter()
        .map(|message| message.ic)
        .max()
        .unwrap_or(center_ic);
    Ok(Some(IcContext {
        center_ic,
        scope,
        before_requested: before,
        after_requested: after,
        messages,
        range: IcRange { min_ic, max_ic },
    }))
}

fn fetch_neighbors(
    conn: &Connection,
    center_ic: i64,
    limit: usize,
    after: bool,
    conversation_id: Option<&str>,
) -> Result<Vec<ReferencedMessage>> {
    if limit == 0 {
        return Ok(Vec::new());
    }
    let comparison = if after { ">" } else { "<" };
    let ordering = if after { "ASC" } else { "DESC" };
    let sql = format!(
        "SELECT m.id, m.ic, m.conversation_id, COALESCE(c.title, 'Untitled'),
                COALESCE(m.role, 'unknown'),
                COALESCE((
                    SELECT GROUP_CONCAT(text_content, char(10))
                    FROM (
                        SELECT cb.text_content
                        FROM content_blocks cb
                        WHERE cb.message_id = m.id
                        ORDER BY cb.ordinal
                    ) ordered_blocks
                ), ''),
                m.create_time, m.timestamp
         FROM messages m
         JOIN conversations c ON c.id = m.conversation_id
         WHERE m.ic {comparison} ?1 AND m.is_active = 1 AND c.is_active = 1
           AND {TRUSTED_REFERENCE_ROLE_PREDICATE}
           {conversation_filter}
         ORDER BY m.ic {ordering}
         LIMIT ?2",
        conversation_filter = if conversation_id.is_some() {
            "AND m.conversation_id = ?3"
        } else {
            ""
        }
    );
    let mut statement = conn.prepare(&sql)?;
    let limit = i64::try_from(limit).unwrap_or(i64::MAX);
    let rows = if let Some(conversation_id) = conversation_id {
        statement.query_map(
            params![center_ic, limit, conversation_id],
            map_referenced_message,
        )?
    } else {
        statement.query_map(params![center_ic, limit], map_referenced_message)?
    };
    Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
}

fn map_referenced_message(row: &rusqlite::Row<'_>) -> rusqlite::Result<ReferencedMessage> {
    let id: String = row.get(0)?;
    let ic: i64 = row.get(1)?;
    let reference = MessageReference {
        ic,
        message_id: id.clone(),
    }
    .human();
    Ok(ReferencedMessage {
        id: id.clone(),
        message_id: id,
        ic,
        reference,
        conversation_id: row.get(2)?,
        conversation_title: row.get(3)?,
        role: row.get(4)?,
        content: row.get(5)?,
        created_at: row.get(6)?,
        timestamp: row.get(7)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn connection() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE conversations (id TEXT PRIMARY KEY, title TEXT, is_active INTEGER);
             CREATE TABLE messages (
                 id TEXT PRIMARY KEY, ic INTEGER UNIQUE, conversation_id TEXT,
                 role TEXT, is_active INTEGER, create_time REAL, timestamp TEXT
             );
             CREATE TABLE content_blocks (
                 id TEXT PRIMARY KEY, message_id TEXT, ordinal INTEGER, text_content TEXT
             );
             INSERT INTO conversations VALUES ('c1', 'Conversation', 1);
             INSERT INTO conversations VALUES ('c2', 'Autre', 1);
             INSERT INTO messages VALUES ('m1', 1, 'c1', 'user', 1, 1.0, '2024-01-01');
             INSERT INTO messages VALUES ('m2', 2, 'c1', 'tool', 1, 2.0, '2024-01-02');
             INSERT INTO messages VALUES ('m3', 3, 'c1', 'assistant', 1, 3.0, '2024-01-03');
             INSERT INTO messages VALUES ('m4', 4, 'c2', 'assistant', 1, 4.0, '2024-01-04');
             INSERT INTO messages VALUES ('m5', 5, 'c1', 'assistant', 0, 5.0, '2024-01-05');
             INSERT INTO content_blocks VALUES ('b1', 'm1', 0, 'one');
             INSERT INTO content_blocks VALUES ('b2', 'm3', 0, 'three');
             INSERT INTO content_blocks VALUES ('b3', 'm4', 0, 'four');",
        )
        .unwrap();
        conn
    }

    #[test]
    fn only_user_and_assistant_are_reference_roles() {
        assert!(is_reference_role(Some("user")));
        assert!(is_reference_role(Some(" ASSISTANT ")));
        assert!(!is_reference_role(Some("tool")));
        assert!(!is_reference_role(None));
    }

    #[test]
    fn technical_and_inactive_messages_are_not_resolvable() {
        let conn = connection();
        assert!(get_active_message_by_ic(&conn, 2).unwrap().is_none());
        assert!(get_active_message_by_ic(&conn, 5).unwrap().is_none());
        assert!(get_active_message_by_id(&conn, "m2").unwrap().is_none());
        assert!(get_active_message_by_id(&conn, "m5").unwrap().is_none());
        assert_eq!(
            get_active_message_by_ic(&conn, 3).unwrap().unwrap().id,
            "m3"
        );
    }

    #[test]
    fn composite_reference_round_trips_human_and_machine_formats() {
        let reference = MessageReference::new(42, "message/opaque id").unwrap();
        assert_eq!(
            reference.human().parse::<MessageReference>().unwrap(),
            reference
        );
        assert_eq!(
            reference.token().parse::<MessageReference>().unwrap(),
            reference
        );
        assert_eq!(reference.token(), "ref:ic/42/uuid/message%2Fopaque%20id");
    }

    #[test]
    fn resolves_by_message_id_and_rejects_mismatched_pair() {
        let conn = connection();
        let message = get_active_message_by_id(&conn, "m3").unwrap().unwrap();
        assert_eq!(message.ic, 3);
        assert_eq!(message.message_id, "m3");
        assert_eq!(message.reference, "[IC:3 | msg:m3]");

        let matching = MessageReference::new(3, "m3").unwrap();
        assert!(resolve_message_reference(&conn, &matching)
            .unwrap()
            .is_some());
        let mismatch = MessageReference::new(1, "m3").unwrap();
        assert!(resolve_message_reference(&conn, &mismatch).is_err());
    }

    #[test]
    fn conversation_context_skips_technical_messages() {
        let conn = connection();
        let context = get_ic_context(&conn, 3, 2, 2, ContextScope::Conversation)
            .unwrap()
            .unwrap();
        assert_eq!(
            context
                .messages
                .iter()
                .map(|message| message.id.as_str())
                .collect::<Vec<_>>(),
            vec!["m1", "m3"]
        );
    }

    #[test]
    fn corpus_context_can_cross_conversations() {
        let conn = connection();
        let context = get_ic_context(&conn, 3, 2, 1, ContextScope::Corpus)
            .unwrap()
            .unwrap();
        assert_eq!(
            context
                .messages
                .iter()
                .map(|message| message.id.as_str())
                .collect::<Vec<_>>(),
            vec!["m1", "m3", "m4"]
        );
    }
}
