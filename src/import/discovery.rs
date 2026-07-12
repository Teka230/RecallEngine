use std::fs;
use std::path::{Path, PathBuf};

use crate::error::{RecallError, Result};

#[derive(Debug, Clone)]
pub struct ExportLayout {
    pub root: PathBuf,
    pub conversation_paths: Vec<PathBuf>,
    pub manifest_path: Option<PathBuf>,
    pub asset_mapping_path: Option<PathBuf>,
    pub feedback_path: Option<PathBuf>,
    pub shared_path: Option<PathBuf>,
    pub library_path: Option<PathBuf>,
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
    let root =
        fs::canonicalize(root).map_err(|e| RecallError::msg(format!("export not found: {e}")))?;
    let mut shards: Vec<PathBuf> = glob_paths(&root, "conversations-*.json")?;
    if shards.is_empty() {
        let single = root.join("conversations.json");
        if single.exists() {
            shards.push(single);
        }
    }
    if shards.is_empty() {
        return Err(RecallError::msg(
            "no conversations.json or conversations-*.json found",
        ));
    }
    shards.sort_by_key(|path| natural_key(path));

    let manifest_path = root.join("export_manifest.json");
    let has_manifest = manifest_path.exists();
    if strict && !has_manifest {
        return Err(RecallError::msg(
            "export_manifest.json required in --strict mode",
        ));
    }

    Ok(ExportLayout {
        root: root.clone(),
        conversation_paths: shards,
        manifest_path: has_manifest.then_some(manifest_path),
        asset_mapping_path: optional_file(&root, "conversation_asset_file_names.json"),
        feedback_path: optional_file(&root, "message_feedback.json"),
        shared_path: optional_file(&root, "shared_conversations.json"),
        library_path: optional_file(&root, "library_files.json"),
    })
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
}
