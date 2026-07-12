# Contributing to RecallEngine

Thanks for helping improve RecallEngine. This project is a small, local-first Rust alpha; focused bug reports, tests, and documentation corrections are especially useful.

## Before opening a pull request

- Keep changes scoped to the ChatGPT-export-to-SQLite engine.
- Do not add personal exports, SQLite databases, binaries, screenshots, API keys, or other sensitive material to the repository.
- Use only anonymized and minimal fixtures. If a fixture is needed, explain what it covers and make sure it contains no real conversations, names, identifiers, or attachments.
- Preserve the stable `message.id` and durable local `messages.ic` contract.

## Local checks

From the repository root, run:

```bash
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test
cargo run --bin recall -- --help
```

Please include tests for behavior changes and update the README or relevant documentation when the public CLI or data contract changes.

## Reporting bugs

Open an issue with the RecallEngine version or commit, your operating system and Rust version, the command you ran, the expected result, and a sanitized error or minimal reproduction. Do not paste private conversation content, exports, or databases into public issues.
