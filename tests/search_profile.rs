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

    for _ in 0..WARMUP_RUNS {
        black_box(
            repository
                .search_page(QUERY, LIMIT, None)
                .expect("warm up repository search"),
        );
    }

    let mut sqlite_timings = Vec::with_capacity(MEASURED_RUNS);
    let mut sample_count = 0usize;
    let mut sample_has_more = false;
    for _ in 0..MEASURED_RUNS {
        let started = Instant::now();
        let page = repository
            .search_page(QUERY, LIMIT, None)
            .expect("profile repository search");
        sqlite_timings.push(started.elapsed());
        sample_count = page.items.len();
        sample_has_more = page.next_cursor.is_some();
        black_box(page);
    }

    let app = serve::app(db.clone(), None);
    for _ in 0..WARMUP_RUNS {
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

    let mut api_timings = Vec::with_capacity(MEASURED_RUNS);
    for _ in 0..MEASURED_RUNS {
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
    }

    println!("DB: {}", db.display());
    println!(
        "query={QUERY:?} limit={LIMIT} first_page_items={sample_count} has_more={sample_has_more}"
    );
    summary("ReadRepository::search_page", &sqlite_timings);
    summary("GET /v1/search handler+JSON body", &api_timings);
}
