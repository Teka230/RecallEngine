use std::collections::HashMap;
use std::fs;
use std::path::Path;

use serde::Deserialize;
use serde_json::Value;

use crate::domain::canonical::{
    ContentBlockRecord, ContentReferenceRecord, ConversationRecord, MessageCandidate, NodeRecord,
    ParsedFragment,
};
use crate::domain::chatgpt_raw::{bool_int, conversation_id, mapping_entries, unix_to_iso};
use crate::error::Result;
use crate::import::assets::{build_asset_id, normalize_asset_key};
use crate::import::ExportLayout;

#[derive(Deserialize)]
struct MetadataConversation {
    id: Option<String>,
    conversation_id: Option<String>,
    mapping: Option<std::collections::BTreeMap<String, MetadataNode>>,
}

#[derive(Deserialize)]
struct MetadataNode {
    message: Option<MetadataMessage>,
}

#[derive(Deserialize)]
struct MetadataMessage {
    id: Option<String>,
    #[serde(deserialize_with = "optional_f64")]
    create_time: Option<f64>,
}

fn optional_f64<'de, D>(deserializer: D) -> std::result::Result<Option<f64>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(Option::<Value>::deserialize(deserializer)?.and_then(|value| value.as_f64()))
}

/// The small, stable subset of a message needed to allocate its IC.
///
/// Keeping this separate from `MessageCandidate` ensures the planning pass does
/// not retain raw JSON, content blocks, references, or message metadata.
#[derive(Debug, Clone, PartialEq)]
pub struct IcCandidate {
    pub id: String,
    pub conversation_id: String,
    pub create_time: Option<f64>,
    pub create_time_raw: Option<f64>,
    pub source_shard_index: i32,
    pub source_node_order: i32,
}

impl IcCandidate {
    pub fn into_message_candidate(self, source_relative_path: String) -> MessageCandidate {
        MessageCandidate {
            id: self.id,
            node_id: String::new(),
            conversation_id: self.conversation_id,
            role: None,
            author_name: None,
            create_time: self.create_time,
            create_time_raw: self.create_time_raw,
            timestamp: unix_to_iso(self.create_time),
            source_shard_index: self.source_shard_index,
            source_node_order: self.source_node_order,
            model_slug: None,
            content_type: None,
            source_relative_path,
            raw_json: String::new(),
            content: Value::Null,
            metadata: Value::Null,
        }
    }
}

/// Metadata retained between the planning and writing passes for one shard.
#[derive(Debug, Clone, PartialEq)]
pub struct FragmentMetadata {
    pub relative_path: String,
    pub shard_index: i32,
    pub messages: Vec<IcCandidate>,
}

/// Parse only the fields required for deterministic IC planning.
pub fn parse_fragment_metadata(
    layout: &ExportLayout,
    path: &Path,
    shard_index: i32,
) -> Result<FragmentMetadata> {
    let relative_path = layout.relative(path);
    let reader = std::io::BufReader::new(fs::File::open(path)?);
    let conversations: Vec<MetadataConversation> = serde_json::from_reader(reader)?;
    let mut messages = Vec::new();

    for conversation in conversations {
        let Some(conversation_id) = conversation.id.or(conversation.conversation_id) else {
            continue;
        };
        for (node_order, (node_id, node)) in conversation
            .mapping
            .unwrap_or_default()
            .into_iter()
            .enumerate()
        {
            let Some(message) = node.message else {
                continue;
            };
            let create_time = message.create_time;
            messages.push(IcCandidate {
                id: message.id.unwrap_or(node_id.clone()),
                conversation_id: conversation_id.clone(),
                create_time,
                create_time_raw: create_time,
                source_shard_index: shard_index,
                source_node_order: node_order as i32,
            });
        }
    }

    Ok(FragmentMetadata {
        relative_path,
        shard_index,
        messages,
    })
}

pub fn parse_fragment(
    layout: &ExportLayout,
    path: &Path,
    shard_index: i32,
) -> Result<ParsedFragment> {
    let relative_path = layout.relative(path);
    let reader = std::io::BufReader::new(fs::File::open(path)?);
    let conversations: Vec<Value> = serde_json::from_reader(reader)?;

    let mut out = ParsedFragment {
        relative_path: relative_path.clone(),
        shard_index,
        conversations: Vec::new(),
        nodes: Vec::new(),
        messages: Vec::new(),
        content_blocks: Vec::new(),
        content_references: Vec::new(),
    };

    for (conv_index, conv) in conversations.into_iter().enumerate() {
        if !conv.is_object() || conv.as_object().is_some_and(|o| o.is_empty()) {
            tracing::debug!(
                "skipping invalid conversation at index {conv_index} in {relative_path} (Empty object from OpenAI)"
            );
            continue;
        }
        let Some(conv_id) = conversation_id(&conv) else {
            tracing::debug!(
                "skipping conversation without id at index {conv_index} in {relative_path}"
            );
            continue;
        };
        let conv_raw = serde_json::to_string(&conv)?;

        out.conversations.push(ConversationRecord {
            id: conv_id.clone(),
            title: conv
                .get("title")
                .and_then(|v| v.as_str())
                .map(str::to_string),
            create_time: conv.get("create_time").and_then(|v| v.as_f64()),
            update_time: conv.get("update_time").and_then(|v| v.as_f64()),
            current_node_id: conv
                .get("current_node")
                .and_then(|v| v.as_str())
                .map(str::to_string),
            default_model_slug: conv
                .get("default_model_slug")
                .and_then(|v| v.as_str())
                .map(str::to_string),
            is_archived: bool_int(conv.get("is_archived")),
            is_starred: bool_int(conv.get("is_starred")),
            source_relative_path: relative_path.clone(),
            raw_json: conv_raw,
        });

        for (node_order, (node_id, node)) in mapping_entries(&conv).into_iter().enumerate() {
            let node_raw = serde_json::to_string(&node)?;
            let parent_id = node
                .get("parent")
                .and_then(|v| v.as_str())
                .map(str::to_string);
            let message = node.get("message");
            let has_message = message.map(|m| !m.is_null()).unwrap_or(false);

            out.nodes.push(NodeRecord {
                id: node_id.clone(),
                conversation_id: conv_id.clone(),
                parent_id,
                has_message,
                source_relative_path: relative_path.clone(),
                raw_json: node_raw,
            });

            if !has_message {
                continue;
            }
            let msg = message.unwrap();
            let msg_id = msg
                .get("id")
                .and_then(|v| v.as_str())
                .unwrap_or(&node_id)
                .to_string();
            let create_time_raw = msg.get("create_time").and_then(|v| v.as_f64());
            let timestamp = unix_to_iso(create_time_raw);
            let content = msg.get("content").cloned().unwrap_or(Value::Null);
            let metadata = msg
                .get("metadata")
                .cloned()
                .unwrap_or(Value::Object(Default::default()));
            let content_type = content
                .get("content_type")
                .and_then(|v| v.as_str())
                .map(str::to_string);

            out.messages.push(MessageCandidate {
                id: msg_id.clone(),
                node_id: node_id.clone(),
                conversation_id: conv_id.clone(),
                role: msg
                    .get("author")
                    .and_then(|a| a.get("role"))
                    .and_then(|v| v.as_str())
                    .map(str::to_string),
                author_name: msg
                    .get("author")
                    .and_then(|a| a.get("name"))
                    .and_then(|v| v.as_str())
                    .map(str::to_string),
                create_time: create_time_raw,
                create_time_raw,
                timestamp,
                source_shard_index: shard_index,
                source_node_order: node_order as i32,
                model_slug: msg
                    .get("metadata")
                    .and_then(|m| m.get("model_slug"))
                    .and_then(|v| v.as_str())
                    .or_else(|| msg.get("model_slug").and_then(|v| v.as_str()))
                    .map(str::to_string),
                content_type: content_type.clone(),
                source_relative_path: relative_path.clone(),
                raw_json: serde_json::to_string(msg)?,
                content: content.clone(),
                metadata: metadata.clone(),
            });

            out.content_blocks
                .extend(build_content_blocks(&msg_id, &content_type, &content));
            out.content_references
                .extend(build_content_references(&msg_id, &metadata, &content));
        }
    }

    Ok(out)
}

pub fn build_content_blocks(
    message_id: &str,
    content_type: &Option<String>,
    content: &Value,
) -> Vec<ContentBlockRecord> {
    let ct = content_type.as_deref().unwrap_or("unknown");
    let parts = content.get("parts").and_then(|p| p.as_array());

    let mut blocks = Vec::new();
    if let Some(parts) = parts {
        if parts.is_empty() {
            blocks.push(single_block(message_id, 0, ct, content));
        } else {
            for (ordinal, part) in parts.iter().enumerate() {
                blocks.push(part_to_block(message_id, ordinal as i32, ct, part, content));
            }
        }
    } else {
        blocks.push(single_block(message_id, 0, ct, content));
    }
    blocks
}

fn single_block(message_id: &str, ordinal: i32, ct: &str, content: &Value) -> ContentBlockRecord {
    ContentBlockRecord {
        id: format!("{message_id}:{ordinal}"),
        message_id: message_id.to_string(),
        ordinal,
        kind: map_content_kind(ct, content),
        text_content: extract_text(content),
        json_content: Some(content.to_string()),
    }
}

fn part_to_block(
    message_id: &str,
    ordinal: i32,
    ct: &str,
    part: &Value,
    _full_content: &Value,
) -> ContentBlockRecord {
    let kind = if part.is_string() {
        map_content_kind(ct, part)
    } else if part.get("asset_pointer").is_some() || part.get("image_url").is_some() {
        "asset_reference".to_string()
    } else {
        map_content_kind(ct, part)
    };
    ContentBlockRecord {
        id: format!("{message_id}:{ordinal}"),
        message_id: message_id.to_string(),
        ordinal,
        kind,
        text_content: extract_text(part),
        json_content: Some(part.to_string()),
    }
}

fn map_content_kind(ct: &str, value: &Value) -> String {
    match ct {
        "text" => "text".to_string(),
        "multimodal_text" => {
            if value.get("asset_pointer").is_some() || value.get("image_url").is_some() {
                "asset_reference".to_string()
            } else {
                "text".to_string()
            }
        }
        "thoughts" => "thoughts".to_string(),
        "reasoning_recap" => "reasoning_recap".to_string(),
        _ => "unknown".to_string(),
    }
}

fn extract_text(value: &Value) -> Option<String> {
    if let Some(s) = value.as_str() {
        return Some(s.to_string());
    }
    if let Some(t) = value.get("text").and_then(|v| v.as_str()) {
        return Some(t.to_string());
    }
    if let Some(parts) = value.get("parts").and_then(|p| p.as_array()) {
        let joined: Vec<String> = parts
            .iter()
            .filter_map(|p| p.as_str().map(str::to_string).or_else(|| extract_text(p)))
            .collect();
        if !joined.is_empty() {
            return Some(joined.join("\n\n"));
        }
    }
    None
}

pub fn build_content_references(
    message_id: &str,
    metadata: &Value,
    content: &Value,
) -> Vec<ContentReferenceRecord> {
    let mut refs = Vec::new();
    if let Some(arr) = metadata
        .get("content_references")
        .and_then(|v| v.as_array())
    {
        for (ordinal, item) in arr.iter().enumerate() {
            refs.push(ContentReferenceRecord {
                id: format!("{message_id}:{ordinal}:metadata"),
                message_id: message_id.to_string(),
                ordinal: ordinal as i32,
                ref_source: "metadata".to_string(),
                raw_json: item.to_string(),
            });
        }
    }
    if let Some(arr) = content.get("content_references").and_then(|v| v.as_array()) {
        for (ordinal, item) in arr.iter().enumerate() {
            refs.push(ContentReferenceRecord {
                id: format!("{message_id}:{ordinal}:content"),
                message_id: message_id.to_string(),
                ordinal: ordinal as i32,
                ref_source: "content".to_string(),
                raw_json: item.to_string(),
            });
        }
    }
    refs
}

#[derive(Debug, Default)]
pub struct AssetLink {
    pub message_id: String,
    pub asset_id: String,
    pub link_source: String,
    pub ordinal: i32,
    pub raw_json: String,
}

#[derive(Debug, Default)]
pub struct UnresolvedAssetLink {
    pub message_id: String,
    pub raw_key: String,
    pub link_source: String,
    pub ordinal: i32,
    pub raw_json: String,
}

pub fn collect_asset_links(
    messages: &[MessageCandidate],
    mapping: &HashMap<String, String>,
) -> (Vec<AssetLink>, Vec<UnresolvedAssetLink>) {
    let mut links = Vec::new();
    let mut unresolved = Vec::new();

    for msg in messages {
        if let Some(atts) = msg.metadata.get("attachments").and_then(|v| v.as_array()) {
            for (ordinal, att) in atts.iter().enumerate() {
                let raw_id = att
                    .get("id")
                    .or_else(|| att.get("file_id"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if let Some(source_key) = normalize_asset_key(raw_id, mapping) {
                    links.push(AssetLink {
                        message_id: msg.id.clone(),
                        asset_id: build_asset_id(&source_key),
                        link_source: "metadata_attachment".to_string(),
                        ordinal: ordinal as i32,
                        raw_json: att.to_string(),
                    });
                } else if !raw_id.is_empty() {
                    unresolved.push(UnresolvedAssetLink {
                        message_id: msg.id.clone(),
                        raw_key: raw_id.to_string(),
                        link_source: "metadata_attachment".to_string(),
                        ordinal: ordinal as i32,
                        raw_json: att.to_string(),
                    });
                }
            }
        }

        if let Some(parts) = msg.content.get("parts").and_then(|p| p.as_array()) {
            for (ordinal, part) in parts.iter().enumerate() {
                let pointer = part
                    .get("asset_pointer")
                    .or_else(|| part.get("image_url"))
                    .and_then(|v| v.as_str());
                if let Some(ptr) = pointer {
                    if let Some(source_key) = normalize_asset_key(ptr, mapping) {
                        links.push(AssetLink {
                            message_id: msg.id.clone(),
                            asset_id: build_asset_id(&source_key),
                            link_source: "content_part_pointer".to_string(),
                            ordinal: ordinal as i32,
                            raw_json: part.to_string(),
                        });
                    } else {
                        unresolved.push(UnresolvedAssetLink {
                            message_id: msg.id.clone(),
                            raw_key: ptr.to_string(),
                            link_source: "content_part_pointer".to_string(),
                            ordinal: ordinal as i32,
                            raw_json: part.to_string(),
                        });
                    }
                }
            }
        }
    }

    (links, unresolved)
}

pub fn load_asset_mapping(path: &Path) -> Result<HashMap<String, String>> {
    let raw = fs::read_to_string(path)?;
    let value: Value = serde_json::from_str(&raw)?;
    let mut map = HashMap::new();
    if let Some(obj) = value.as_object() {
        for (k, v) in obj {
            if let Some(name) = v.as_str() {
                map.insert(k.clone(), name.to_string());
            }
        }
    }
    Ok(map)
}
