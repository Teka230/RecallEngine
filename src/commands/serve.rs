use std::{
    net::SocketAddr,
    path::{Path as FsPath, PathBuf},
    sync::Arc,
};

use axum::body::Body;
use axum::http::header;
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use rusqlite::{Connection, OpenFlags, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::domain::reference::{
    get_active_message_by_ic, get_active_message_by_id, get_ic_context, resolve_message_reference,
    ContextScope, IcContext, MessageReference, ReferencedMessage,
};
use crate::error::{RecallError, Result};
use crate::read_model::ReadRepository;
use crate::storage::Database;

#[derive(Clone)]
struct AppState {
    db_path: Arc<PathBuf>,
    assets_dir: Arc<PathBuf>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ConversationSummary {
    id: String,
    title: String,
    updated_at: Option<f64>,
    message_count: i64,
    excerpt: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Message {
    id: String,
    ic: Option<i64>,
    role: String,
    content: String,
    created_at: Option<f64>,
    timestamp: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Asset {
    id: String,
    name: String,
    mime_type: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ConversationDetail {
    id: String,
    title: String,
    updated_at: Option<f64>,
    message_count: i64,
    messages: Vec<Message>,
    assets: Vec<Asset>,
}

#[derive(Deserialize)]
struct ListQuery {
    q: Option<String>,
    limit: Option<usize>,
}

#[derive(Deserialize)]
struct SearchQuery {
    q: String,
    limit: Option<usize>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SearchResult {
    conversation_id: String,
    conversation_title: String,
    message_id: String,
    ic: Option<i64>,
    role: String,
    content: String,
    created_at: Option<f64>,
    timestamp: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct GraphNode {
    id: String,
    parent_id: Option<String>,
    message_id: Option<String>,
    ic: Option<i64>,
    role: Option<String>,
    preview: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ConversationGraph {
    conversation_id: String,
    nodes: Vec<GraphNode>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AssetListItem {
    id: String,
    name: String,
    mime_type: Option<String>,
    exists_locally: bool,
    conversation_id: String,
    conversation_title: String,
}

type ApiResult<T> = std::result::Result<Json<T>, (StatusCode, String)>;

pub fn run(db_path: PathBuf, assets_dir: Option<PathBuf>, host: String, port: u16) -> Result<()> {
    // Apply pending migrations before switching to the read-only API connection.
    Database::open(&db_path)?;
    let addr = format!("{host}:{port}")
        .parse::<SocketAddr>()
        .map_err(|error| RecallError::msg(error.to_string()))?;
    tokio::runtime::Runtime::new()?.block_on(async move {
        let app = app(db_path, assets_dir);
        let listener = tokio::net::TcpListener::bind(addr).await?;
        println!("RecallEngine API listening on http://{addr}");
        axum::serve(listener, app)
            .await
            .map_err(std::io::Error::other)
    })?;
    Ok(())
}

pub fn app(db_path: PathBuf, assets_dir: Option<PathBuf>) -> Router {
    let state = AppState {
        assets_dir: Arc::new(assets_dir.unwrap_or_else(|| {
            db_path
                .parent()
                .map(|path| path.join("assets"))
                .unwrap_or_else(|| PathBuf::from("assets"))
        })),
        db_path: Arc::new(db_path),
    };
    Router::new()
        .route("/api/health", get(health))
        .route("/api/conversations", get(list_conversations))
        .route("/api/conversations/{id}", get(get_conversation))
        .route("/api/conversations/{id}/graph", get(get_graph))
        .route("/api/search", get(search))
        .route("/api/messages/by-ic/{ic}", get(get_message_by_ic))
        .route(
            "/api/messages/by-message-id/{message_id}",
            get(get_message_by_id),
        )
        .route("/api/messages/by-reference", get(get_message_by_reference))
        .route("/api/messages/by-ic/{ic}/context", get(get_message_context))
        .route("/api/assets", get(list_assets))
        .route("/api/assets/{id}/file", get(get_asset_file))
        .with_state(state)
}

async fn get_asset_file(
    State(state): State<AppState>,
    Path(id): axum::extract::Path<String>,
) -> std::result::Result<Response, (StatusCode, String)> {
    let connection = open_read_only(&state).map_err(to_http_error)?;
    let row: Option<(Option<String>, Option<String>, i64)> = connection
        .query_row(
            "SELECT relative_path, mime_type, exists_locally FROM assets WHERE id = ?1 AND is_active = 1",
            [&id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(to_http_error)?;
    let Some((Some(relative_path), mime_type, exists_locally)) = row else {
        return Err((StatusCode::NOT_FOUND, "Asset not found".into()));
    };
    if exists_locally == 0 {
        return Err((
            StatusCode::NOT_FOUND,
            "Asset is not available locally".into(),
        ));
    }
    let path = confined_asset_path(&state.assets_dir, &relative_path)
        .map_err(|_| (StatusCode::NOT_FOUND, "Asset not found".into()))?;
    let bytes =
        std::fs::read(&path).map_err(|_| (StatusCode::NOT_FOUND, "Asset not found".into()))?;
    let content_type = mime_type
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "application/octet-stream".to_string());
    let mut response = Response::new(Body::from(bytes));
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        content_type
            .parse()
            .unwrap_or_else(|_| header::HeaderValue::from_static("application/octet-stream")),
    );
    response.headers_mut().insert(
        header::HeaderName::from_static("x-content-type-options"),
        header::HeaderValue::from_static("nosniff"),
    );
    Ok(response)
}

fn confined_asset_path(root: &FsPath, relative_path: &str) -> std::io::Result<PathBuf> {
    let relative = FsPath::new(relative_path);
    if relative.is_absolute()
        || relative
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "invalid asset path",
        ));
    }
    let root = std::fs::canonicalize(root)?;
    let candidate = std::fs::canonicalize(root.join(relative))?;
    if candidate.starts_with(&root) {
        Ok(candidate)
    } else {
        Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "asset escapes root",
        ))
    }
}

async fn health() -> impl IntoResponse {
    Json(serde_json::json!({ "status": "ok" }))
}

#[derive(Deserialize)]
struct ContextQuery {
    before: Option<usize>,
    after: Option<usize>,
    scope: Option<ContextScope>,
}

async fn get_message_by_ic(
    State(state): State<AppState>,
    Path(ic): Path<i64>,
) -> ApiResult<ReferencedMessage> {
    if ic <= 0 {
        return Err((StatusCode::BAD_REQUEST, "Invalid IC".into()));
    }
    let connection = open_read_only(&state).map_err(to_http_error)?;
    let message = get_active_message_by_ic(&connection, ic)
        .map_err(to_http_error)?
        .ok_or_else(|| (StatusCode::NOT_FOUND, format!("IC {ic} not found")))?;
    Ok(Json(message))
}

async fn get_message_by_id(
    State(state): State<AppState>,
    Path(message_id): Path<String>,
) -> ApiResult<ReferencedMessage> {
    if message_id.trim().is_empty() {
        return Err((StatusCode::BAD_REQUEST, "Invalid message ID".into()));
    }
    let connection = open_read_only(&state).map_err(to_http_error)?;
    let message = get_active_message_by_id(&connection, &message_id)
        .map_err(to_http_error)?
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                format!("Message {message_id} not found"),
            )
        })?;
    Ok(Json(message))
}

#[derive(Deserialize)]
struct ReferenceQuery {
    #[serde(rename = "ref")]
    reference: String,
}

async fn get_message_by_reference(
    State(state): State<AppState>,
    Query(query): Query<ReferenceQuery>,
) -> ApiResult<ReferencedMessage> {
    let reference = query
        .reference
        .parse::<MessageReference>()
        .map_err(|error| (StatusCode::BAD_REQUEST, error.to_string()))?;
    let connection = open_read_only(&state).map_err(to_http_error)?;
    let message = resolve_message_reference(&connection, &reference)
        .map_err(to_reference_error)?
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                format!("Message {} not found", reference.message_id),
            )
        })?;
    Ok(Json(message))
}

async fn get_message_context(
    State(state): State<AppState>,
    Path(ic): Path<i64>,
    Query(query): Query<ContextQuery>,
) -> ApiResult<IcContext> {
    if ic <= 0 {
        return Err((StatusCode::BAD_REQUEST, "Invalid IC".into()));
    }
    let before = query.before.unwrap_or(2);
    let after = query.after.unwrap_or(2);
    if before > 50 || after > 50 {
        return Err((
            StatusCode::BAD_REQUEST,
            "before et after doivent être inférieurs ou égaux à 50".into(),
        ));
    }
    let scope = query.scope.unwrap_or(ContextScope::Conversation);
    let connection = open_read_only(&state).map_err(to_http_error)?;
    let context = get_ic_context(&connection, ic, before, after, scope)
        .map_err(to_http_error)?
        .ok_or_else(|| (StatusCode::NOT_FOUND, format!("IC {ic} not found")))?;
    Ok(Json(context))
}

async fn list_conversations(
    State(state): State<AppState>,
    Query(query): Query<ListQuery>,
) -> ApiResult<Vec<ConversationSummary>> {
    let term = query.q.unwrap_or_default().trim().to_owned();
    let limit = query.limit.unwrap_or(100).min(500);
    let repository =
        ReadRepository::open_read_only(state.db_path.as_ref()).map_err(to_http_error)?;
    let rows = repository
        .list_conversations(&term, limit)
        .map_err(to_http_error)?;
    Ok(Json(
        rows.into_iter()
            .map(|conversation| ConversationSummary {
                id: conversation.id,
                title: conversation.title,
                updated_at: conversation.updated_at,
                message_count: conversation.message_count,
                excerpt: conversation.excerpt,
            })
            .collect(),
    ))
}

async fn get_conversation(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<ConversationDetail> {
    let repository =
        ReadRepository::open_read_only(state.db_path.as_ref()).map_err(to_http_error)?;
    let meta = repository
        .conversation_meta(&id)
        .map_err(to_http_error)?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Conversation not found".into()))?;
    let updated_at: Option<f64> = open_read_only(&state)
        .and_then(|connection| {
            connection.query_row(
                "SELECT update_time FROM conversations WHERE id = ?1 AND is_active = 1",
                [&id],
                |row| row.get(0),
            )
        })
        .map_err(to_http_error)?;
    let messages = repository
        .conversation_messages(&id)
        .map_err(to_http_error)?
        .into_iter()
        .map(|message| Message {
            id: message.id,
            ic: message.ic,
            role: message.role,
            content: message.content,
            created_at: message.created_at,
            timestamp: message.timestamp,
        })
        .collect::<Vec<_>>();
    let assets = repository
        .conversation_assets(&id)
        .map_err(to_http_error)?
        .into_iter()
        .map(|asset| Asset {
            id: asset.id,
            name: asset.name,
            mime_type: asset.mime_type,
        })
        .collect::<Vec<_>>();
    let message_count = messages.len() as i64;
    Ok(Json(ConversationDetail {
        id,
        title: meta.title,
        updated_at,
        message_count,
        messages,
        assets,
    }))
}

async fn search(
    State(state): State<AppState>,
    Query(query): Query<SearchQuery>,
) -> ApiResult<Vec<SearchResult>> {
    let term = query.q.trim();
    if term.is_empty() {
        return Ok(Json(Vec::new()));
    }
    let repository =
        ReadRepository::open_read_only(state.db_path.as_ref()).map_err(to_http_error)?;
    let rows = repository
        .search(term, query.limit.unwrap_or(100).min(500))
        .map_err(to_http_error)?;
    Ok(Json(
        rows.into_iter()
            .map(|hit| SearchResult {
                conversation_id: hit.conversation_id,
                conversation_title: hit.conversation_title,
                message_id: hit.message_id,
                ic: hit.ic,
                role: hit.role,
                content: hit.excerpt,
                created_at: hit.created_at,
                timestamp: hit.timestamp,
            })
            .collect(),
    ))
}

async fn get_graph(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<ConversationGraph> {
    let repository =
        ReadRepository::open_read_only(state.db_path.as_ref()).map_err(to_http_error)?;
    let nodes = repository
        .conversation_graph(&id)
        .map_err(to_http_error)?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Conversation not found".into()))?
        .into_iter()
        .map(|node| GraphNode {
            id: node.id,
            parent_id: node.parent_id,
            message_id: node.message_id,
            ic: node.ic,
            role: node.role,
            preview: node.preview,
        })
        .collect();
    Ok(Json(ConversationGraph {
        conversation_id: id,
        nodes,
    }))
}

async fn list_assets(
    State(state): State<AppState>,
    Query(query): Query<ListQuery>,
) -> ApiResult<Vec<AssetListItem>> {
    let term = query.q.unwrap_or_default().trim().to_owned();
    let repository =
        ReadRepository::open_read_only(state.db_path.as_ref()).map_err(to_http_error)?;
    let assets = repository
        .list_assets(&term, query.limit.unwrap_or(100).min(500))
        .map_err(to_http_error)?
        .into_iter()
        .map(|asset| AssetListItem {
            id: asset.id,
            name: asset.name,
            mime_type: asset.mime_type,
            exists_locally: asset.exists_locally,
            conversation_id: asset.conversation_id,
            conversation_title: asset.conversation_title,
        })
        .collect();
    Ok(Json(assets))
}

fn open_read_only(state: &AppState) -> rusqlite::Result<Connection> {
    Connection::open_with_flags(
        state.db_path.as_ref(),
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
}

fn to_http_error(error: impl std::fmt::Display) -> (StatusCode, String) {
    (StatusCode::INTERNAL_SERVER_ERROR, error.to_string())
}

fn to_reference_error(error: RecallError) -> (StatusCode, String) {
    match error {
        RecallError::Message(message) => (StatusCode::BAD_REQUEST, message),
        other => to_http_error(other),
    }
}

#[cfg(test)]
mod tests {
    use super::confined_asset_path;

    #[test]
    fn confines_asset_paths_to_root() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("ok.txt"), "ok").unwrap();
        assert!(confined_asset_path(root.path(), "ok.txt").is_ok());
        assert!(confined_asset_path(root.path(), "../outside.txt").is_err());
        assert!(confined_asset_path(root.path(), "/etc/passwd").is_err());
    }
}
