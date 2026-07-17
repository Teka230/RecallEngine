use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BundleLimits {
    pub max_words_per_file: Option<u64>,
    pub max_bytes_per_file: Option<u64>,
    pub max_files_per_bundle: Option<u32>,
    pub max_conversations_per_file: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BundleProfile {
    pub name: String,
    pub limits: BundleLimits,
}

impl BundleProfile {
    pub fn notebooklm() -> Self {
        Self {
            name: "notebooklm".to_string(),
            limits: BundleLimits {
                max_words_per_file: Some(450_000),
                max_bytes_per_file: Some(190 * 1024 * 1024), // 190 MB
                max_files_per_bundle: Some(50),              // standard profile in python
                max_conversations_per_file: None,
            },
        }
    }

    pub fn chatgpt() -> Self {
        Self {
            name: "chatgpt".to_string(),
            limits: BundleLimits {
                max_words_per_file: Some(1_200_000),
                max_bytes_per_file: Some(45 * 1024 * 1024), // 45 MB
                max_files_per_bundle: Some(20),
                max_conversations_per_file: None,
            },
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        match name.to_lowercase().as_str() {
            "notebooklm" => Some(Self::notebooklm()),
            "chatgpt" => Some(Self::chatgpt()),
            _ => None,
        }
    }
}
