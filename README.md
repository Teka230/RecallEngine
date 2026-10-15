# RecallEngine

**A local, privacy-first Rust CLI for turning a ChatGPT export into a durable SQLite history.**

RecallEngine imports a ChatGPT data export without sending it anywhere. It preserves the conversation graph, message IDs, source JSON, attachment references, and branch structure in a canonical local database you can verify, inspect, search, embed with local dense vectors, or export to the flat SQLite format used by GPTExtractor and ExploGPT. The result is an addressable local message repository with a stable reference scheme and privacy-first hybrid search (lexical + semantic).

> [!WARNING]
> **Alpha software — local use only.** RecallEngine currently supports ChatGPT exports only. It features local semantic indexing and hybrid search (via `sqlite-vec` and `fastembed`), a terminal visualizer TUI, and a local read-only web UI, but no cloud sync, profile import, authentication, or TLS. It is not published on crates.io yet; build it from this repository.

## Why RecallEngine?

- **Local by design** — the CLI works on files and SQLite databases on your machine.
- **Faithful imports** — keeps source message IDs, conversation branches, content blocks, attachment references, feedback, and supported ChatGPT sidecars.
- **Stable local references** — each imported message receives a durable integer `IC` (Internal Citation), used as a compact human/machine handle for the local SQLite database. Existing ICs are preserved on re-import.
- **Local hybrid & semantic search** — combine FTS5 BM25 lexical search with offline `intfloat/multilingual-e5-small` vector embeddings stored in SQLite via `sqlite-vec`. Incremental indexing evaluates content fingerprints to avoid recalculating embeddings on unchanged messages.
- **Inspectable output** — validate the import, print JSON statistics, use the optional local read API, or export a flat legacy SQLite table.
- **Lightweight Rust CLI** — a focused command-line tool rather than a hosted service.

## Before you start

### 1. Install Rust and Cargo

RecallEngine is distributed as source code and is not published on crates.io yet. You need the current stable [Rust toolchain](https://www.rust-lang.org/tools/install), which provides:

- `cargo`, to download dependencies, compile the binary, and run the tests;
- `rustc`, the Rust compiler used by Cargo.

After installing Rust, check that both commands are available:

```bash
rustc --version
cargo --version
```

RecallEngine builds SQLite through its Rust dependency. You do not need to install SQLite separately or configure a system SQLite library.

### 2. Download a ChatGPT data export

Use OpenAI's official guide: [How do I export my ChatGPT history and data?](https://help.openai.com/en/articles/7260999-how-do-i-export-my-data)

From ChatGPT:

1. Sign in and open your profile menu.
2. Go to **Settings → Data Controls**.
3. Select **Export data**, then confirm the export.
4. When OpenAI sends the confirmation email or SMS, choose **Download data export**.
5. Extract the downloaded ZIP archive and pass the extracted directory to `--source`.
   Newer large exports may contain `Conversations*.zip` partitions inside the
   export directory; that directory can be passed directly without manually
   unpacking those inner archives.

The export link expires after 24 hours, and preparing an export can take up to 7 days. The extracted directory normally contains `conversations.json`; large exports may contain numbered conversation shards instead. RecallEngine can also read supported sidecars and asset files from that same directory.

ChatGPT data export availability depends on the account or workspace type. If the export option is not available in ChatGPT, use the Privacy Portal instructions in the official OpenAI guide.

### 3. Or use the included sanitized fixture

For a local smoke test without personal data, use `tests/fixtures/chatgpt-sanitized/`. The quick-start example below imports this fixture and writes the generated SQLite database under `/tmp`.

## Quick start

This example uses the anonymized test fixture and keeps generated data outside the repository:

```bash
cargo build --release --bin recall

mkdir -p /tmp/recallengine-demo
./target/release/recall import chatgpt \
  --source tests/fixtures/chatgpt-sanitized \
  --db /tmp/recallengine-demo/history.sqlite \
  --assets external

./target/release/recall verify --db /tmp/recallengine-demo/history.sqlite
./target/release/recall stats --db /tmp/recallengine-demo/history.sqlite --json
```

For development, replace `./target/release/recall` with `cargo run --bin recall --`.

## Import a ChatGPT export

RecallEngine accepts either `conversations.json`, numbered shards such as
`conversations-000.json`, or a partitioned export directory containing
`Conversations*.zip`. For a partitioned directory, conversation JSON and
supported sidecars are materialized in a temporary staging directory; the
downloaded export is never modified. When present, RecallEngine also reads
sidecars including `export_manifest.json`, `message_feedback.json`,
`shared_conversations.json`, `library_files.json`, and
`conversation_asset_file_names.json`.

```bash
./target/release/recall import chatgpt \
  --source /path/to/chatgpt-export \
  --db /path/outside/the/repository/history.sqlite \
  --assets external
```

### Asset modes

| Mode | Behaviour |
| --- | --- |
| `external` (default) | Record asset references without copying or linking files. |
| `copy` | Copy source assets to the destination directory, avoiding redundant writes. |
| `symlink` | Create symlinks to source assets. |

For a partitioned export, `external` keeps the `.dat` references without
unpacking the large asset partitions; `verify` may therefore report those
assets as not locally materialized. Use `copy` or `symlink` when local asset
files are required.

Use `--assets-dir <PATH>` to choose the destination for copied or linked assets; otherwise an `assets` directory is created next to the database. Add `--strict` to require a valid `export_manifest.json`. Without it, malformed conversation shards are recorded as import issues while valid shards can still be imported. Use `--seed-legacy-ic /path/to/legacy.sqlite` only with a SQLite database previously produced by GPTExtractor or RecallEngine's legacy export, containing the expected legacy `messages` schema.

## Validate and inspect

```bash
./target/release/recall verify --db /path/to/history.sqlite
./target/release/recall stats --db /path/to/history.sqlite --json
```

`verify` checks the imported database. `stats --json` reports active conversations and messages, role totals, branch structure, attachment links, feedback, shared conversations, and library-file metadata.

For the bundled sanitized fixture, a successful run produces output like this:

```json
{
  "conversations": 4,
  "messages": 10,
  "user_messages": 4,
  "assistant_messages": 6,
  "branching_conversations": 1,
  "attachments": 2
}
```

`verify` exits successfully when the database is internally consistent. Missing local assets in `external` mode are reported as warnings, not verification failures.

### Inspect tool usage

`tools` scans `messages.raw_json` and prints a stable JSON summary of tool calls and tool results. It is read-only: it does not write the database or contact the network.

```bash
./target/release/recall tools --db /path/to/history.sqlite
```

Example shape:

```json
{
  "total_calls": 4,
  "total_results": 2,
  "tools": {
    "browser.search": {
      "calls": 2,
      "results": 1
    },
    "python": {
      "calls": 1,
      "results": 1
    }
  }
}
```

Tool names are sorted alphabetically. Counts come from assistant `tool_calls`, recipient / `function_call` fallbacks, and `tool` / named `system` result messages in the stored source JSON.

### Human-readable message references

The SQLite database is the source of truth for all IC values. IC references resolve only active `user` and `assistant` messages; technical roles such as `system` and `tool` remain stored but are not citation targets. The source message ID is the portable anchor stored in `messages.id`; the IC is the short local address, stable inside a given canonical database. New citations keep both together:

```text
[IC:42 | msg:00000000-0000-4000-8000-000000000042]
ref:ic/42/uuid/00000000-0000-4000-8000-000000000042
```

The bracketed form is for people and prompts; the `ref:ic/.../uuid/...` token is parseable by tools. Existing IC-only input remains accepted, but an IC alone is local to its database and may differ in an independently rebuilt corpus.

Resolve one message or request a bounded context window:

```bash
./target/release/recall show --db /path/to/history.sqlite --ic 42
./target/release/recall show --db /path/to/history.sqlite --message-id <message-id>
./target/release/recall show --db /path/to/history.sqlite --reference 'ref:ic/42/uuid/<message-id>'
./target/release/recall show --db /path/to/history.sqlite --ic 42 --before 2 --after 2 --scope conversation --json
```

`show` reports `IC 42 not found` when the IC does not resolve to an active user/assistant message. `before` and `after` count eligible active messages, selected by ascending IC rather than timestamp. Context responses contain only those eligible messages; technical or inactive rows are omitted. With `scope=conversation`, all active eligible messages in the target conversation are considered, across its branches. With `scope=corpus`, the same IC ordering is applied across the whole database.

### Browse the database in the terminal

`browse` is a read-only terminal explorer for a local RecallEngine database. It reads the normalized SQLite model through `ReadRepository`; it does not parse the source ChatGPT JSON, start the HTTP API, or modify the database.

```bash
./target/release/recall browse --db /path/to/history.sqlite
./target/release/recall browse --db /path/to/history.sqlite --ic 42
./target/release/recall browse --db /path/to/history.sqlite --conversation <conversation-uuid>
```

Use `/` to search, `i` to jump by IC, message ID, or composite reference, `v` to switch between the current conversation branch and all active messages in ascending IC order, `b` to choose a branch, `t` to reveal technical messages, `y` to copy the IC+message-ID pair, and `?` for complete keyboard help. Moving with `j/k` schedules the selected conversation after a 180 ms debounce; `Enter` loads it immediately when pending and focuses Reader. Conversation pages and search results are limited to 150, All messages to 500, and partial views are labeled in the interface. Technical messages remain inspectable but have no public reference. `browse` never writes to the database or sends data over the network.

### Semantic indexing and hybrid search

RecallEngine includes local vector indexing and hybrid search powered by `sqlite-vec` and `fastembed` (`intfloat/multilingual-e5-small`, 384 dimensions).

#### 1. Generate semantic embeddings

```bash
# Index with strict offline mode (requires pre-cached weights)
./target/release/recall embed --db /path/to/history.sqlite

# Or allow initial model download into local cache
./target/release/recall embed --db /path/to/history.sqlite --allow-download
```

Key features:
- **Pre-inference idempotence**: Evaluates chunk fingerprints (`sha256(model:version:chunk)`) before calling the embedding model. Re-indexing an unchanged database computes **0 embeddings**.
- **Atomic updates & pruning**: Updates chunks within SQLite transactions and prunes obsolete chunks (controllable via `--prune`).
- **Structured output**: Use `--json` for automation and pipeline integration.

#### 2. Query with search modes

```bash
# Default lexical search (FTS5 BM25)
./target/release/recall search --db /path/to/history.sqlite "quantum computing" --mode lexical

# Hybrid search (RRF fusing BM25 and vector distance)
./target/release/recall search --db /path/to/history.sqlite "quantum computing" --mode hybrid

# Pure dense vector similarity search
./target/release/recall search --db /path/to/history.sqlite "quantum computing" --mode semantic
```

- **In the terminal (`recall browse`)**: Press `/` to search (defaults to `hybrid`), and press `[Tab]` to cycle dynamically between `hybrid`, `lexical`, and `semantic` modes with live result updates.
- **In the Web UI**: Use the `Hybrid` / `Lexical` / `Semantic` toggle pills next to the search input.
- **Graceful fallback**: If the database has not been embedded yet or if model weights are absent, `hybrid` mode automatically falls back to standard FTS5 without error.

## Export for GPTExtractor or ExploGPT

Create the flat SQLite `messages` table expected by GPTExtractor or ExploGPT:

```bash
./target/release/recall export legacy-sqlite \
  --db /path/to/history.sqlite \
  --output /path/to/legacy.sqlite
```

Only active messages are exported, and their existing `IC` values are copied without recalculation.

## Optional local read API

Start the read-only API against an imported database:

```bash
./target/release/recall serve \
  --db /path/to/history.sqlite \
  --assets-dir /path/to/assets

curl http://127.0.0.1:8788/api/health
```

The same command serves the read-only web UI. Build it once before starting
`recall serve`:

```bash
cd web
npm install
npm run build
cd ..
./target/release/recall serve --db /path/to/history.sqlite
```

Then open <http://127.0.0.1:8788/>. The browser uses only the versioned
`/v1` read contract; it does not read SQLite directly or expose write actions.

Binding outside loopback requires an explicit operator consent flag:

```bash
# rejected — accidental LAN/public exposure
./target/release/recall serve --db /path/to/history.sqlite --host 0.0.0.0

# allowed only with consent; emits a stderr privacy warning
./target/release/recall serve --db /path/to/history.sqlite --host 0.0.0.0 --allow-remote
```

| Endpoint | Description |
| --- | --- |
| `GET /api/health` | Health response. |
| `GET /api/conversations?q=&limit=` | List conversations; `q` and `limit` are optional. |
| `GET /api/conversations/{id}` | Retrieve conversation messages and linked assets. |
| `GET /api/conversations/{id}/graph` | Retrieve conversation graph nodes. |
| `GET /api/search?q=&limit=` | Search active text content; `q` is required. |
| `GET /api/messages/by-ic/{ic}` | Resolve one active user/assistant message by local IC. |
| `GET /api/messages/by-message-id/{message_id}` | Resolve the same reference by portable source message ID. |
| `GET /api/messages/by-reference?ref=` | Resolve a matching composite IC+message-ID reference. |
| `GET /api/messages/by-ic/{ic}/context?before=&after=&scope=` | Resolve neighbors of IC `{ic}` using `before`, `after`, and `scope`. |
| `GET /api/assets?q=&limit=` | List linked assets. |
| `GET /api/assets/{id}/file` | Serve a locally copied or symlinked asset. |

The versioned read contract used by the web UI is documented in
[`READ-API-V1.md`](READ-API-V1.md). It is available under `/v1` and
provides explicit snapshot metadata, cursor pagination, `current-thread` /
`all-active` message views, context windows, search, and stable-reference
resolution. The `/api` routes above remain the compatibility surface.

> [!CAUTION]
> **Your history is extremely sensitive.** The API has **no authentication and no TLS**. Keep its default loopback binding (`127.0.0.1`). Binding a non-loopback address requires `--allow-remote` and still leaves the API unprotected — never expose it to your LAN, the internet, a reverse proxy, or untrusted users.

Search supports lexical FTS5 matching with substring fallback (`mode=lexical`), dense vectors via `sqlite-vec` (`mode=semantic`), and Reciprocal Rank Fusion (`mode=hybrid`, default in TUI and Web UI).

The reference endpoints (`/api/messages/by-ic/...`, `/by-message-id/...`, and `/by-reference`) and `show` provide the human-facing filtered surface: only active `user`/`assistant` messages are returned. Their JSON keeps the legacy `id` field and adds the equivalent `messageId` plus the human-readable `reference`. The conversation, graph, and search endpoints remain inspection surfaces: they may include technical `system` and `tool` rows, with `ic: null`, so consumers that need a faithful view of an imported conversation should use them instead of the reference endpoints.

Context resolution:

- `before` and `after` count active `user`/`assistant` messages.
- Neighbors are ordered by ascending IC, not timestamp.
- `scope=conversation` searches all active branches of the target conversation using the same ascending IC order.
- `scope=corpus` traverses eligible messages in ascending IC order. This defines a deterministic IC neighborhood rather than chronological proximity.
- Inactive messages and inactive conversations are not context neighbors.

> [!IMPORTANT]
> Concurrent use of one database file is **unsupported**. Do not run `import` while `serve`, `browse`, another `import`, or an export is using the same file. SQLite allows only one writer; this alpha does not coordinate processes or set a busy timeout.
>
> Measured spike (`tests/sqlite_concurrency.rs`, PR-H5): RecallEngine keeps the default **DELETE/rollback** journal (**Outcome A**). WAL can let writers finish while a long-lived reader holds a snapshot, but it does not make concurrent import+serve safe or consistent (uncommitted import rows stay invisible; two writers still contend), and it adds `-wal`/`-shm` sidecars that break simple backups and read-only distribution. `serve` / `browse` open the database read-only and must not change journal mode.

## Logging and diagnostics

The local API does not log successful requests, message content, search text, JSON payloads, or full sensitive asset paths. An IC that is not found is a normal `404` client result, not a server failure. Startup and operational failures are surfaced by the CLI; HTTP failures are returned to the caller. Use HTTP responses and process error output for diagnostics.

## Privacy and safety

ChatGPT exports and the databases created by RecallEngine can contain personal identifiers, credentials, private code, financial information, and intimate conversations.

- Keep exports, databases, generated reports, assets, and screenshots outside this repository.
- Never commit or paste real conversation data into an issue, pull request, log, or public service.
- Use only anonymized, minimal fixtures when contributing tests.
- Do not change the API host away from `127.0.0.1` unless you fully understand the exposure, pass `--allow-remote`, and provide the required protections yourself.

The repository ignores local `export/` and `docs/` work directories as a safety measure; generated data should stay outside the clone in any case.

## Development

Run the same checks as CI:

```bash
cargo fmt --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked
cargo run --bin recall -- --help
```

Contributions are welcome, especially focused bug reports, tests, and documentation improvements. Read [CONTRIBUTING.md](CONTRIBUTING.md) before opening a pull request and [SECURITY.md](SECURITY.md) for vulnerability reporting and exposure guidance.

## License

RecallEngine is released under the [MIT License](LICENSE).
