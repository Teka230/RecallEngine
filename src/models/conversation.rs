use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Conversation {
    pub id: String,
    pub title: Option<String>,
    pub create_time: Option<f64>,
    pub update_time: Option<f64>,
    pub current_node_id: Option<String>,
    pub default_model_slug: Option<String>,
    pub is_archived: bool,
    pub is_starred: bool,
    pub source_relative_path: String,
}
