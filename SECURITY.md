# Security policy

## Supported version

RecallEngine is currently an alpha project. Security fixes, when available, are made against the latest code on the default branch.

## Reporting a vulnerability

Please do not disclose suspected vulnerabilities in a public GitHub issue. Before this repository is made public, its maintainer must enable GitHub private vulnerability reporting or add a monitored security contact to this policy. Until one of those reporting paths exists, do not open a public security issue.

## Local data and network exposure

ChatGPT exports and the SQLite databases created by RecallEngine can contain highly sensitive personal and business data. Keep them out of Git, issue trackers, logs, screenshots, and public file-sharing services.

`recall serve` has no authentication or TLS. It binds to `127.0.0.1:8788` by default and should remain on a trusted local interface. Do not expose it directly to a LAN, the internet, a reverse proxy, or an untrusted user.

## Logging and diagnostics

The server is intentionally quiet about data: it does not log successful requests, message content, search parameters, JSON payloads, or full sensitive asset paths. A missing IC is an expected client-side `404`, not an operational fault. Startup and other operational failures are surfaced through the CLI/process error output, while HTTP errors are returned to the caller. Do not add request or payload logging around production exports without an explicit redaction policy.
