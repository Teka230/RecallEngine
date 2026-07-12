use serde_json::Value;

/// Raw ChatGPT conversation JSON (permissive).
pub type RawConversation = Value;
pub type RawNode = Value;
pub type RawMessage = Value;

pub fn conversation_id(conv: &Value) -> Option<String> {
    conv.get("id")
        .or_else(|| conv.get("conversation_id"))
        .and_then(|v| v.as_str())
        .map(str::to_string)
}

pub fn mapping_entries(conv: &Value) -> Vec<(String, Value)> {
    conv.get("mapping")
        .and_then(|m| m.as_object())
        .map(|obj| obj.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
        .unwrap_or_default()
}

pub fn unix_to_iso(ts: Option<f64>) -> Option<String> {
    ts.map(|t| {
        time::OffsetDateTime::from_unix_timestamp(t as i64)
            .map(|dt| {
                dt.format(&time::format_description::well_known::Rfc3339)
                    .unwrap_or_default()
            })
            .unwrap_or_default()
    })
}

pub fn bool_int(v: Option<&Value>) -> i32 {
    v.and_then(|x| x.as_bool()).map(i32::from).unwrap_or(0)
}
