use std::collections::HashMap;
use std::path::Path;

use crate::domain::ic::seed_legacy_ic_map;
use crate::error::Result;

pub use crate::domain::ic::LegacyIcSeed;

pub fn load_legacy_seed(path: &Path) -> Result<LegacyIcSeed> {
    seed_legacy_ic_map(path)
}

pub fn build_asset_id(source_key: &str) -> String {
    format!("chatgpt:{source_key}")
}

/// Resolve a raw attachment/pointer value to a mapping source_key.
pub fn normalize_asset_key(value: &str, mapping_keys: &HashMap<String, String>) -> Option<String> {
    let raw = value.trim();
    if raw.is_empty() {
        return None;
    }

    let mut candidates = Vec::new();
    let mut candidate = raw.to_string();
    if let Some(stripped) = candidate.strip_prefix("file-service://") {
        candidate = stripped.to_string();
    }
    candidates.push(candidate.clone());

    if let Some(base) = Path::new(&candidate).file_name().and_then(|s| s.to_str()) {
        candidates.push(base.to_string());
    }

    if !candidate.ends_with(".dat") {
        candidates.push(format!("{candidate}.dat"));
        if let Some(base) = Path::new(&candidate).file_name().and_then(|s| s.to_str()) {
            candidates.push(format!("{base}.dat"));
        }
    }

    for c in &candidates {
        if mapping_keys.contains_key(c) {
            return Some(c.clone());
        }
        let alt_dash = c.replace("file_", "file-");
        if mapping_keys.contains_key(&alt_dash) {
            return Some(alt_dash);
        }
        let alt_us = c.replace("file-", "file_");
        if mapping_keys.contains_key(&alt_us) {
            return Some(alt_us);
        }

        // New exports store the file id as a prefix of the local filename,
        // for example file-abc... -> file-abc...-uuid.png.
        if let Some(key) = mapping_keys
            .keys()
            .find(|key| key.starts_with(&format!("{c}-")))
        {
            return Some(key.clone());
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_file_service_pointer() {
        let mut map = HashMap::new();
        map.insert("file-abc.dat".to_string(), "image.png".to_string());
        let key = normalize_asset_key("file-service://file-abc", &map).unwrap();
        assert_eq!(key, "file-abc.dat");
    }

    #[test]
    fn resolves_attachment_without_dat() {
        let mut map = HashMap::new();
        map.insert("file-xyz.dat".to_string(), "doc.pdf".to_string());
        let key = normalize_asset_key("file-xyz", &map).unwrap();
        assert_eq!(key, "file-xyz.dat");
    }

    #[test]
    fn resolves_new_export_file_prefix() {
        let mut map = HashMap::new();
        map.insert(
            "file-abc-12345678.png".to_string(),
            "file-abc-12345678.png".to_string(),
        );
        let key = normalize_asset_key("file-service://file-abc", &map).unwrap();
        assert_eq!(key, "file-abc-12345678.png");
    }
}
