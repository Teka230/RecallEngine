use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub id: String,
    pub ic: i64,
    pub node_id: String,
    pub conversation_id: String,
    pub role: Option<String>,
    pub author_name: Option<String>,
    pub create_time: Option<f64>,
    pub timestamp: Option<String>,
    pub source_shard_index: i32,
    pub source_node_order: i32,
    pub model_slug: Option<String>,
    pub content_type: Option<String>,
    pub is_active: bool,
}

impl Message {
    /// Retourne true si le message est éligible pour l'export public (utilisateur ou assistant)
    pub fn is_reference_role(&self) -> bool {
        matches!(
            self.role
                .as_deref()
                .map(|r| r.trim().to_ascii_lowercase())
                .as_deref(),
            Some("user") | Some("assistant")
        )
    }
}
