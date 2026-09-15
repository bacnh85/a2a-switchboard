# Development

## Build & test

The console is a Preact + TypeScript SPA in `ui/` (Vite). Node is required to
build it; `build.rs` stubs `ui/dist/` so plain `cargo build`/`test` still work
without Node.

```bash
# console UI (once, and after any ui/ change)
npm --prefix ui ci
npm --prefix ui run build          # typecheck + bundle into ui/dist
npm --prefix ui test               # vitest unit tests (formatters, markdown)

# backend
cargo build --release              # static binary with embedded UI
cargo fmt --check                  # formatting
cargo clippy --all-targets -- -D warnings
cargo test --test gateway -- --test-threads=1   # SSE/broadcast tests are timing-sensitive
```

UI live-reload development: run `cargo run` and `npm --prefix ui run dev`
(Vite on :5173, proxying `/api` to :9920) side by side; release builds embed
`ui/dist/` via rust-embed, so `cargo run` in debug picks up `npm run build`
output without a recompile.

The integration suite (~70 tests in `tests/gateway.rs`) covers admission,
proxy, channel roundtrip, impersonation resistance, size caps, the JSON API,
the task inbox (derive/reply/cancel), chat history/typing, notifications, and
the summary windows — using `tower::ServiceExt::oneshot` against the router
with fake in-process peers.

## Architecture

```
src/
├── main.rs     — entry, config load, token first-run echo, router assembly
├── lib.rs      — crate root: router() + embedded ui/dist serving + SPA shell
├── config.rs   — config.toml + AGW_* env overrides
├── state.rs    — App (tokens, peers, rings, rate limiter, channels, task
│                 store), persistence (atomic state.json), RouteEntry
├── auth.rs     — constant-time token classify, ClientIp extractor, error bodies
├── peers.rs    — /register (pending|auto-accept), .well-known directory,
│                 dual-mode proxy (direct HTTP | channel), deregister
├── channel.rs  — reverse channel: Channels registry (mpsc + per-conn secret),
│                 /channel SSE, /channel/response, CleanupStream, size caps
├── chat.rs     — messenger: built-in gateway agent, rooms, history/typing,
│                 human identities; text extraction from A2A message/send
├── tasks.rs    — A2A task inbox: derive from routed message/send, list/
│                 detail/reply/cancel endpoints
├── api.rs      — admin JSON API: /api/summary, /api/peers, /api/logs,
│                 /api/notifications, /api/settings + admission actions
├── health.rs   — periodic probe; live channel = healthy
└── admin.rs    — SSE stream, /metrics, JSONL export, legacy form actions
ui/              — console SPA (Preact + TypeScript + Vite; built to ui/dist,
                 embedded via rust-embed)
tests/           — integration tests
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
