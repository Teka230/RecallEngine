# RecallEngine web explorer — M0.2

This directory contains the first read-only web surface for RecallEngine.

## Boundary

- The browser talks only to the versioned `/v1` read API.
- It never reads SQLite directly.
- It does not call GPTExtractor.
- It exposes no import, mutation, migration, or export action.
- `recall serve` still validates and opens the selected RecallEngine database read-only before listening.

The frontend is intentionally dependency-free for this slice. `index.html`, `styles.css`, and `app.js` are embedded into the Rust binary with `include_str!` and served from the same Axum origin as `/v1`. This keeps M0.2 focused on validating the read contract; a later framework migration can preserve the same HTTP boundary.

## M0.2 flows

The explorer supports:

1. text search through `/v1/search`;
2. direct stable-reference resolution through `/v1/references/...`;
3. conversation loading with cursor-paginated messages;
4. explicit `current-thread` / `all-active` switching;
5. optional display of technical messages;
6. message inspection with IC, message ID, model, node/parent, timestamp and stable reference;
7. bounded before/after context through `/v1/messages/{id}/context`;
8. copying a stable reference from the inspector.

If a referenced/search result is outside the current thread, the explorer switches explicitly to `all-active` before showing its context.

## Non-goals

M0.2 deliberately excludes Markdown rendering, write actions, import controls, GPTExtractor actions, semantic search, authentication, and remote deployment. The loopback-only safety rules from `recall serve` remain unchanged.
