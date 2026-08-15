# Development

## Build & test

```bash
cargo build --release              # static binary with embedded UI
cargo fmt --check                  # formatting
cargo clippy --all-targets -- -D warnings
cargo test --test gateway -- --test-threads=1   # SSE/broadcast tests are timing-sensitive
```

The test suite is 12 integration tests covering admission, proxy, channel
roundtrip, impersonation resistance, and size caps — using `tower::ServiceExt::oneshot`
against the router with two fake in-process peers.

## Architecture

```
src/
├── main.rs     — entry, config load, token first-run echo, router assembly
├── lib.rs      — crate root: router() + embedded asset serving
├── config.rs   — config.toml + AGW_* env overrides
├── state.rs    — App (tokens, peers, routing-log ring, rate limiter, channels),
│                 persistence (atomic state.json), RouteEntry
├── auth.rs     — constant-time token classify, ClientIp extractor, error bodies
├── peers.rs    — /register (pending|auto-accept), .well-known directory,
│                 dual-mode proxy (direct HTTP | channel), deregister
├── channel.rs  — reverse channel: Channels registry (mpsc + per-conn secret),
│                 /channel SSE, /channel/response, CleanupStream, size caps
├── health.rs   — periodic probe; live channel = healthy
└── admin.rs    — dashboard/peers/logs/graph/settings pages + SSE + actions
templates/      — Askama compiled templates (inherits layout.html)
assets/         — app.css (token system), graph.js, vendored htmx/vis-network
tests/          — integration tests
```

Data flow for a proxied call:

```
caller → POST /peer/x → peers::proxy → channels.has(x)?
   yes → envelope (mpsc) → peer's SSE → POST /channel/response → oneshot → reply
   no  → reqwest to pinned url → reply (headers filtered) → reply
both paths log a RouteEntry (ring + routing.jsonl + SSE broadcast)
```

## How to add a feature

1. Add the route in `lib.rs::router()`.
2. Implement the handler in the right module; keep the security invariants:
   auth before work, size caps before decode, metadata-only logs.
3. Add a template if it's a UI page (extend `layout.html`; use the token
   system in `assets/app.css`).
4. Add an integration test in `tests/gateway.rs` (oneshot + fake peer).
5. `cargo fmt && cargo clippy && cargo test`.

## Debugging

- `RUST_LOG=debug ./a2a-switchboard` — verbose request tracing.
- `data/routing.jsonl` — every routed exchange (metadata only).
- `data/gateway.log` (if you redirect stdout) — startup + channel events.
- Channel drop detection: `channel drop: <name>` in logs.

## Release checklist

1. Bump version in `Cargo.toml` + CHANGELOG.
2. `cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test --test gateway -- --test-threads=1`.
3. Commit + tag: `git tag -s vX.Y.Z` (SSH-signed tags preferred).
4. `git push origin main --tags` — the Release workflow builds the multi-arch
   image and pushes to `ghcr.io/bacnh85/a2a-switchboard`.
5. Optionally attach release binaries from a local `cargo build --release`.
