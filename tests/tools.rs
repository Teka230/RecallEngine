use recall_engine::commands::export::tools::{collect_tools, ToolStats, ToolsOutput};
use recall_engine::storage::Database;
use std::collections::BTreeMap;
use std::path::Path;

/// Seed a minimal FK chain plus known `messages.raw_json` rows for tool stats.
fn seed_known_tools_db(db_path: &Path) {
    let db = Database::open(db_path).unwrap();
    let run_id = db.create_import_run("tools-fixture", false).unwrap();
    let conn = db.connection();

    conn.execute(
        "INSERT INTO conversations
         (id, title, create_time, update_time, default_model_slug, source_relative_path,
          last_seen_import_run_id, is_active, raw_json)
         VALUES ('conv-tools', 'Tools fixture', NULL, NULL, NULL, 'tools.json', ?1, 1, '{}')",
        [&run_id],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO nodes
         (id, conversation_id, parent_id, has_message, source_relative_path,
          last_seen_import_run_id, is_active, raw_json)
         VALUES ('node-tools', 'conv-tools', NULL, 1, 'tools.json', ?1, 1, '{}')",
        [&run_id],
    )
    .unwrap();

    // Known counts:
    // - browser.search: 2 calls (tool_calls + content.parts function_call), 1 result (tool role)
    // - python: 1 call (assistant recipient fallback), 1 result (system + author.name)
    // - myfiles_browser: 1 call (tool_calls type fallback)
    // Totals: 4 calls, 2 results
    let messages: &[(&str, i64, &str)] = &[
        (
            "msg-call-browser-1",
            1,
            r#"{
              "role": "assistant",
              "tool_calls": [
                {"function": {"name": "browser.search", "arguments": "{}"}}
              ]
            }"#,
        ),
        (
            "msg-call-browser-2",
            2,
            r#"{
              "role": "assistant",
              "content": {
                "parts": [
                  {"function_call": {"name": "browser.search", "arguments": "{}"}}
                ]
              }
            }"#,
        ),
        (
            "msg-result-browser",
            3,
            r#"{
              "role": "tool",
              "name": "browser.search",
              "content": {"name": "browser.search", "result": "ok"}
            }"#,
        ),
        (
            "msg-call-python",
            4,
            r#"{
              "role": "assistant",
              "recipient": "python",
              "content": {"parts": ["print(1)"]}
            }"#,
        ),
        (
            "msg-result-python",
            5,
            r#"{
              "role": "system",
              "author": {"name": "python"},
              "content": {"result": "1"}
            }"#,
        ),
        (
            "msg-call-myfiles",
            6,
            r#"{
              "role": "assistant",
              "tool_calls": [
                {"type": "myfiles_browser"}
              ]
            }"#,
        ),
        (
            "msg-plain-assistant",
            7,
            r#"{
              "role": "assistant",
              "content": {"parts": ["no tools here"]}
            }"#,
        ),
        (
            "msg-system-noise",
            8,
            r#"{
              "role": "system",
              "content": {"parts": ["ignore me"]}
            }"#,
        ),
    ];

    for (id, ic, raw_json) in messages {
        conn.execute(
            "INSERT INTO messages
             (id, ic, node_id, conversation_id, role, author_name, create_time, create_time_raw,
              timestamp, source_shard_index, source_node_order, model_slug, content_type,
              source_relative_path, last_seen_import_run_id, is_active, raw_json)
             VALUES (?1, ?2, 'node-tools', 'conv-tools', NULL, NULL, NULL, NULL, NULL, 0, 0,
                     NULL, NULL, 'tools.json', ?3, 1, ?4)",
            rusqlite::params![id, ic, run_id, raw_json],
        )
        .unwrap();
    }
}

fn expected_known_tools_output() -> ToolsOutput {
    let mut tools = BTreeMap::new();
    tools.insert(
        "browser.search".to_string(),
        ToolStats {
            calls: 2,
            results: 1,
        },
    );
    tools.insert(
        "myfiles_browser".to_string(),
        ToolStats {
            calls: 1,
            results: 0,
        },
    );
    tools.insert(
        "python".to_string(),
        ToolStats {
            calls: 1,
            results: 1,
        },
    );
    ToolsOutput {
        total_calls: 4,
        total_results: 2,
        tools,
    }
}

#[test]
fn test_tools_export_on_empty_db() {
    let tmp = tempfile::tempdir().unwrap();
    let db = tmp.path().join("history.sqlite");
    Database::open(&db).unwrap();

    let mut cmd = std::process::Command::new(env!("CARGO_BIN_EXE_recall"));
    cmd.arg("tools").arg("--db").arg(&db);
    let output = cmd.output().unwrap();

    assert!(output.status.success());
    let parsed: ToolsOutput = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(parsed.total_calls, 0);
    assert_eq!(parsed.total_results, 0);
    assert!(parsed.tools.is_empty());
}

#[test]
fn test_collect_tools_known_counts() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("history.sqlite");
    seed_known_tools_db(&db_path);

    let db = Database::open(&db_path).unwrap();
    let parsed = collect_tools(db.connection()).unwrap();
    assert_eq!(parsed, expected_known_tools_output());
}

#[test]
fn test_tools_cli_known_counts_and_stable_json() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("history.sqlite");
    seed_known_tools_db(&db_path);

    let mut cmd = std::process::Command::new(env!("CARGO_BIN_EXE_recall"));
    cmd.arg("tools").arg("--db").arg(&db_path);
    let output = cmd.output().unwrap();

    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );

    let parsed: ToolsOutput = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(parsed, expected_known_tools_output());

    // Pretty JSON with BTreeMap keys must be deterministic across runs.
    let first = String::from_utf8(output.stdout.clone()).unwrap();
    let second = std::process::Command::new(env!("CARGO_BIN_EXE_recall"))
        .arg("tools")
        .arg("--db")
        .arg(&db_path)
        .output()
        .unwrap();
    assert!(second.status.success());
    assert_eq!(first, String::from_utf8(second.stdout).unwrap());

    // Keys appear in sorted order in the object.
    let browser_pos = first.find("\"browser.search\"").unwrap();
    let myfiles_pos = first.find("\"myfiles_browser\"").unwrap();
    let python_pos = first.find("\"python\"").unwrap();
    assert!(browser_pos < myfiles_pos && myfiles_pos < python_pos);
}
