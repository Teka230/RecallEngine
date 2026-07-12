use serde_json::Value;

#[derive(Debug, Clone)]
pub struct MessageCandidate {
    pub id: String,
    pub node_id: String,
    pub conversation_id: String,
    pub role: Option<String>,
    pub author_name: Option<String>,
    pub create_time: Option<f64>,
    pub create_time_raw: Option<f64>,
    pub timestamp: Option<String>,
    pub source_shard_index: i32,
    pub source_node_order: i32,
    pub model_slug: Option<String>,
    pub content_type: Option<String>,
    pub source_relative_path: String,
    pub raw_json: String,
    pub content: Value,
    pub metadata: Value,
}

#[derive(Debug, Clone)]
pub struct NodeRecord {
    pub id: String,
    pub conversation_id: String,
    pub parent_id: Option<String>,
    pub has_message: bool,
    pub source_relative_path: String,
    pub raw_json: String,
}

#[derive(Debug, Clone)]
pub struct ConversationRecord {
    pub id: String,
    pub title: Option<String>,
    pub create_time: Option<f64>,
    pub update_time: Option<f64>,
    pub current_node_id: Option<String>,
    pub default_model_slug: Option<String>,
    pub is_archived: i32,
    pub is_starred: i32,
    pub source_relative_path: String,
    pub raw_json: String,
}

#[derive(Debug, Clone)]
pub struct ContentBlockRecord {
    pub id: String,
    pub message_id: String,
    pub ordinal: i32,
    pub kind: String,
    pub text_content: Option<String>,
    pub json_content: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ContentReferenceRecord {
    pub id: String,
    pub message_id: String,
    pub ordinal: i32,
    pub ref_source: String,
    pub raw_json: String,
}

#[derive(Debug, Clone)]
pub struct ParsedFragment {
    pub relative_path: String,
    pub shard_index: i32,
    pub conversations: Vec<ConversationRecord>,
    pub nodes: Vec<NodeRecord>,
    pub messages: Vec<MessageCandidate>,
    pub content_blocks: Vec<ContentBlockRecord>,
    pub content_references: Vec<ContentReferenceRecord>,
}
