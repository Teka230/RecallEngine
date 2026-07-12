use thiserror::Error;

pub type Result<T> = std::result::Result<T, RecallError>;

#[derive(Debug, Error)]
pub enum RecallError {
    #[error("database error: {0}")]
    Database(#[from] rusqlite::Error),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("verification failed")]
    VerifyFailed,

    #[error("{0}")]
    Message(String),
}

impl RecallError {
    pub fn msg(s: impl Into<String>) -> Self {
        Self::Message(s.into())
    }
}
