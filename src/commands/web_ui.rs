use std::{
    net::{IpAddr, SocketAddr},
    path::{Component, Path, PathBuf},
    sync::Arc,
};

use axum::{
    body::Body,
    extract::Path as AxumPath,
    http::{header, HeaderValue, StatusCode},
    response::Response,
    routing::get,
    Router,
};
use serde::Serialize;

use crate::{
    commands::serve,
    domain::reference::MessageReference,
    error::Result,
    read_model::ReadRepository,
};

#[derive(Debug, Serialize)]
struct IcResolution {
    ic: i64,
    message_id: String,
    conversation_id: String,
    reference: String,
    stable_reference: String,
}

pub fn app(db_path: PathBuf, assets_dir: Option<PathBuf>) -> Router {
    app_with_dist(db_path, assets_dir, default_dist_dir())
}

pub fn app_with_dist(
    db_path: PathBuf,
    assets_dir: Option<PathBuf>,
    dist_dir: PathBuf,
) -> Router {
    let db_path = Arc::new(db_path);
    let dist_dir = Arc::new(dist_dir);

    serve::app(db_path.as_ref().clone(), assets_dir)
        .route(
            "/v1/messages/by-ic/{ic}",
            get({
                let db_path = Arc::clone(&db_path);
                move |AxumPath(ic): AxumPath<i64>| {
                    let db_path = Arc::clone(&db_path);
                    async move { resolve_ic(db_path, ic).await }
                }
            }),
        )
        .route(
            "/",
            get({
                let dist_dir = Arc::clone(&dist_dir);
                move || {
                    let dist_dir = Arc::clone(&dist_dir);
                    async move { serve_index(dist_dir).await }
                }
            }),
        )
        .route(
            "/assets/{*path}",
            get({
                let dist_dir = Arc::clone(&dist_dir);
                move |AxumPath(path): AxumPath<String>| {
                    let dist_dir = Arc::clone(&dist_dir);
                    async move { serve_asset(dist_dir, path).await }
                }
            }),
        )
}

pub fn run(
    db_path: PathBuf,
    assets_dir: Option<PathBuf>,
    host: IpAddr,
    port: u16,
    allow_remote: bool,
) -> Result<()> {
    serve::validate_bind_host(host, allow_remote)?;
    if !host.is_loopback() {
        eprintln!(
            "WARNING: binding to {host} with --allow-remote. \
             The API and web UI have no authentication or TLS and may expose private conversation data."
        );
    }

    ReadRepository::open_read_only(&db_path)?;
    let addr = SocketAddr::new(host, port);
    tokio::runtime::Runtime::new()?.block_on(async move {
        let app = app(db_path, assets_dir);
        let listener = tokio::net::TcpListener::bind(addr).await?;
        println!("RecallEngine web UI listening on http://{addr}");
        axum::serve(listener, app)
            .await
            .map_err(std::io::Error::other)
    })?;
    Ok(())
}

async fn resolve_ic(
    db_path: Arc<PathBuf>,
    ic: i64,
) -> std::result::Result<axum::Json<IcResolution>, (StatusCode, String)> {
    if ic <= 0 {
        return Err((StatusCode::BAD_REQUEST, "IC must be a positive integer".into()));
    }
    let repository = ReadRepository::open_read_only(db_path.as_ref())
        .map_err(|error| (StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?;
    let message = repository
        .resolve_ic_reference(ic)
        .map_err(|error| (StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, format!("IC {ic} not found")))?;
    let reference = MessageReference::new(ic, message.message_id.clone())
        .map_err(|error| (StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?;
    Ok(axum::Json(IcResolution {
        ic,
        message_id: message.message_id,
        conversation_id: message.conversation_id,
        reference: reference.human(),
        stable_reference: reference.token(),
    }))
}

async fn serve_index(dist_dir: Arc<PathBuf>) -> Response {
    let path = dist_dir.join("index.html");
    match std::fs::read(path) {
        Ok(bytes) => static_response(StatusCode::OK, "text/html; charset=utf-8", bytes),
        Err(_) => static_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "text/html; charset=utf-8",
            br#"<!doctype html><html><body><main><h1>RecallEngine web UI is not built</h1><p>Run <code>npm install</code> then <code>npm run build</code> in <code>web/</code>.</p></main></body></html>"#.to_vec(),
        ),
    }
}

async fn serve_asset(dist_dir: Arc<PathBuf>, path: String) -> Response {
    let Some(relative) = safe_relative_path(&path) else {
        return static_response(
            StatusCode::NOT_FOUND,
            "text/plain; charset=utf-8",
            b"Not found".to_vec(),
        );
    };
    let file = dist_dir.join("assets").join(relative);
    match std::fs::read(&file) {
        Ok(bytes) => static_response(StatusCode::OK, content_type(&file), bytes),
        Err(_) => static_response(
            StatusCode::NOT_FOUND,
            "text/plain; charset=utf-8",
            b"Not found".to_vec(),
        ),
    }
}

fn default_dist_dir() -> PathBuf {
    std::env::var_os("RECALL_WEB_DIST")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("web/dist"))
}

fn safe_relative_path(value: &str) -> Option<PathBuf> {
    let path = Path::new(value.trim_start_matches('/'));
    if path.as_os_str().is_empty()
        || path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return None;
    }
    Some(path.to_path_buf())
}

fn content_type(path: &Path) -> &'static str {
    match path.extension().and_then(|value| value.to_str()) {
        Some("js") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("webp") => "image/webp",
        Some("json") => "application/json; charset=utf-8",
        _ => "application/octet-stream",
    }
}

fn static_response(status: StatusCode, content_type: &'static str, content: Vec<u8>) -> Response {
    let mut response = Response::new(Body::from(content));
    *response.status_mut() = status;
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(
        header::HeaderName::from_static("x-content-type-options"),
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        header::HeaderName::from_static("referrer-policy"),
        HeaderValue::from_static("no-referrer"),
    );
    headers.insert(
        header::HeaderName::from_static("content-security-policy"),
        HeaderValue::from_static(
            "default-src 'self'; script-src 'self'; style-src 'self'; connect-src 'self'; img-src 'self' data:; object-src 'none'; base-uri 'none'; frame-ancestors 'none'",
        ),
    );
    response
}
