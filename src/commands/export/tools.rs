use std::collections::BTreeMap;
use std::path::PathBuf;

use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::Result;
use crate::storage::Database;

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolStats {
    pub calls: u64,
    pub results: u64,
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolsOutput {
    pub total_calls: u64,
    pub total_results: u64,
    pub tools: BTreeMap<String, ToolStats>,
}

/// Aggregate tool call / result counts from `messages.raw_json`.
///
/// Read-only: does not write the database or contact the network.
pub fn collect_tools(conn: &Connection) -> Result<ToolsOutput> {
    let mut stmt = conn.prepare("SELECT raw_json FROM messages")?;
    let mut rows = stmt.query([])?;

    let mut output = ToolsOutput::default();

    while let Some(row) = rows.next()? {
        let raw_json: String = row.get(0)?;
        if let Ok(msg) = serde_json::from_str::<Value>(&raw_json) {
            accumulate_message(&mut output, &msg);
        }
    }

    Ok(output)
}

fn accumulate_message(output: &mut ToolsOutput, msg: &Value) {
    let role = msg.get("role").and_then(|v| v.as_str()).unwrap_or("");
    let recipient = msg.get("recipient").and_then(|v| v.as_str()).unwrap_or("");

    // Extract calls
    let mut calls_found = 0;
    if let Some(tool_calls) = msg.get("tool_calls").and_then(|v| v.as_array()) {
        for call in tool_calls {
            let tool_name = call
                .get("function")
                .and_then(|f| f.get("name"))
                .and_then(|n| n.as_str())
                .or_else(|| call.get("type").and_then(|t| t.as_str()))
                .unwrap_or(recipient);
            let tool_name = tool_name.to_string();
            if !tool_name.is_empty() && tool_name != "all" {
                output.tools.entry(tool_name).or_default().calls += 1;
                calls_found += 1;
            }
        }
    }
    if calls_found == 0 && role == "assistant" && !recipient.is_empty() && recipient != "all" {
        output.tools.entry(recipient.to_string()).or_default().calls += 1;
        calls_found += 1;
    }
    if calls_found == 0 && role == "assistant" {
        if let Some(content) = msg
            .get("content")
            .and_then(|c| c.get("parts"))
            .and_then(|p| p.as_array())
        {
            for part in content {
                if let Some(fc) = part.get("function_call").or_else(|| part.get("tool_call")) {
                    let tool_name = fc.get("name").and_then(|n| n.as_str()).unwrap_or(recipient);
                    if !tool_name.is_empty() && tool_name != "all" {
                        output.tools.entry(tool_name.to_string()).or_default().calls += 1;
                        calls_found += 1;
                    }
                }
            }
        }
    }
    output.total_calls += calls_found as u64;

    // Extract results
    if role == "tool" || role == "system" {
        let name_from_content = msg
            .get("content")
            .and_then(|c| c.get("name"))
            .and_then(|n| n.as_str());
        let name_from_author = msg
            .get("author")
            .and_then(|a| a.get("name"))
            .and_then(|n| n.as_str());
        let name_from_domain = msg
            .get("content")
            .and_then(|c| c.get("domain"))
            .and_then(|n| n.as_str());
        let name_from_msg = msg.get("name").and_then(|n| n.as_str());

        let name = name_from_content
            .or_else(|| {
                if !recipient.is_empty() && recipient != "all" {
                    Some(recipient)
                } else {
                    None
                }
            })
            .or(name_from_author)
            .or(name_from_domain)
            .or(name_from_msg)
            .unwrap_or("");

        // System messages are only tool results if they have a clear tool signature.
        // We assume it's a tool result if we actually resolved a name (that is not "all" or empty).
        if !name.is_empty() && name != "all" {
            output.tools.entry(name.to_string()).or_default().results += 1;
            output.total_results += 1;
        }
    }
}

pub fn run(db_path: PathBuf) -> Result<()> {
    let db = Database::open(&db_path)?;
    let output = collect_tools(db.connection())?;
    println!("{}", serde_json::to_string_pretty(&output)?);
    Ok(())
}
