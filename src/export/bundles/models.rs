use crate::export::bundles::profile::BundleProfile;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BundleStatistics {
    pub total_conversations: usize,
    pub total_messages: usize,
    pub total_words: u64,
    pub total_bytes: u64,
    pub sharded_conversations: usize,
    pub content_files_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlockSlicePlan {
    pub ordinal: i32,
    pub byte_start: usize,
    pub byte_end: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageSlicePlan {
    pub message_id: String,
    pub ic: i64,
    pub block_ranges: Vec<BlockSlicePlan>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShardPlan {
    pub conversation_id: String,
    pub title: String,
    pub part_number: u32,
    pub total_parts: u32,
    pub message_slices: Vec<MessageSlicePlan>,
    pub first_ic: Option<i64>,
    pub last_ic: Option<i64>,
    pub words: u64,
    pub bytes: u64,
    pub messages_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BundleFilePlan {
    pub relative_path: String,
    pub shards: Vec<ShardPlan>,
    pub total_words: u64,
    pub total_bytes: u64,
    pub total_messages: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuarterPlan {
    pub name: String,
    pub files: Vec<BundleFilePlan>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BundlePlan {
    pub profile: BundleProfile,
    pub quarters: Vec<QuarterPlan>,
    pub statistics: BundleStatistics,
}
