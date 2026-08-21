# RecallEngine read API v1

`/v1` is the versioned, read-only HTTP contract for a future RecallEngine web UI. It represents the canonical RecallEngine read model; it does not expose SQLite tables or GPTExtractor projections.

The existing `/api` routes remain available for compatibility. New clients should use `/v1`.

## Invariants

- RecallEngine is the canonical authority for conversation, message, branch, and reference data.
- The server opens the selected database read-only. Starting `serve` does not apply migrations.
- The server binds to loopback unless the operator explicitly uses `--allow-remote`.
- Field names in this contract are `snake_case`.
- Collections are returned as `{ "items": [...], "next_cursor": ... }`. `next_cursor` is `null` at the end.
- Cursors are opaque strings. A client must pass back the exact value returned by the server; it must not construct or interpret one.
- Page sizes are capped at 500. The default is 100.

## `GET /v1/meta`

Returns the identity of the selected read source:

```json
{
  "api_schema_version": "1",
  "database_schema_version": 2,
  "snapshot_id": "import-run:...",
  "database_path": "/path/to/recall.sqlite",
  "read_only": true
}
```

`snapshot_id` is derived from the latest completed or partial canonical import run. It is `null` when the database has no completed logical import yet.

## `GET /v1/conversations`

Query parameters:

- `q` — optional search term over titles and message text;
- `limit` — optional page size;
- `cursor` — cursor returned by the previous page.

Each item contains `conversation_id`, `title`, `updated_at`, `message_count`, `default_model_slug`, and `has_branches`. The list does not load conversation messages or assets implicitly.

`GET /v1/conversations/{conversation_id}` returns the conversation metadata plus `current_node_id` and its asset references. Messages are loaded only through the messages endpoint.

## `GET /v1/conversations/{conversation_id}/messages`

The `view` query parameter is required:

- `view=current-thread` — the path ending at the conversation's canonical `current_node_id`;
- `view=all-active` — all active messages in the conversation, in canonical IC order.

The frontend must send the view explicitly on every request. There is no API-side branch-scope default.

Message items expose:

```json
{
  "message_id": "message-id",
  "ic": 42,
  "conversation_id": "conversation-id",
  "role": "assistant",
  "model_slug": "gpt-5",
  "content": "...",
  "created_at": 1720000000.0,
  "timestamp": "2024-07-03T12:00:00Z",
  "node_id": "node-id",
  "parent_id": "parent-node-id",
  "technical": false,
  "visible": true,
  "branch": {
    "node_id": "node-id",
    "parent_id": "parent-node-id",
    "is_current_thread": true
  },
  "reference": "[IC:42 | msg:message-id]",
  "stable_reference": "ref:ic/42/uuid/message-id",
  "assets": []
}
```

Technical messages remain addressable by message ID, but they have `technical: true`, `visible: false`, and no public IC reference.

## `GET /v1/messages/{message_id}`

Resolves one active canonical message by its stable source message ID. It can return a technical message; IC/reference lookup deliberately remains limited to trusted user and assistant roles.

## `GET /v1/messages/{message_id}/context`

Requires the same explicit `view` values as the messages endpoint. Optional `before` and `after` values default to 2 and are capped at 50.

```json
{
  "view": "current-thread",
  "before_requested": 2,
  "after_requested": 2,
  "center": { "message_id": "..." },
  "before": [],
  "after": []
}
```

The context uses the same branch semantics and ordering as the requested message view.

## `GET /v1/search`

Query parameters are `q`, `limit`, and `cursor`. Results are message-level hits with the conversation identity, excerpt, model metadata, node/parent branch information, visibility classification, asset references, and stable references when the hit is a trusted public-role message.

## `GET /v1/references/{reference}`

Resolves either the human reference (`[IC:42 | msg:message-id]`) or the token form (`ref:ic/42/uuid/message-id`). The response includes the requested form, the canonical human and token forms, and the resolved message.

## Compatibility boundary

GPTExtractor may consume an explicit RecallEngine projection or a selected RecallEngine database, but it does not define this contract and it does not select the canonical database implicitly.
