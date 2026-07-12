use std::collections::{HashMap, HashSet};
use std::path::Path;

use rusqlite::Connection;

use crate::domain::canonical::MessageCandidate;
use crate::error::Result;

const NULL_TIMESTAMP_SENTINEL: &str = "9999-12-31T23:59:59Z";

#[derive(Debug, Clone, Default)]
pub struct LegacyIcSeed {
    pub map: HashMap<String, i64>,
    pub max_ic: i64,
}

#[derive(Debug, Clone, Default)]
pub struct IcBatchResult {
    pub assignments: HashMap<String, i64>,
    pub legacy_ic_matched: u64,
    pub legacy_ic_missing: u64,
    pub legacy_ic_conflicts: u64,
}

/// Load message.id → IC from a legacy ExploGPT/GPTExtractor SQLite database.
pub fn seed_legacy_ic_map(path: &Path) -> Result<LegacyIcSeed> {
    let conn = Connection::open(path)?;
    let mut map = HashMap::new();
    let mut max_ic = 0i64;
    let mut stmt = conn.prepare("SELECT id, IC FROM messages WHERE id IS NOT NULL")?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
    })?;
    for row in rows {
        let (id, ic) = row?;
        max_ic = max_ic.max(ic);
        map.insert(id, ic);
    }
    Ok(LegacyIcSeed { map, max_ic })
}

/// Sort key aligned with GPTExtractor compute_global_ic.
fn sort_key(msg: &MessageCandidate) -> (bool, String, String, f64, i32, i32, String) {
    let ts_null = msg.timestamp.is_none();
    let ts = msg
        .timestamp
        .clone()
        .unwrap_or_else(|| NULL_TIMESTAMP_SENTINEL.to_string());
    let conv = msg.conversation_id.clone();
    let create_raw = msg.create_time_raw.unwrap_or(f64::INFINITY);
    (
        ts_null,
        ts,
        conv,
        create_raw,
        msg.source_shard_index,
        msg.source_node_order,
        msg.id.clone(),
    )
}

/// Prepare IC assignments for new messages; preserve existing IC from DB.
pub fn prepare_ic_batch(
    candidates: &[MessageCandidate],
    existing_ic: &HashMap<String, i64>,
    legacy: Option<&LegacyIcSeed>,
    max_ic_in_db: i64,
) -> IcBatchResult {
    let mut result = IcBatchResult::default();
    let mut used_ics: HashSet<i64> = existing_ic.values().copied().collect();
    if let Some(leg) = legacy {
        let export_ids: HashSet<_> = candidates.iter().map(|c| c.id.clone()).collect();
        result.legacy_ic_missing = leg
            .map
            .keys()
            .filter(|id| !export_ids.contains(*id))
            .count() as u64;
    }

    let mut next_ic = max_ic_in_db;
    if let Some(leg) = legacy {
        next_ic = next_ic.max(leg.max_ic);
    }

    let mut new_messages: Vec<&MessageCandidate> = candidates
        .iter()
        .filter(|c| !existing_ic.contains_key(&c.id))
        .collect();
    new_messages.sort_by(|a, b| {
        sort_key(a)
            .partial_cmp(&sort_key(b))
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    for msg in candidates {
        if let Some(&ic) = existing_ic.get(&msg.id) {
            result.assignments.insert(msg.id.clone(), ic);
        }
    }

    for msg in new_messages {
        if let Some(leg) = legacy {
            if let Some(&ic) = leg.map.get(&msg.id) {
                if !used_ics.contains(&ic) {
                    result.legacy_ic_matched += 1;
                    used_ics.insert(ic);
                    result.assignments.insert(msg.id.clone(), ic);
                    next_ic = next_ic.max(ic);
                    continue;
                }
                result.legacy_ic_conflicts += 1;
            }
        }
        next_ic += 1;
        while used_ics.contains(&next_ic) {
            next_ic += 1;
        }
        used_ics.insert(next_ic);
        result.assignments.insert(msg.id.clone(), next_ic);
    }

    result
}

pub fn resolve_ic_for_message(
    message_id: &str,
    existing_ic: &HashMap<String, i64>,
    batch: &IcBatchResult,
) -> Option<i64> {
    existing_ic
        .get(message_id)
        .copied()
        .or_else(|| batch.assignments.get(message_id).copied())
}

pub fn max_ic_in_db(conn: &Connection) -> Result<i64> {
    let v: i64 = conn.query_row("SELECT COALESCE(MAX(ic), 0) FROM messages", [], |r| {
        r.get(0)
    })?;
    Ok(v)
}

pub fn existing_ic_map(conn: &Connection) -> Result<HashMap<String, i64>> {
    let mut map = HashMap::new();
    let mut stmt = conn.prepare("SELECT id, ic FROM messages")?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
    })?;
    for row in rows {
        let (id, ic) = row?;
        map.insert(id, ic);
    }
    Ok(map)
}

pub fn message_count(conn: &Connection) -> Result<i64> {
    let c: i64 = conn.query_row("SELECT COUNT(*) FROM messages", [], |r| r.get(0))?;
    Ok(c)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::canonical::MessageCandidate;

    fn candidate(id: &str, ts: &str, shard: i32, order: i32) -> MessageCandidate {
        MessageCandidate {
            id: id.to_string(),
            node_id: format!("node-{id}"),
            conversation_id: "conv-1".to_string(),
            role: Some("user".to_string()),
            author_name: None,
            create_time: Some(1.0),
            create_time_raw: Some(1.0),
            timestamp: Some(ts.to_string()),
            source_shard_index: shard,
            source_node_order: order,
            model_slug: None,
            content_type: Some("text".to_string()),
            source_relative_path: "conversations-000.json".to_string(),
            raw_json: "{}".to_string(),
            content: serde_json::json!({}),
            metadata: serde_json::json!({}),
        }
    }

    #[test]
    fn first_import_assigns_contiguous_ic() {
        let msgs = vec![
            candidate("m2", "2024-01-02T00:00:00Z", 1, 1),
            candidate("m1", "2024-01-01T00:00:00Z", 1, 0),
        ];
        let batch = prepare_ic_batch(&msgs, &HashMap::new(), None, 0);
        assert_eq!(batch.assignments.get("m1"), Some(&1));
        assert_eq!(batch.assignments.get("m2"), Some(&2));
    }

    #[test]
    fn reimport_preserves_existing_ic() {
        let msgs = vec![candidate("m1", "2024-01-01T00:00:00Z", 1, 0)];
        let mut existing = HashMap::new();
        existing.insert("m1".to_string(), 42);
        let batch = prepare_ic_batch(&msgs, &existing, None, 42);
        assert_eq!(batch.assignments.get("m1"), Some(&42));
    }

    #[test]
    fn legacy_seed_reuses_ic() {
        let msgs = vec![candidate("m1", "2024-01-01T00:00:00Z", 1, 0)];
        let legacy = LegacyIcSeed {
            map: HashMap::from([("m1".to_string(), 100)]),
            max_ic: 100,
        };
        let batch = prepare_ic_batch(&msgs, &HashMap::new(), Some(&legacy), 0);
        assert_eq!(batch.assignments.get("m1"), Some(&100));
        assert_eq!(batch.legacy_ic_matched, 1);
    }
}
