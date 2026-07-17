use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct JsonEnvelope<T: Serialize> {
    pub schema_version: String,
    #[serde(flatten)]
    pub data: T,
}

impl<T: Serialize> JsonEnvelope<T> {
    pub fn new(data: T) -> Self {
        Self {
            schema_version: "1".to_string(),
            data,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct JsonError {
    pub schema_version: String,
    pub error: ErrorDetail,
}

#[derive(Debug, Serialize)]
pub struct ErrorDetail {
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
}

impl JsonError {
    pub fn new(
        code: impl Into<String>,
        message: impl Into<String>,
        details: Option<serde_json::Value>,
    ) -> Self {
        Self {
            schema_version: "1".to_string(),
            error: ErrorDetail {
                code: code.into(),
                message: message.into(),
                details,
            },
        }
    }
}
