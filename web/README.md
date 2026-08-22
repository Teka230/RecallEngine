# RecallEngine web UI — M0.2

This directory contains the first React/TypeScript read-only web UI for RecallEngine.

## Boundary

- The browser talks only to the versioned `/v1` read API.
- It never reads SQLite directly.
- It does not call GPTExtractor.
- It exposes no import, mutation, migration, or export action.
- `recall serve` still validates and opens the selected RecallEngine database read-only before listening.

## Development

Start RecallEngine on its normal loopback port, then run the Vite dev server:

```bash
cd web
npm install
npm run dev
```

Vite proxies `/v1` to `http://127.0.0.1:8788`, so the frontend keeps the same API boundary during development.

Build the static UI before using it through `recall serve`:

```bash
cd web
npm install
npm run build
```

The build is written to `web/dist/`. `recall serve` serves that directory from the same origin as `/v1`. `RECALL_WEB_DIST` can override the directory for packaging or tests. If the build is absent, `/` returns an explicit `503` instead of silently falling back to another UI implementation.

## M0.2 flows

The UI supports:

1. text search through `/v1/search`;
2. direct IC lookup through `/v1/messages/by-ic/{ic}`;
3. direct UUID lookup through `/v1/messages/{id}`;
4. stable-reference resolution through `/v1/references/...`;
5. cursor-paginated conversation and message loading;
6. explicit `current-thread` / `all-active` switching;
7. optional display of technical messages;
8. message inspection with IC, UUID, model, node/parent, branch, timestamp and assets;
9. bounded before/after context;
10. copying a stable reference;
11. visible loading, empty, error and database-unavailable states;
12. visible API version and `snapshot_id`.

If a direct lookup targets a message outside the current thread, the UI switches explicitly to `all-active` before loading context.

## Checks

```bash
npm run check
```

This runs strict TypeScript checking and a production Vite build.

## Non-goals

M0.2 deliberately excludes Markdown rendering, write actions, import controls, GPTExtractor actions, semantic search, authentication, and remote deployment. The loopback-only safety rules from `recall serve` remain unchanged.
