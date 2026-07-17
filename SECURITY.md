# Security policy

## Supported version

RecallEngine is currently an alpha project. Security fixes, when available, are made against the latest code on the default branch.

## Reporting a vulnerability

Please do not disclose suspected vulnerabilities in a public GitHub issue. Prefer [GitHub private vulnerability reporting](https://docs.github.com/en/code-security/security-advisories/guidance-on-reporting-and-writing-information-about-vulnerabilities/privately-reporting-a-security-vulnerability) for this repository when available. If that path is unavailable, contact the maintainer privately instead of opening a public security issue.

## Local data and network exposure

ChatGPT exports and the SQLite databases created by RecallEngine can contain highly sensitive personal and business data. Keep them out of Git, issue trackers, logs, screenshots, and public file-sharing services.

`recall serve` has no authentication or TLS. It binds to `127.0.0.1:8788` by default and should remain on a trusted local interface. Binding outside loopback (for example `0.0.0.0` or a LAN address) requires an explicit `--allow-remote` flag and prints a privacy warning. Do not expose it directly to a LAN, the internet, a reverse proxy, or an untrusted user unless you fully understand the risk.

## Logging and diagnostics

The server is intentionally quiet about data: it does not log successful requests, message content, search parameters, JSON payloads, or full sensitive asset paths. A missing IC is an expected client-side `404`, not an operational fault. Startup and other operational failures are surfaced through the CLI/process error output, while HTTP errors are returned to the caller. Do not add request or payload logging around production exports without an explicit redaction policy.
