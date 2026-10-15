use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    routing::get,
    Json, Router,
};
use serde::{Deserialize, Serialize};

use super::serve::AppState;
use crate::{
    domain::reference::{is_reference_role, MessageReference},
    read_model::{
        AssetView, ConversationListItem, MessageView, MessageViewMode, ReadRepository, SearchHit,
    },
    storage::CURRENT_SCHEMA_VERSION,
};

const DEFAULT_LIMIT: usize = 100;
const MAX_LIMIT: usize = 500;
const MAX_CONTEXT_RADIUS: usize = 50;

pub(crate) fn router() -> Router<AppState> {
    Router::new()
        .route("/meta", get(get_meta))
        .route("/conversations", get(list_conversations))
        .route("/conversations/{id}", get(get_conversation))
        .route("/conversations/{id}/messages", get(list_messages))
        .route("/messages/{id}", get(get_message))
        .route("/messages/{id}/context", get(get_context))
        .route("/search", get(search))
        .route("/references/{*reference}", get(resolve_reference))
}

#[derive(Debug, Serialize)]
struct MetaResponse {
    api_schema_version: &'static str,
    database_schema_version: u32,
    snapshot_id: Option<String>,
    database_path: String,
    read_only: bool,
}

#[derive(Debug, Serialize)]
struct PageResponse<T> {
    items: Vec<T>,
    next_cursor: Option<String>,
}

#[derive(Debug, Serialize)]
struct ConversationSummaryResponse {
    conversation_id: String,
    title: String,
    updated_at: Option<f64>,
    message_count: i64,
    default_model_slug: Option<String>,
    has_branches: bool,
}

#[derive(Debug, Serialize)]
struct ConversationResponse {
    conversation_id: String,
    title: String,
    updated_at: Option<f64>,
    message_count: i64,
    current_node_id: Option<String>,
    default_model_slug: Option<String>,
    has_branches: bool,
    assets: Vec<ApiAsset>,
}

#[derive(Debug, Serialize)]
struct ApiAsset {
    id: String,
    name: String,
    mime_type: Option<String>,
    relative_path: Option<String>,
    exists_locally: bool,
}

#[derive(Debug, Serialize)]
struct BranchInfo {
    node_id: String,
    parent_id: Option<String>,
    is_current_thread: Option<bool>,
}

#[derive(Debug, Serialize)]
struct ApiMessage {
    message_id: String,
    ic: Option<i64>,
    conversation_id: String,
    role: String,
    model_slug: Option<String>,
    content: String,
    created_at: Option<f64>,
    timestamp: Option<String>,
    node_id: String,
    parent_id: Option<String>,
    technical: bool,
    visible: bool,
    branch: BranchInfo,
    reference: Option<String>,
    stable_reference: Option<String>,
    assets: Vec<ApiAsset>,
}

#[derive(Debug, Serialize)]
struct SearchResponse {
    conversation_id: String,
    conversation_title: String,
    message_id: String,
    ic: Option<i64>,
    role: String,
    model_slug: Option<String>,
    excerpt: String,
    created_at: Option<f64>,
    timestamp: Option<String>,
    node_id: String,
    parent_id: Option<String>,
    technical: bool,
    visible: bool,
    branch: BranchInfo,
    reference: Option<String>,
    stable_reference: Option<String>,
    assets: Vec<ApiAsset>,
}

#[derive(Debug, Serialize)]
struct ContextResponse {
    view: &'static str,
    before_requested: usize,
    after_requested: usize,
    center: ApiMessage,
    before: Vec<ApiMessage>,
    after: Vec<ApiMessage>,
}

#[derive(Debug, Serialize)]
struct ReferenceResponse {
    requested_reference: String,
    reference: String,
    stable_reference: String,
    message: ApiMessage,
}

#[derive(Debug, Deserialize)]
struct ConversationQuery {
    q: Option<String>,
    limit: Option<usize>,
    cursor: Option<String>,
}

#[derive(Debug, Deserialize)]
struct MessagesQuery {
    view: Option<String>,
    limit: Option<usize>,
    cursor: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ContextQuery {
    view: Option<String>,
    before: Option<usize>,
    after: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct SearchQuery {
    q: String,
    limit: Option<usize>,
    cursor: Option<String>,
    mode: Option<String>,
}

type ApiResult<T> = std::result::Result<Json<T>, (StatusCode, String)>;

async fn get_meta(State(state): State<AppState>) -> ApiResult<MetaResponse> {
    let repository = repository(&state)?;
    Ok(Json(MetaResponse {
        api_schema_version: "1",
        database_schema_version: CURRENT_SCHEMA_VERSION,
        snapshot_id: repository.latest_snapshot_id().map_err(map_error)?,
        database_path: state.db_path.display().to_string(),
        read_only: true,
    }))
}

async fn list_conversations(
    State(state): State<AppState>,
    Query(query): Query<ConversationQuery>,
) -> ApiResult<PageResponse<ConversationSummaryResponse>> {
    let repository = repository(&state)?;
    let page = repository
        .list_conversations_cursor(
            query.q.as_deref().unwrap_or_default(),
            page_limit(query.limit),
            query.cursor.as_deref(),
        )
        .map_err(map_error)?;
    let items = page.items.into_iter().map(conversation_summary).collect();
    Ok(Json(PageResponse {
        items,
        next_cursor: page.next_cursor,
    }))
}

async fn get_conversation(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<ConversationResponse> {
    let repository = repository(&state)?;
    let meta = repository
        .conversation_meta(&id)
        .map_err(map_error)?
        .ok_or_else(|| not_found("Conversation not found"))?;
    let assets = repository
        .conversation_assets(&id)
        .map_err(map_error)?
        .into_iter()
        .map(asset)
        .collect();
    Ok(Json(ConversationResponse {
        conversation_id: meta.id,
        title: meta.title,
        updated_at: meta.updated_at,
        message_count: meta.message_count,
        current_node_id: meta.current_node_id,
        default_model_slug: meta.default_model_slug,
        has_branches: meta.has_branches,
        assets,
    }))
}

async fn list_messages(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<MessagesQuery>,
) -> ApiResult<PageResponse<ApiMessage>> {
    let mode = parse_view(query.view.as_deref())?;
    let repository = repository(&state)?;
    let conversation = repository
        .conversation_meta(&id)
        .map_err(map_error)?
        .ok_or_else(|| not_found("Conversation not found"))?;
    let current_thread_marker = match mode {
        MessageViewMode::CurrentThread if conversation.current_node_id.is_some() => Some(true),
        _ => None,
    };
    let page = repository
        .message_page(&id, mode, page_limit(query.limit), query.cursor.as_deref())
        .map_err(map_error)?;
    let items = page
        .items
        .into_iter()
        .map(|message| api_message(&repository, message, current_thread_marker))
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(Json(PageResponse {
        items,
        next_cursor: page.next_cursor,
    }))
}

async fn get_message(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<ApiMessage> {
    let repository = repository(&state)?;
    let message = repository
        .message_by_id(&id)
        .map_err(map_error)?
        .ok_or_else(|| not_found("Message not found"))?;
    Ok(Json(api_message(&repository, message, None)?))
}

async fn get_context(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<ContextQuery>,
) -> ApiResult<ContextResponse> {
    let mode = parse_view(query.view.as_deref())?;
    let before = query.before.unwrap_or(2);
    let after = query.after.unwrap_or(2);
    if before > MAX_CONTEXT_RADIUS || after > MAX_CONTEXT_RADIUS {
        return Err(bad_request(
            "before and after must be less than or equal to 50",
        ));
    }
    let repository = repository(&state)?;
    let window = repository
        .message_window(&id, before, after, mode)
        .map_err(map_error)?
        .ok_or_else(|| not_found("Message not found in requested view"))?;
    let has_current_node = repository
        .conversation_meta(&window.center.conversation_id)
        .map_err(map_error)?
        .is_some_and(|meta| meta.current_node_id.is_some());
    let current_thread_marker =
        (mode == MessageViewMode::CurrentThread && has_current_node).then_some(true);
    let center = api_message(&repository, window.center, current_thread_marker)?;
    let before = window
        .before
        .into_iter()
        .map(|message| api_message(&repository, message, current_thread_marker))
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let after = window
        .after
        .into_iter()
        .map(|message| api_message(&repository, message, current_thread_marker))
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(Json(ContextResponse {
        view: mode.as_str(),
        before_requested: query.before.unwrap_or(2),
        after_requested: query.after.unwrap_or(2),
        center,
        before,
        after,
    }))
}

async fn search(
    State(state): State<AppState>,
    Query(query): Query<SearchQuery>,
) -> ApiResult<PageResponse<SearchResponse>> {
    let repository = repository(&state)?;
    let mode = match query.mode.as_deref() {
        Some("hybrid") => crate::search::SearchMode::Hybrid,
        Some("semantic") => crate::search::SearchMode::Semantic,
        Some("lexical") | None => crate::search::SearchMode::Lexical,
        Some(other) => {
            return Err(bad_request(format!(
                "unsupported mode '{other}'; use lexical, semantic, or hybrid"
            )));
        }
    };

    let mut embedder = if mode != crate::search::SearchMode::Lexical
        && crate::semantic::embedding::is_e5_small_cached(None)
    {
        crate::semantic::embedding::FastEmbedProvider::try_new_offline(None).ok()
    } else {
        None
    };

    if mode == crate::search::SearchMode::Semantic {
        let hybrid = crate::semantic::hybrid::HybridSearchService::new(repository.connection());
        if !hybrid.is_vector_index_available() {
            return Err(bad_request(
                "Semantic search index not found or empty. Run 'recall embed' first.",
            ));
        }
        if embedder.is_none() {
            return Err(bad_request(
                "Model cache for 'intfloat/multilingual-e5-small' not found. Run 'recall embed' to set up.",
            ));
        }
    }

    let page = repository
        .search_page_with_mode(
            &query.q,
            page_limit(query.limit),
            query.cursor.as_deref(),
            mode,
            embedder
                .as_mut()
                .map(|e| e as &mut dyn crate::semantic::EmbeddingProvider),
        )
        .map_err(map_error)?;
    let items = page
        .items
        .into_iter()
        .map(|hit| search_response(&repository, hit))
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(Json(PageResponse {
        items,
        next_cursor: page.next_cursor,
    }))
}

async fn resolve_reference(
    State(state): State<AppState>,
    Path(reference): Path<String>,
) -> ApiResult<ReferenceResponse> {
    let requested_reference = reference.trim_start_matches('/').to_string();
    let parsed = requested_reference
        .parse::<MessageReference>()
        .map_err(|error| bad_request(error.to_string()))?;
    let repository = repository(&state)?;
    let resolved = repository
        .resolve_reference(&parsed)
        .map_err(map_error)?
        .ok_or_else(|| not_found("Reference not found"))?;
    let message = repository
        .message_by_id(&resolved.id)
        .map_err(map_error)?
        .ok_or_else(|| not_found("Reference target not found"))?;
    let api_message = api_message(&repository, message, None)?;
    let reference = parsed.human();
    let stable_reference = parsed.token();
    Ok(Json(ReferenceResponse {
        requested_reference,
        reference,
        stable_reference,
        message: api_message,
    }))
}

fn repository(state: &AppState) -> std::result::Result<ReadRepository, (StatusCode, String)> {
    ReadRepository::open_read_only(&state.db_path).map_err(map_error)
}

fn page_limit(limit: Option<usize>) -> usize {
    limit.unwrap_or(DEFAULT_LIMIT).min(MAX_LIMIT)
}

fn parse_view(value: Option<&str>) -> std::result::Result<MessageViewMode, (StatusCode, String)> {
    match value {
        Some("current-thread") => Ok(MessageViewMode::CurrentThread),
        Some("all-active") => Ok(MessageViewMode::AllActive),
        Some(other) => Err(bad_request(format!(
            "unsupported view '{other}'; use current-thread or all-active"
        ))),
        None => Err(bad_request(
            "view is required; use current-thread or all-active",
        )),
    }
}

fn conversation_summary(item: ConversationListItem) -> ConversationSummaryResponse {
    ConversationSummaryResponse {
        conversation_id: item.id,
        title: item.title,
        updated_at: item.updated_at,
        message_count: item.message_count,
        default_model_slug: item.default_model_slug,
        has_branches: item.has_branches,
    }
}

fn api_message(
    repository: &ReadRepository,
    message: MessageView,
    is_current_thread: Option<bool>,
) -> std::result::Result<ApiMessage, (StatusCode, String)> {
    let technical = message.is_technical();
    let (reference, stable_reference) = message
        .ic
        .and_then(|ic| MessageReference::new(ic, message.id.clone()).ok())
        .map(|reference| (Some(reference.human()), Some(reference.token())))
        .unwrap_or((None, None));
    let assets = repository
        .message_assets(&message.id)
        .map_err(map_error)?
        .into_iter()
        .map(asset)
        .collect();
    Ok(ApiMessage {
        message_id: message.id,
        ic: message.ic,
        conversation_id: message.conversation_id,
        role: message.role,
        model_slug: message.model_slug,
        content: message.content,
        created_at: message.created_at,
        timestamp: message.timestamp,
        node_id: message.node_id.clone(),
        parent_id: message.parent_node_id.clone(),
        technical,
        visible: !technical,
        branch: BranchInfo {
            node_id: message.node_id,
            parent_id: message.parent_node_id,
            is_current_thread,
        },
        reference,
        stable_reference,
        assets,
    })
}

fn search_response(
    repository: &ReadRepository,
    hit: SearchHit,
) -> std::result::Result<SearchResponse, (StatusCode, String)> {
    let technical = !is_reference_role(Some(&hit.role));
    let (reference, stable_reference) = hit
        .ic
        .and_then(|ic| MessageReference::new(ic, hit.message_id.clone()).ok())
        .map(|reference| (Some(reference.human()), Some(reference.token())))
        .unwrap_or((None, None));
    let assets = repository
        .message_assets(&hit.message_id)
        .map_err(map_error)?
        .into_iter()
        .map(asset)
        .collect();
    Ok(SearchResponse {
        conversation_id: hit.conversation_id,
        conversation_title: hit.conversation_title,
        message_id: hit.message_id,
        ic: hit.ic,
        role: hit.role,
        model_slug: hit.model_slug,
        excerpt: hit.excerpt,
        created_at: hit.created_at,
        timestamp: hit.timestamp,
        node_id: hit.node_id.clone(),
        parent_id: hit.parent_node_id.clone(),
        technical,
        visible: !technical,
        branch: BranchInfo {
            node_id: hit.node_id,
            parent_id: hit.parent_node_id,
            is_current_thread: None,
        },
        reference,
        stable_reference,
        assets,
    })
}

fn asset(value: AssetView) -> ApiAsset {
    ApiAsset {
        id: value.id,
        name: value.name,
        mime_type: value.mime_type,
        relative_path: value.relative_path,
        exists_locally: value.exists_locally,
    }
}

fn bad_request(message: impl Into<String>) -> (StatusCode, String) {
    (StatusCode::BAD_REQUEST, message.into())
}

fn not_found(message: impl Into<String>) -> (StatusCode, String) {
    (StatusCode::NOT_FOUND, message.into())
}

fn map_error(error: impl std::fmt::Display) -> (StatusCode, String) {
    let message = error.to_string();
    if message.starts_with("invalid cursor") || message.contains("Semantic search") {
        return bad_request(message);
    }
    (StatusCode::INTERNAL_SERVER_ERROR, message)
}
