use std::{
    net::{IpAddr, SocketAddr},
    path::PathBuf,
};

use axum::{
    body::Body,
    http::{header, HeaderValue},
    response::Response,
    routing::get,
    Router,
};

use crate::{
    commands::serve,
    error::Result,
    read_model::ReadRepository,
};

const INDEX_HTML: &str = include_str!("../../web/index.html");
const STYLES_CSS: &str = include_str!("../../web/styles.css");
const APP_JS: &str = include_str!("../../web/app.js");

pub fn app(db_path: PathBuf, assets_dir: Option<PathBuf>) -> Router {
    serve::app(db_path, assets_dir)
        .route("/", get(index))
        .route("/recall.css", get(styles))
        .route("/recall.js", get(script))
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
             The API and web explorer have no authentication or TLS and may expose private conversation data."
        );
    }

    ReadRepository::open_read_only(&db_path)?;
    let addr = SocketAddr::new(host, port);
    tokio::runtime::Runtime::new()?.block_on(async move {
        let app = app(db_path, assets_dir);
        let listener = tokio::net::TcpListener::bind(addr).await?;
        println!("RecallEngine web explorer listening on http://{addr}");
        axum::serve(listener, app)
            .await
            .map_err(std::io::Error::other)
    })?;
    Ok(())
}

async fn index() -> Response {
    static_response("text/html; charset=utf-8", INDEX_HTML)
}

async fn styles() -> Response {
    static_response("text/css; charset=utf-8", STYLES_CSS)
}

async fn script() -> Response {
    static_response("text/javascript; charset=utf-8", APP_JS)
}

fn static_response(content_type: &'static str, content: &'static str) -> Response {
    let mut response = Response::new(Body::from(content));
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
