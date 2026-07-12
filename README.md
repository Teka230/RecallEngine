# RecallEngine

**A local, privacy-first Rust CLI for turning a ChatGPT export into a durable SQLite history.**

RecallEngine imports a ChatGPT data export without sending it anywhere. It preserves the conversation graph, message IDs, source JSON, attachment references, and branch structure in a canonical local database you can verify, inspect, search, or export to the flat SQLite format used by GPTExtractor and ExploGPT. The result is an addressable local message repository with a stable reference scheme—not an intelligent context-selection or semantic-ranking engine.

> [!WARNING]
> **Alpha software — local use only.** RecallEngine currently supports ChatGPT exports only. It has no UI, embeddings, cloud sync, profile import, authentication, or TLS. It is not published on crates.io yet; build it from this repository.

## Why RecallEngine?

- **Local by design** — the CLI works on files and SQLite databases on your machine.
- **Faithful imports** — keeps source message IDs, conversation branches, content blocks, attachment references, feedback, and supported ChatGPT sidecars.
- **Stable local references** — each imported message receives a durable integer `IC` (Internal Citation), used as a compact human/machine handle for the local SQLite database. Existing ICs are preserved on re-import.
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

RecallEngine accepts either `conversations.json` or numbered shards such as `conversations-000.json`. When present, it also reads supported sidecars including `export_manifest.json`, `message_feedback.json`, `shared_conversations.json`, `library_files.json`, and `conversation_asset_file_names.json`.

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

> [!CAUTION]
> **Your history is extremely sensitive.** The API has **no authentication and no TLS**. Keep its default loopback binding (`127.0.0.1`); never expose it to your LAN, the internet, a reverse proxy, or untrusted users.

Search uses SQLite FTS5 for token-prefix matching, with a `LIKE` fallback for literal text. It is designed for inspection, not semantic or embedding-based retrieval.

The reference endpoints (`/api/messages/by-ic/...`, `/by-message-id/...`, and `/by-reference`) and `show` provide the human-facing filtered surface: only active `user`/`assistant` messages are returned. Their JSON keeps the legacy `id` field and adds the equivalent `messageId` plus the human-readable `reference`. The conversation, graph, and search endpoints remain inspection surfaces: they may include technical `system` and `tool` rows, with `ic: null`, so consumers that need a faithful view of an imported conversation should use them instead of the reference endpoints.

Context resolution:

- `before` and `after` count active `user`/`assistant` messages.
- Neighbors are ordered by ascending IC, not timestamp.
- `scope=conversation` searches all active branches of the target conversation using the same ascending IC order.
- `scope=corpus` traverses eligible messages in ascending IC order. This defines a deterministic IC neighborhood rather than chronological proximity.
- Inactive messages and inactive conversations are not context neighbors.

> [!IMPORTANT]
> Do not import into a database while `recall serve`, another import, or an export is using that same file. SQLite permits only one writer and this alpha does not coordinate concurrent operations or configure retry handling for lock contention.

## Logging and diagnostics

The local API does not log successful requests, message content, search text, JSON payloads, or full sensitive asset paths. An IC that is not found is a normal `404` client result, not a server failure. Startup and operational failures are surfaced by the CLI; HTTP failures are returned to the caller. Use HTTP responses and process error output for diagnostics.

## Privacy and safety

ChatGPT exports and the databases created by RecallEngine can contain personal identifiers, credentials, private code, financial information, and intimate conversations.

- Keep exports, databases, generated reports, assets, and screenshots outside this repository.
- Never commit or paste real conversation data into an issue, pull request, log, or public service.
- Use only anonymized, minimal fixtures when contributing tests.
- Do not change the API host away from `127.0.0.1` unless you fully understand the exposure and provide the required protections yourself.

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
