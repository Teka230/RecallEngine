use std::{
    env,
    hint::black_box,
    path::PathBuf,
    time::{Duration, Instant},
};

use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
};
use recall_engine::{commands::serve, read_model::ReadRepository};
use tower::ServiceExt;

const QUERY: &str = "je";
const LIMIT: usize = 60;
const WARMUP_RUNS: usize = 3;
const MEASURED_RUNS: usize = 20;

fn configured_runs(name: &str, default: usize) -> usize {
    env::var(name)
        .ok()
        .map(|value| {
            value
                .parse::<usize>()
                .unwrap_or_else(|_| panic!("{name} must be a non-negative integer"))
        })
        .unwrap_or(default)
}

fn profile_db() -> PathBuf {
    let value = env::var_os("RECALL_PROFILE_DB")
        .expect("set RECALL_PROFILE_DB to an existing RecallEngine SQLite database");
    let path = PathBuf::from(value);
    assert!(
        path.is_file(),
        "RECALL_PROFILE_DB does not exist or is not a file: {}",
        path.display()
    );
    path
}

fn summary(label: &str, timings: &[Duration]) {
    let mut micros = timings.iter().map(Duration::as_micros).collect::<Vec<_>>();
    micros.sort_unstable();
    let median = micros[micros.len() / 2];
    let p95 = micros[((micros.len() - 1) * 95) / 100];
    println!(
        "{label}: min={}us median={}us p95={}us max={}us",
        micros[0],
        median,
        p95,
        micros[micros.len() - 1]
    );
}

#[tokio::test]
#[ignore = "manual profiling against an explicit local RecallEngine database"]
async fn profile_search_je_sqlite_and_api() {
    let db = profile_db();
    let repository = ReadRepository::open_read_only(&db).expect("open profiling DB read-only");
    let warmup_runs = configured_runs("RECALL_PROFILE_WARMUP_RUNS", WARMUP_RUNS);
    let measured_runs = configured_runs("RECALL_PROFILE_MEASURED_RUNS", MEASURED_RUNS);
    assert!(
        measured_runs > 0,
        "RECALL_PROFILE_MEASURED_RUNS must be greater than zero"
    );
    println!("profile configuration: warmup_runs={warmup_runs} measured_runs={measured_runs}");

    if warmup_runs > 0 {
        println!("SQLite warmup: {warmup_runs} run(s)");
    }
    for _ in 0..warmup_runs {
        black_box(
            repository
                .search_page(QUERY, LIMIT, None)
                .expect("warm up repository search"),
        );
    }

    let mut sqlite_timings = Vec::with_capacity(measured_runs);
    let mut sample_count = 0usize;
    let mut sample_has_more = false;
    println!("SQLite first measured query starting");
    for iteration in 0..measured_runs {
        let started = Instant::now();
        let page = repository
            .search_page(QUERY, LIMIT, None)
            .expect("profile repository search");
        sqlite_timings.push(started.elapsed());
        sample_count = page.items.len();
        sample_has_more = page.next_cursor.is_some();
        black_box(page);
        if iteration == 0 {
            println!(
                "SQLite first measured query: {}us",
                started.elapsed().as_micros()
            );
        }
    }

    let app = serve::app(db.clone(), None);
    if warmup_runs > 0 {
        println!("API warmup: {warmup_runs} run(s)");
    }
    for _ in 0..warmup_runs {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/v1/search?q=je&limit=60")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("warm up API search");
        assert_eq!(response.status(), StatusCode::OK);
        black_box(to_bytes(response.into_body(), usize::MAX).await.unwrap());
    }

    let mut api_timings = Vec::with_capacity(measured_runs);
    println!("API first measured query starting");
    for iteration in 0..measured_runs {
        let started = Instant::now();
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/v1/search?q=je&limit=60")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("profile API search");
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        black_box(body);
        api_timings.push(started.elapsed());
        if iteration == 0 {
            println!(
                "API first measured query: {}us",
                started.elapsed().as_micros()
            );
        }
    }

    println!("DB: {}", db.display());
    println!(
        "query={QUERY:?} limit={LIMIT} first_page_items={sample_count} has_more={sample_has_more}"
    );
    summary("ReadRepository::search_page", &sqlite_timings);
    summary("GET /v1/search handler+JSON body", &api_timings);
}
