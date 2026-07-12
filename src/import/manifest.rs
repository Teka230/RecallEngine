use std::fs;

use serde_json::Value;

use crate::error::{RecallError, Result};
use crate::import::ExportLayout;

pub fn load_manifest(layout: &ExportLayout, strict: bool) -> Result<Option<Value>> {
    let Some(path) = &layout.manifest_path else {
        return Ok(None);
    };
    match fs::read_to_string(path) {
        Ok(raw) => Ok(Some(serde_json::from_str(&raw)?)),
        Err(e) => {
            if strict {
                Err(RecallError::msg(format!("manifest unreadable: {e}")))
            } else {
                tracing::warn!("manifest unreadable: {e}");
                Ok(None)
            }
        }
    }
}

pub fn validate_manifest_sizes(manifest: &Value, layout: &ExportLayout) -> Vec<String> {
    let mut warnings = Vec::new();
    let Some(files) = manifest.get("export_files").and_then(|v| v.as_array()) else {
        return warnings;
    };
    for entry in files {
        let Some(rel) = entry.get("path").and_then(|v| v.as_str()) else {
            continue;
        };
        let expected = entry.get("size_bytes").and_then(|v| v.as_u64());
        let path = layout.root.join(rel);
        if let (Ok(meta), Some(exp)) = (fs::metadata(&path), expected) {
            if meta.len() != exp {
                warnings.push(format!(
                    "size mismatch for {rel}: expected {exp}, got {}",
                    meta.len()
                ));
            }
        }
    }
    warnings
}
