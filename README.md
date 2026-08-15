# agent-gateway

Self-hosted **A2A (Agent2Agent) gateway** in Rust. Single static binary: peer
admission control, token exchange, traffic brokering, admin UI with routing
log and live communication graph.

```
cargo run --release        # → http://127.0.0.1:9920
```

## What it does

- **Gateway API token** — peers authenticate with it; unknown peers land in a
  **pending queue**, visible in the admin UI.
- **Bootstrap token** — registrations presenting it are **auto-accepted**.
- **Deny-by-default proxying** — traffic is only ever forwarded to *accepted*
  peers' pinned URLs (`/peer/{name}/...`). No URL from a request body is ever
  fetched (SSRF-safe by construction).
- **Directory as Agent Card** — `/.well-known/agent.json` (alias
  `/.well-known/agent-card.json`) lists accepted peers; auth-aware:
  capabilities/skills only with a valid token.
- **Admin UI** (no auth by design, bind to localhost): dashboard, pending-peer
  accept/reject, live routing log (SSE), communication graph (vis-network).

## Quickstart

```bash
cargo build --release
./target/release/agent-gateway
# First run prints:
#   gateway API token : agw_...
#   bootstrap token   : agw_...
```

### Register a peer (gateway token → pending)

```bash
curl -X POST http://127.0.0.1:9920/register \
  -H "Authorization: Bearer $GATEWAY_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"name":"hermes","url":"http://192.168.1.20:9900/",
       "card": {...optional agent card...},
       "upstream_token": "optional-token-gateway-uses-when-calling-this-peer"}'
# → {"status":"registered","peer":"hermes","state":"pending"}
# Accept it in the UI at http://127.0.0.1:9920/peers
```

With the **bootstrap token** instead, the same call returns
`"state":"accepted"` immediately.

### Call a peer through the gateway

Any A2A JSON-RPC call works — just point the client at the gateway path:

```bash
curl -X POST http://127.0.0.1:9920/peer/hermes/ \
  -H "Authorization: Bearer $GATEWAY_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"message/send","params":{...}}'
```

### pi-a2a (Pi) peer config example

```jsonc
{
  "a2a": {
    "peers": {
      "gateway": { "url": "http://127.0.0.1:9920/peer/hermes", "auth": { "bearer": "<GATEWAY_TOKEN>" } }
    }
  }
}
```

## Configuration

`config.toml` (optional) or env overrides:

| Key | Env | Default |
|-----|-----|---------|
| `server.bind` | `AGW_BIND` | `127.0.0.1:9920` |
| `server.data_dir` | `AGW_DATA_DIR` | `data/` |
| `server.heartbeat_sec` | `AGW_HEARTBEAT_SEC` | `30` |

State: `data/state.json` (peers + tokens, atomic write). Routing log:
`data/routing.jsonl` (append-only, metadata only — never message bodies).

## Security model

- Tokens compared in **constant time** (`subtle`).
- Egress **only** to accepted peers' pinned URLs; redirects never followed.
- URL scheme allowlist (http/https) at registration.
- Per-IP rate limits: 20 req/min registration, 120 req/min proxy.
- Routing log stores metadata only (src/dst/method/status/bytes/latency).
- Admin UI is **unauthenticated by design** → default bind is `127.0.0.1`;
  a warning banner + startup warning appear when bound wider. Put a TLS
  terminator (Caddy/nginx) in front for remote access.

### Known limitations (by design, v2 candidates)

- One shared peer token ⇒ an accepted peer can impersonate another on the
  wire. Attribution in the log is token-class level. Per-peer tokens are the
  planned v2 (`peerTokens` support already exists in pi-a2a clients).
- No admin authentication.
- Plain HTTP; terminate TLS externally.

## Development

```bash
cargo test               # 8 integration tests: admission, proxy, SSE, rate limit
cargo clippy --all-targets -- -D warnings
```

Stack: axum, tokio, askama (compiled templates), rust-embed (htmx, SSE ext,
vis-network vendored — zero runtime CDN), subtle, reqwest.
