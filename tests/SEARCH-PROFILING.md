# M1 search profiling

M1 does not optimize search before measuring it. The ignored `search_profile` test compares the real read-model search path with the `/v1/search` Axum/JSON path for the deliberately broad query `je`.

The profiling database is always explicit and is opened read-only. The test refuses to run when the path is missing, which makes it safe against a cleaned `/tmp` directory instead of silently profiling another database.

```bash
RECALL_PROFILE_DB=/path/to/history.sqlite \
  cargo test --test search_profile profile_search_je_sqlite_and_api -- --ignored --nocapture
```

The report prints min/median/p95/max for:

- `ReadRepository::search_page("je", 60, None)` — SQLite/read-model cost;
- `GET /v1/search?q=je&limit=60` — handler + model conversion + JSON body cost, without network transport.

Use a representative imported database and record the output before changing FTS, LIKE fallback, pagination, indexes, or response shaping. The test is diagnostic only and performs no writes.
