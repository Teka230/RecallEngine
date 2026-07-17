use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContentBlock {
    pub id: String,
    pub message_id: String,
    pub ordinal: i32,
    pub kind: String,
    pub text_content: Option<String>,
    pub json_content: Option<String>,
}
