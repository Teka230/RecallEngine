use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use crate::cli::AssetMode;

use serde_json::json;
use tracing::{info, warn};

use crate::domain::canonical::ParsedFragment;
use crate::domain::ic::{existing_ic_map, max_ic_in_db, message_count, prepare_ic_batch};
use crate::domain::projections::{count_active, count_all};
use crate::error::{RecallError, Result};
use crate::import::assets::build_asset_id;
use crate::import::chatgpt::{
    collect_asset_links, load_asset_mapping, parse_fragment, parse_fragment_metadata, AssetLink,
};
use crate::import::discovery::discover_export;
use crate::import::legacy_ic::seed_legacy_ic_map;
use crate::import::manifest::{load_manifest, validate_manifest_sizes};
use crate::import::validation::sha256_file;
use crate::import::ExportLayout;
use crate::storage::sqlite::{
    AssetUpsert, Database, FeedbackUpsert, ImportIssue, LibraryFileUpsert, SharedUpsert,
};
use crate::storage::{CountableTable, SidecarTable};

struct AssetImportOptions<'a> {
    mode: AssetMode,
    directory: &'a std::path::Path,
}

pub fn run_chatgpt_import(
    source: PathBuf,
    db_path: PathBuf,
    assets: AssetMode,
    assets_dir: Option<PathBuf>,
    strict: bool,
    seed_legacy_ic: Option<PathBuf>,
) -> Result<()> {
    let layout = discover_export(&source, strict)?;
    let source_root = layout.root.to_string_lossy().to_string();

    let resolved_assets_dir = if let Some(dir) = assets_dir {
        dir
    } else {
        db_path
            .parent()
            .map(|p| p.join("assets"))
            .unwrap_or_else(|| PathBuf::from("assets"))
    };

    if assets == AssetMode::Copy || assets == AssetMode::Symlink {
        fs::create_dir_all(&resolved_assets_dir)?;
    }

    let mut db = Database::open(&db_path)?;
    let run_id = db.create_import_run(&source_root, strict)?;

    let mut issues: Vec<ImportIssue> = Vec::new();
    let legacy_seed = if let Some(p) = seed_legacy_ic {
        Some(seed_legacy_ic_map(&p)?)
    } else {
        None
    };

    if let Ok(Some(manifest)) = load_manifest(&layout, strict) {
        for w in validate_manifest_sizes(&manifest, &layout) {
            issues.push(ImportIssue {
                severity: "warning",
                code: "MANIFEST_SIZE_MISMATCH",
                entity_type: None,
                entity_id: None,
                source_relative_path: Some("export_manifest.json".to_string()),
                message: w,
            });
        }
    } else if strict {
        return Err(RecallError::msg("manifest required in strict mode"));
    }

    let is_first_import = message_count(db.connection())? == 0;
    let mut fragments_to_parse: Vec<(PathBuf, i32, String, i64)> = Vec::new();
    let mut skipped_shards: Vec<String> = Vec::new();
    let mut any_fragment_failed = false;

    for (idx, path) in layout.conversation_paths.iter().enumerate() {
        let rel = layout.relative(path);
        let (size, hash) = sha256_file(path)?;
        if let Some(prev) = db.last_completed_hash(&source_root, &rel)? {
            if prev == hash {
                db.record_source_file(
                    &run_id,
                    &rel,
                    "conversations_shard",
                    size,
                    &hash,
                    "skipped",
                )?;
                skipped_shards.push(rel);
                continue;
            }
        }
        db.record_source_file(&run_id, &rel, "conversations_shard", size, &hash, "seen")?;
        fragments_to_parse.push((path.clone(), (idx + 1) as i32, hash, size));
    }

    // Pass 1 deliberately retains only the fields required to assign ICs. Do
    // not replace this with a Vec<ParsedFragment>: exports may contain many
    // large shards and the writing pass below parses just one at a time.
    let mut all_candidates = Vec::new();
    let mut planned_fragments: Vec<(PathBuf, i32, String, i64)> = Vec::new();
    for (path, shard_index, hash, size) in &fragments_to_parse {
        let rel = layout.relative(path);
        match parse_fragment_metadata(&layout, path, *shard_index) {
            Ok(metadata) => {
                all_candidates.extend(
                    metadata.messages.into_iter().map(|message| {
                        message.into_message_candidate(metadata.relative_path.clone())
                    }),
                );
                planned_fragments.push((path.clone(), *shard_index, hash.clone(), *size));
            }
            Err(e) => {
                any_fragment_failed = true;
                db.record_source_file(&run_id, &rel, "conversations_shard", *size, hash, "failed")?;
                issues.push(ImportIssue {
                    severity: "error",
                    code: "FRAGMENT_PARSE_ERROR",
                    entity_type: Some("fragment".to_string()),
                    entity_id: None,
                    source_relative_path: Some(rel),
                    message: e.to_string(),
                });
            }
        }
    }

    if !planned_fragments.is_empty() && planned_fragments.len() < fragments_to_parse.len() {
        issues.push(ImportIssue {
            severity: "warning",
            code: "FRAGMENT_PARTIAL_PARSE",
            entity_type: None,
            entity_id: None,
            source_relative_path: None,
            message: format!(
                "parsed {} of {} conversation shards",
                planned_fragments.len(),
                fragments_to_parse.len()
            ),
        });
    }

    let needs_atomic = is_first_import || !planned_fragments.is_empty();
    let existing = existing_ic_map(db.connection())?;
    let max_ic = max_ic_in_db(db.connection())?;

    let ic_batch = prepare_ic_batch(&all_candidates, &existing, legacy_seed.as_ref(), max_ic);

    if ic_batch.legacy_ic_conflicts > 0 {
        issues.push(ImportIssue {
            severity: "warning",
            code: "LEGACY_IC_CONFLICT",
            entity_type: None,
            entity_id: None,
            source_relative_path: None,
            message: format!(
                "{} legacy IC conflicts detected",
                ic_batch.legacy_ic_conflicts
            ),
        });
    }

    let mut canonical_committed = false;
    let mut asset_mapping = if let Some(p) = &layout.asset_mapping_path {
        load_asset_mapping(p)?
    } else {
        HashMap::new()
    };
    merge_local_asset_files(&layout.root, &mut asset_mapping)?;

    if needs_atomic && !any_fragment_failed {
        let conn = db.connection();
        conn.execute("BEGIN IMMEDIATE", [])?;
        let mut ok = true;
        let mut write_err = None;
        if let Err(e) = import_asset_catalog(
            &db,
            &run_id,
            &layout,
            &asset_mapping,
            AssetImportOptions {
                mode: assets,
                directory: &resolved_assets_dir,
            },
            &mut issues,
        ) {
            ok = false;
            write_err = Some(e);
        }
        for (path, shard_index, _hash, _size) in &planned_fragments {
            if !ok {
                break;
            }
            let rel = layout.relative(path);
            let frag = match parse_fragment(&layout, path, *shard_index) {
                Ok(fragment) => fragment,
                Err(e) => {
                    any_fragment_failed = true;
                    issues.push(ImportIssue {
                        severity: "error",
                        code: "FRAGMENT_PARSE_ERROR",
                        entity_type: Some("fragment".to_string()),
                        entity_id: None,
                        source_relative_path: Some(rel),
                        message: e.to_string(),
                    });
                    write_err = Some(e);
                    ok = false;
                    break;
                }
            };
            let sp = format!("sp_{}", frag.shard_index);
            // Trusted savepoint name: shard_index is an internal integer, not user input.
            let _ = conn.execute_batch(&format!("SAVEPOINT {sp};"));
            if let Err(e) = write_fragment(&db, &run_id, &frag, &ic_batch.assignments)
                .and_then(|_| {
                    write_fragment_asset_links(&db, &run_id, &frag, &asset_mapping, &mut issues)
                })
                .and_then(|_| db.reconcile_fragment(&run_id, &frag.relative_path))
            {
                warn!("Error writing fragment {}: {:?}", frag.shard_index, e);
                write_err = Some(e);
                let _ = conn.execute_batch(&format!("ROLLBACK TO {sp};"));
                ok = false;
                any_fragment_failed = true;
                break;
            }
            let _ = conn.execute_batch(&format!("RELEASE {sp};"));
        }
        if ok {
            conn.execute("COMMIT", [])?;
            canonical_committed = true;
            for (path, _shard_index, hash, size) in &planned_fragments {
                db.record_source_file(
                    &run_id,
                    &layout.relative(path),
                    "conversations_shard",
                    *size,
                    hash,
                    "imported",
                )?;
            }
        } else {
            if let Err(rollback_err) = conn.execute("ROLLBACK", []) {
                warn!(
                    "explicit ROLLBACK failed (transaction may have already aborted): {:?}",
                    rollback_err
                );
            }
            let message = if let Some(ref e) = write_err {
                format!("canonical write rolled back due to fragment failure: {}", e)
            } else {
                "canonical write rolled back due to fragment failure".to_string()
            };
            issues.push(ImportIssue {
                severity: "error",
                code: "CANONICAL_ROLLBACK",
                entity_type: None,
                entity_id: None,
                source_relative_path: None,
                message,
            });
        }
    } else if needs_atomic && any_fragment_failed && is_first_import {
        info!("first import partial — canonical data rolled back");
    }

    if canonical_committed {
        import_sidecars(&mut db, &run_id, &layout, &mut issues)?;
    }

    for issue in &issues {
        db.insert_issue(&run_id, issue)?;
    }

    let status = if any_fragment_failed {
        "partial"
    } else {
        "completed"
    };

    let stats = compute_stats(&db, &ic_batch, canonical_committed)?;
    if canonical_committed {
        record_acceptance_divergences(&db, &run_id, &stats)?;
    }
    let stats_json = serde_json::to_string(&stats)?;
    db.finish_import_run(&run_id, status, Some(&stats_json), None)?;

    let reports_dir = Database::reports_dir(&db_path);
    fs::create_dir_all(&reports_dir)?;
    let report_path = reports_dir.join(format!("import-{run_id}.json"));
    fs::write(&report_path, &stats_json)?;

    info!("import {status}: {report_path:?}");
    if status == "partial" {
        warn!("import completed with errors — see import_issues");
    }

    Ok(())
}

fn merge_local_asset_files(
    root: &std::path::Path,
    mapping: &mut HashMap<String, String>,
) -> Result<()> {
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if name.starts_with("file-") || name.starts_with("file_") {
            mapping
                .entry(name.to_string())
                .or_insert_with(|| name.to_string());
        }
    }
    Ok(())
}

fn write_fragment(
    db: &Database,
    run_id: &str,
    frag: &ParsedFragment,
    ic_map: &HashMap<String, i64>,
) -> Result<()> {
    for c in &frag.conversations {
        db.upsert_conversation(run_id, c)?;
    }
    for n in &frag.nodes {
        db.upsert_node(run_id, n)?;
    }
    for m in &frag.messages {
        let ic = ic_map
            .get(&m.id)
            .copied()
            .ok_or_else(|| RecallError::msg(format!("missing IC for message {}", m.id)))?;
        db.upsert_message(run_id, m, ic)?;
    }
    for b in &frag.content_blocks {
        db.upsert_content_block(b)?;
    }
    for r in &frag.content_references {
        db.upsert_content_reference(r)?;
    }
    Ok(())
}

/// Persist the asset catalogue once per import. Asset links are intentionally
/// handled by `write_fragment_asset_links` immediately after each fragment so
/// no full parsed export has to remain alive.
fn import_asset_catalog(
    db: &Database,
    run_id: &str,
    layout: &ExportLayout,
    mapping: &HashMap<String, String>,
    asset_options: AssetImportOptions<'_>,
    issues: &mut Vec<ImportIssue>,
) -> Result<()> {
    let export_root = &layout.root;

    for (source_key, display_name) in mapping {
        let rel = source_key.clone();
        let path = export_root.join(&rel);
        let exists = path.exists();
        if !exists {
            issues.push(ImportIssue {
                severity: "warning",
                code: "MISSING_ASSET_FILE",
                entity_type: Some("asset".to_string()),
                entity_id: Some(source_key.clone()),
                source_relative_path: Some(rel.clone()),
                message: format!("asset file not found: {rel}"),
            });
        }

        let dest_path = asset_options.directory.join(&rel);
        if exists
            && (asset_options.mode == AssetMode::Copy || asset_options.mode == AssetMode::Symlink)
        {
            if let Some(parent) = dest_path.parent() {
                let _ = fs::create_dir_all(parent);
            }
            match asset_options.mode {
                AssetMode::Copy => {
                    let mut need_copy = true;
                    if dest_path.exists() {
                        if let (Ok(src_meta), Ok(dst_meta)) =
                            (path.metadata(), dest_path.metadata())
                        {
                            if src_meta.len() == dst_meta.len()
                                && src_meta.modified().ok() == dst_meta.modified().ok()
                            {
                                need_copy = false;
                            }
                        }
                    }
                    if need_copy {
                        if let Err(e) = fs::copy(&path, &dest_path) {
                            issues.push(ImportIssue {
                                severity: "warning",
                                code: "ASSET_COPY_ERROR",
                                entity_type: Some("asset".to_string()),
                                entity_id: Some(source_key.clone()),
                                source_relative_path: Some(rel.clone()),
                                message: format!(
                                    "failed to copy asset file {rel} to {dest_path:?}: {e}"
                                ),
                            });
                        }
                    }
                }
                AssetMode::Symlink => {
                    if dest_path.exists() || dest_path.symlink_metadata().is_ok() {
                        let _ = fs::remove_file(&dest_path);
                    }
                    #[cfg(unix)]
                    let symlink_res = std::os::unix::fs::symlink(&path, &dest_path);
                    #[cfg(windows)]
                    let symlink_res = std::os::windows::fs::symlink_file(&path, &dest_path);

                    if let Err(e) = symlink_res {
                        issues.push(ImportIssue {
                            severity: "warning",
                            code: "ASSET_SYMLINK_ERROR",
                            entity_type: Some("asset".to_string()),
                            entity_id: Some(source_key.clone()),
                            source_relative_path: Some(rel.clone()),
                            message: format!(
                                "failed to symlink asset file {rel} to {dest_path:?}: {e}"
                            ),
                        });
                    }
                }
                _ => {}
            }
        }

        let exists_locally = match asset_options.mode {
            AssetMode::External => exists,
            AssetMode::Copy | AssetMode::Symlink => dest_path.exists(),
        };

        let raw = json!({"source_key": source_key, "display_name": display_name}).to_string();
        db.upsert_asset(AssetUpsert {
            run_id,
            id: &build_asset_id(source_key),
            source_key,
            display_name: Some(display_name.as_str()),
            relative_path: Some(&rel),
            mime_type: None,
            size_bytes: path.metadata().ok().map(|m| m.len() as i64),
            exists_locally,
            raw_json: &raw,
        })?;
    }

    Ok(())
}

fn write_fragment_asset_links(
    db: &Database,
    run_id: &str,
    fragment: &ParsedFragment,
    mapping: &HashMap<String, String>,
    issues: &mut Vec<ImportIssue>,
) -> Result<()> {
    let (links, unresolved) = collect_asset_links(&fragment.messages, mapping);
    for unresolved_link in unresolved {
        let unresolved_key = format!("unresolved:{}", unresolved_link.raw_key);
        let asset_id = build_asset_id(&unresolved_key);
        db.upsert_asset(AssetUpsert {
            run_id,
            id: &asset_id,
            source_key: &unresolved_key,
            display_name: None,
            relative_path: None,
            mime_type: None,
            size_bytes: None,
            exists_locally: false,
            raw_json: &unresolved_link.raw_json,
        })?;
        let link_source = format!("{}_unresolved", unresolved_link.link_source);
        db.upsert_message_asset(
            &unresolved_link.message_id,
            &asset_id,
            &link_source,
            unresolved_link.ordinal,
            &unresolved_link.raw_json,
        )?;
        issues.push(ImportIssue {
            severity: "warning",
            code: "ASSET_KEY_UNRESOLVED",
            entity_type: Some("message".to_string()),
            entity_id: Some(unresolved_link.message_id),
            source_relative_path: None,
            message: format!("could not resolve asset key: {}", unresolved_link.raw_key),
        });
    }
    write_asset_links(db, &links)?;

    Ok(())
}

fn import_sidecars(
    db: &mut Database,
    run_id: &str,
    layout: &ExportLayout,
    issues: &mut Vec<ImportIssue>,
) -> Result<()> {
    import_feedback(db, run_id, layout, issues)?;
    import_shared(db, run_id, layout, issues)?;
    import_library(db, run_id, layout, issues)?;

    Ok(())
}

fn write_asset_links(db: &Database, links: &[AssetLink]) -> Result<()> {
    for link in links {
        db.upsert_message_asset(
            &link.message_id,
            &link.asset_id,
            &link.link_source,
            link.ordinal,
            &link.raw_json,
        )?;
    }
    Ok(())
}

fn import_feedback(
    db: &mut Database,
    run_id: &str,
    layout: &ExportLayout,
    _issues: &mut Vec<ImportIssue>,
) -> Result<()> {
    let Some(path) = &layout.feedback_path else {
        return Ok(());
    };
    let rel = layout.relative(path);
    let raw = fs::read_to_string(path)?;
    let items: Vec<serde_json::Value> = serde_json::from_str(&raw).unwrap_or_default();
    for (i, item) in items.iter().enumerate() {
        let id = item
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or(&format!("feedback-{i}"))
            .to_string();
        db.upsert_feedback(FeedbackUpsert {
            run_id,
            id: &id,
            message_id: item.get("message_id").and_then(|v| v.as_str()),
            rating: item.get("rating").and_then(|v| v.as_str()),
            tags: item.get("tags").map(|v| v.to_string()).as_deref(),
            text: item.get("text").and_then(|v| v.as_str()),
            created_at: item.get("created_at").and_then(|v| v.as_str()),
            raw_json: &item.to_string(),
        })?;
    }
    db.reconcile_sidecar(run_id, &rel, SidecarTable::Feedback)?;
    Ok(())
}

fn import_shared(
    db: &mut Database,
    run_id: &str,
    layout: &ExportLayout,
    _issues: &mut Vec<ImportIssue>,
) -> Result<()> {
    let Some(path) = &layout.shared_path else {
        return Ok(());
    };
    let rel = layout.relative(path);
    let raw = fs::read_to_string(path)?;
    let items: Vec<serde_json::Value> = serde_json::from_str(&raw).unwrap_or_default();
    for (i, item) in items.iter().enumerate() {
        let id = item
            .get("id")
            .or_else(|| item.get("share_id"))
            .and_then(|v| v.as_str())
            .unwrap_or(&format!("shared-{i}"))
            .to_string();
        db.upsert_shared(SharedUpsert {
            run_id,
            id: &id,
            conversation_id: item.get("conversation_id").and_then(|v| v.as_str()),
            share_id: item.get("share_id").and_then(|v| v.as_str()),
            url: item
                .get("url")
                .or_else(|| item.get("link"))
                .and_then(|v| v.as_str()),
            created_at: item.get("created_at").and_then(|v| v.as_str()),
            is_anonymous: i32::from(
                item.get("is_anonymous")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false),
            ),
            raw_json: &item.to_string(),
        })?;
    }
    db.reconcile_sidecar(run_id, &rel, SidecarTable::SharedConversations)?;
    Ok(())
}

fn import_library(
    db: &mut Database,
    run_id: &str,
    layout: &ExportLayout,
    _issues: &mut Vec<ImportIssue>,
) -> Result<()> {
    let Some(path) = &layout.library_path else {
        return Ok(());
    };
    let rel = layout.relative(path);
    let raw = fs::read_to_string(path)?;
    let items: Vec<serde_json::Value> = serde_json::from_str(&raw).unwrap_or_default();
    for (i, item) in items.iter().enumerate() {
        let id = item
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or(&format!("library-{i}"))
            .to_string();
        db.upsert_library_file(LibraryFileUpsert {
            run_id,
            id: &id,
            file_id: item.get("file_id").and_then(|v| v.as_str()),
            file_name: item.get("file_name").and_then(|v| v.as_str()),
            mime_type: item.get("mime_type").and_then(|v| v.as_str()),
            file_size_bytes: item.get("file_size_bytes").and_then(|v| v.as_i64()),
            sha256_digest: item.get("sha256_digest").and_then(|v| v.as_str()),
            raw_json: &item.to_string(),
        })?;
    }
    db.reconcile_sidecar(run_id, &rel, SidecarTable::LibraryFiles)?;
    Ok(())
}

fn compute_stats(
    db: &Database,
    ic_batch: &crate::domain::ic::IcBatchResult,
    committed: bool,
) -> Result<serde_json::Value> {
    let conn = db.connection();
    if !committed {
        return Ok(json!({
            "committed": false,
            "legacy_ic_matched": ic_batch.legacy_ic_matched,
            "legacy_ic_missing": ic_batch.legacy_ic_missing,
            "legacy_ic_conflicts": ic_batch.legacy_ic_conflicts,
        }));
    }

    let conversations = count_active(conn, CountableTable::Conversations)?;
    let nodes = count_active(conn, CountableTable::Nodes)?;
    let messages = count_active(conn, CountableTable::Messages)?;
    let assistant = count_role(conn, "assistant")?;
    let user = count_role(conn, "user")?;
    let branching = branching_conversations(conn)?;
    let branch_points = branch_points(conn)?;
    let max_children = max_children(conn)?;
    let mapped_assets = count_mapped_assets(conn)?;
    let unresolved_assets = count_unresolved_assets(conn)?;
    let attachments = count_message_assets(conn)?;
    let content_references = count_all(conn, CountableTable::ContentReferences)?;
    let feedback = count_active(conn, CountableTable::Feedback)?;
    let shared_conversations = count_active(conn, CountableTable::SharedConversations)?;
    let library_files = count_active(conn, CountableTable::LibraryFiles)?;

    Ok(json!({
        "committed": true,
        "conversations": conversations,
        "nodes": nodes,
        "messages": messages,
        "assistant_messages": assistant,
        "user_messages": user,
        "branching_conversations": branching,
        "branch_points": branch_points,
        "max_children": max_children,
        "mapped_assets": mapped_assets,
        "unresolved_assets": unresolved_assets,
        "attachments": attachments,
        "content_references": content_references,
        "feedback": feedback,
        "shared_conversations": shared_conversations,
        "library_files": library_files,
        "legacy_ic_matched": ic_batch.legacy_ic_matched,
        "legacy_ic_missing": ic_batch.legacy_ic_missing,
        "legacy_ic_conflicts": ic_batch.legacy_ic_conflicts,
    }))
}

fn count_role(conn: &rusqlite::Connection, role: &str) -> Result<i64> {
    Ok(conn.query_row(
        "SELECT COUNT(*) FROM messages WHERE is_active = 1 AND role = ?1",
        [role],
        |r| r.get(0),
    )?)
}

fn branching_conversations(conn: &rusqlite::Connection) -> Result<i64> {
    Ok(conn.query_row(
        "SELECT COUNT(DISTINCT conversation_id) FROM (
            SELECT conversation_id, parent_id, COUNT(*) c FROM nodes
            WHERE is_active = 1 AND parent_id IS NOT NULL
            GROUP BY conversation_id, parent_id HAVING c > 1
        )",
        [],
        |r| r.get(0),
    )?)
}

fn branch_points(conn: &rusqlite::Connection) -> Result<i64> {
    Ok(conn.query_row(
        "SELECT COUNT(*) FROM (
            SELECT conversation_id, parent_id
            FROM nodes
            WHERE is_active = 1 AND parent_id IS NOT NULL
            GROUP BY conversation_id, parent_id
            HAVING COUNT(*) > 1
        )",
        [],
        |r| r.get(0),
    )?)
}

fn max_children(conn: &rusqlite::Connection) -> Result<i64> {
    Ok(conn.query_row(
        "SELECT COALESCE(MAX(cnt),0) FROM (
            SELECT conversation_id, parent_id, COUNT(*) cnt FROM nodes
            WHERE is_active = 1 AND parent_id IS NOT NULL AND parent_id != 'client-created-root'
            GROUP BY conversation_id, parent_id
        )",
        [],
        |r| r.get(0),
    )?)
}

fn count_message_assets(conn: &rusqlite::Connection) -> Result<i64> {
    Ok(conn.query_row(
        "SELECT COUNT(*) FROM message_assets WHERE link_source LIKE 'metadata_attachment%'",
        [],
        |r| r.get(0),
    )?)
}

fn count_mapped_assets(conn: &rusqlite::Connection) -> Result<i64> {
    Ok(conn.query_row(
        "SELECT COUNT(*) FROM assets WHERE source_key LIKE 'chatgpt:%' AND source_key NOT LIKE 'chatgpt:unresolved:%'",
        [],
        |r| r.get(0),
    )?)
}

fn count_unresolved_assets(conn: &rusqlite::Connection) -> Result<i64> {
    Ok(conn.query_row(
        "SELECT COUNT(*) FROM assets WHERE source_key LIKE 'chatgpt:unresolved:%'",
        [],
        |r| r.get(0),
    )?)
}

fn record_acceptance_divergences(
    db: &Database,
    run_id: &str,
    stats: &serde_json::Value,
) -> Result<()> {
    let expected = [
        ("conversations", 2352_i64),
        ("nodes", 115719),
        ("messages", 113368),
        ("assistant_messages", 69116),
        ("user_messages", 44252),
        ("branching_conversations", 420),
        ("branch_points", 2218),
        ("max_children", 8),
        ("mapped_assets", 1844),
        ("attachments", 2307),
        ("content_references", 21462),
        ("feedback", 23),
        ("shared_conversations", 11),
        ("library_files", 57),
    ];
    for (key, exp) in expected {
        let actual = stats.get(key).and_then(|v| v.as_i64()).unwrap_or(0);
        if actual != exp {
            db.insert_issue(
                run_id,
                &ImportIssue {
                    severity: "info",
                    code: "ACCEPTANCE_DIVERGENCE",
                    entity_type: Some("stat".to_string()),
                    entity_id: Some(key.to_string()),
                    source_relative_path: None,
                    message: format!("expected {key}={exp}, got {actual}"),
                },
            )?;
        }
    }
    Ok(())
}
