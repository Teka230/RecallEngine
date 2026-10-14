use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::error::{RecallError, Result};
use tempfile::TempDir;
use walkdir::WalkDir;
use zip::ZipArchive;

#[derive(Debug, Clone)]
pub struct ExportLayout {
    pub root: PathBuf,
    pub source_root: PathBuf,
    pub conversation_paths: Vec<PathBuf>,
    pub manifest_path: Option<PathBuf>,
    pub asset_mapping_path: Option<PathBuf>,
    pub feedback_path: Option<PathBuf>,
    pub shared_path: Option<PathBuf>,
    pub library_path: Option<PathBuf>,
    // Keeps a materialized partitioned export alive for the duration of the
    // import without writing anything into the user's downloaded export.
    _staging: Option<Arc<TempDir>>,
}

impl ExportLayout {
    pub fn relative(&self, path: &Path) -> String {
        path.strip_prefix(&self.root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/")
    }
}

pub fn discover_export(root: &Path, strict: bool) -> Result<ExportLayout> {
    discover_export_with_assets(root, strict, false)
}

/// Discover a ChatGPT export. In addition to the historical extracted layout,
/// this accepts the newer export directory containing Conversations*.zip
/// partitions. Asset payloads are materialized only when requested by a copy
/// or symlink import.
pub fn discover_export_with_assets(
    root: &Path,
    strict: bool,
    include_assets: bool,
) -> Result<ExportLayout> {
    let canonical_root =
        fs::canonicalize(root).map_err(|e| RecallError::msg(format!("export not found: {e}")))?;
    let source_root = if canonical_root.is_file() {
        canonical_root
            .parent()
            .map(Path::to_path_buf)
            .ok_or_else(|| RecallError::msg("export has no parent directory"))?
    } else {
        canonical_root
    };

    if let Some(layout) = discover_in_root(&source_root, source_root.clone(), None, strict)? {
        return Ok(layout);
    }

    let Some((staging_root, staging)) =
        materialize_partitioned_export(&source_root, include_assets)?
    else {
        return Err(RecallError::msg(
            "no conversations.json or conversations-*.json found",
        ));
    };

    discover_in_root(&staging_root, source_root, Some(staging), strict)?
        .ok_or_else(|| RecallError::msg("partitioned export contains no conversation shards"))
}

fn discover_in_root(
    root: &Path,
    source_root: PathBuf,
    staging: Option<Arc<TempDir>>,
    strict: bool,
) -> Result<Option<ExportLayout>> {
    let mut shards: Vec<PathBuf> = glob_paths(root, "conversations-*.json")?;
    if shards.is_empty() {
        let single = root.join("conversations.json");
        if single.exists() {
            shards.push(single);
        }
    }
    if shards.is_empty() {
        return Ok(None);
    }
    shards.sort_by_key(|path| natural_key(path));

    let manifest_path = root.join("export_manifest.json");
    let has_manifest = manifest_path.exists();
    if strict && !has_manifest {
        return Err(RecallError::msg(
            "export_manifest.json required in --strict mode",
        ));
    }

    Ok(Some(ExportLayout {
        root: root.to_path_buf(),
        source_root,
        conversation_paths: shards,
        manifest_path: has_manifest.then_some(manifest_path),
        asset_mapping_path: optional_file(root, "conversation_asset_file_names.json"),
        feedback_path: optional_file(root, "message_feedback.json"),
        shared_path: optional_file(root, "shared_conversations.json"),
        library_path: optional_file(root, "library_files.json"),
        _staging: staging,
    }))
}

fn materialize_partitioned_export(
    source_root: &Path,
    include_assets: bool,
) -> Result<Option<(PathBuf, Arc<TempDir>)>> {
    let archives = partitioned_archive_paths(source_root)?;
    if archives.is_empty() {
        return Ok(None);
    }

    let staging = Arc::new(tempfile::tempdir()?);
    let mut conversation_shard_found = false;

    for archive_path in archives {
        let file = fs::File::open(&archive_path)?;
        let mut archive = ZipArchive::new(file).map_err(|error| {
            RecallError::msg(format!(
                "invalid partitioned export archive {:?}: {error}",
                archive_path
            ))
        })?;

        for index in 0..archive.len() {
            let mut member = archive.by_index(index).map_err(|error| {
                RecallError::msg(format!(
                    "cannot read partitioned export archive {:?}: {error}",
                    archive_path
                ))
            })?;
            if member.is_dir() {
                continue;
            }
            let Some(name) = safe_top_level_member_name(member.name()) else {
                continue;
            };
            if !is_partitioned_member(&name, include_assets) {
                continue;
            }

            let target = staging.path().join(&name);
            if target.exists() {
                return Err(RecallError::msg(format!(
                    "duplicate entry in partitioned export: {name}"
                )));
            }
            let mut destination = fs::File::create(&target)?;
            io::copy(&mut member, &mut destination)?;
            conversation_shard_found |= is_conversation_shard(&name);
        }
    }

    if !conversation_shard_found {
        return Ok(None);
    }
    Ok(Some((staging.path().to_path_buf(), staging)))
}

fn partitioned_archive_paths(root: &Path) -> Result<Vec<PathBuf>> {
    let mut archives = Vec::new();
    for entry in WalkDir::new(root).follow_links(false) {
        let entry = entry.map_err(|error| RecallError::msg(error.to_string()))?;
        if !entry.file_type().is_file() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
        if name.starts_with("conversations") && name.ends_with(".zip") {
            archives.push(entry.path().to_path_buf());
        }
    }
    archives.sort();
    Ok(archives)
}

fn safe_top_level_member_name(raw_name: &str) -> Option<String> {
    let normalized = raw_name.replace('\\', "/");
    if normalized.starts_with('/') {
        return None;
    }
    let mut parts = normalized.split('/');
    let first = parts.next()?;
    if first.is_empty() || first == "." || first == ".." || parts.next().is_some() {
        return None;
    }
    Some(first.to_string())
}

fn is_partitioned_member(name: &str, include_assets: bool) -> bool {
    if name.ends_with(".json") {
        return true;
    }
    include_assets
        && (name.starts_with("file-") || name.starts_with("file_"))
        && name.ends_with(".dat")
}

fn optional_file(root: &Path, name: &str) -> Option<PathBuf> {
    let p = root.join(name);
    p.exists().then_some(p)
}

fn glob_paths(root: &Path, pattern: &str) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().to_string();
        if wildcard_match(pattern, &name) && is_conversation_shard(&name) {
            out.push(entry.path());
        }
    }
    Ok(out)
}

fn is_conversation_shard(name: &str) -> bool {
    name.starts_with("conversations-")
        && name.ends_with(".json")
        && name
            .strip_prefix("conversations-")
            .and_then(|s| s.strip_suffix(".json"))
            .is_some_and(|suffix| !suffix.is_empty() && suffix.chars().all(|c| c.is_ascii_digit()))
}

fn wildcard_match(pattern: &str, name: &str) -> bool {
    if let Some(prefix) = pattern.strip_suffix("*.json") {
        name.starts_with(prefix) && name.ends_with(".json")
    } else {
        name == pattern
    }
}

fn natural_key(path: &Path) -> String {
    path.file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn discovers_shards() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("conversations-001.json"), "[]").unwrap();
        fs::write(tmp.path().join("conversations-000.json"), "[]").unwrap();
        let layout = discover_export(tmp.path(), false).unwrap();
        assert_eq!(layout.conversation_paths.len(), 2);
        assert!(layout.conversation_paths[0]
            .file_name()
            .unwrap()
            .to_string_lossy()
            .contains("000"));
    }

    #[test]
    fn strict_requires_manifest() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("conversations-000.json"), "[]").unwrap();
        assert!(discover_export(tmp.path(), true).is_err());
    }

    #[test]
    fn ignores_non_numeric_shard_files() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("conversations-000.json"), "[]").unwrap();
        fs::write(tmp.path().join("conversations-corrupt.json"), "bad").unwrap();
        let layout = discover_export(tmp.path(), false).unwrap();
        assert_eq!(layout.conversation_paths.len(), 1);
    }

    #[test]
    fn discovers_partitioned_export_and_keeps_source_untouched() {
        use std::io::Write;

        fn write_archive(path: &Path, entries: &[(&str, &str)]) {
            let file = fs::File::create(path).unwrap();
            let mut archive = zip::ZipWriter::new(file);
            let options = zip::write::SimpleFileOptions::default();
            for (name, content) in entries {
                archive.start_file(*name, options).unwrap();
                archive.write_all(content.as_bytes()).unwrap();
            }
            archive.finish().unwrap();
        }

        let tmp = TempDir::new().unwrap();
        let source = tmp.path().join("OpenAI-export");
        let activity = source.join("User Online Activity");
        fs::create_dir_all(&activity).unwrap();
        let conversations = r#"[{"id":"conv-1","mapping":{}}]"#;
        write_archive(
            &activity.join("Conversations__export-part-0001.zip"),
            &[
                ("conversations-000.json", conversations),
                ("conversation_asset_file_names.json", "{}"),
                ("file-asset.dat", "asset"),
            ],
        );
        write_archive(
            &activity.join("Conversations__export-part-0005.zip"),
            &[("export_manifest.json", r#"{"export_files":[]}"#)],
        );

        let layout = discover_export(&source, false).unwrap();
        let staged_root = layout.root.clone();
        assert_ne!(layout.root, source);
        assert_eq!(layout.source_root, fs::canonicalize(&source).unwrap());
        assert_eq!(layout.conversation_paths.len(), 1);
        assert!(layout.manifest_path.is_some());
        assert!(staged_root
            .join("conversation_asset_file_names.json")
            .exists());
        assert!(activity
            .join("Conversations__export-part-0001.zip")
            .exists());
        assert!(!staged_root.join("file-asset.dat").exists());

        drop(layout);
        assert!(!staged_root.exists());

        let asset_layout = discover_export_with_assets(&source, false, true).unwrap();
        assert_eq!(
            fs::read(asset_layout.root.join("file-asset.dat")).unwrap(),
            b"asset"
        );
    }
}
