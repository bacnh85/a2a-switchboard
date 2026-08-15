# Agent Gateway — Self-Hosted A2A Broker with Admission Control (Rust)

## Goal

A single-binary, self-hosted A2A gateway in Rust that:

1. Issues a **gateway API token** — peers add it to their a2a config (e.g. `a2a.peers.<gateway>.auth`) to authenticate.
2. **Deny-by-default admission**: unknown peers land in a *pending* queue; admin accepts/rejects via UI.
3. **Bootstrap token**: a special token that auto-accepts any peer presenting it.
4. **Brokers A2A traffic** between accepted peers (reverse proxy, so peers on different networks only need to reach the gateway).
5. Serves the **peer directory as an Agent Card** at `.well-known/agent.json` (also aliases `.well-known/agent-card.json` for A2A v1.0 canonical path).
6. Admin UI: dashboard (connected/health), pending peers, routing log (live), communication graph. No admin auth for now (localhost bind + warning banner instead).

## Research findings

### Existing solutions & the gap

| Project | What it is | What it lacks for our use case |
|---|---|---|
| [agentgateway/agentgateway](https://github.com/agentgateway/agentgateway) (ref) | Rust proxy for LLM+MCP+A2A, config-driven YAML, Kubernetes/Gateway API, istio-style xDS | No interactive peer admission (pending/accept), no bootstrap token, no comm-graph/routing-log UI, heavyweight k8s-oriented deployment |
| Apicurio Registry A2A ([discussion #741](https://github.com/a2aproject/A2A/discussions/741)) | Agent Card registry w/ search, versioning, RBAC visibility | Discovery-only registry; no traffic brokering, no admission queue |
| mcp-contextforge-gateway (IBM) | Python gateway, JWT/SSO/RBAC, OTel | Python (not the perf goal), config-driven, no admission UI |
| ra2a / a2a-rs (Rust SDKs) | A2A v1.0 client/server SDKs | Libraries, not gateways — usable as reference for types only |

**Nobody has**: manual admission queue + bootstrap auto-accept + live comm graph + routing log + directory-as-agent-card in one self-hosted static binary. That's our niche.

### Security concerns from research (A2A spec v1.0, Palo Alto A2A risk guide, Red Hat, Tyk)

1. **SSRF** — never fetch URLs from request bodies (webhook/file parts) at the gateway; proxy *only* to registered+accepted peer URLs. Pass push-notification config through untouched.
2. **Agent Card poisoning** — cards are peer-declared; we store and re-publish them as-is but pin the peer's *routing URL* from registration; log card changes (hash) for audit.
3. **Spec: no plaintext secrets in Agent Cards** — our card declares the `apiKey` security *scheme* only; tokens never appear in card JSON.
4. **Token comparison must be constant-time** (`subtle` crate) to avoid timing oracles.
5. **Deny-by-default egress** (SSRF mitigation above) + URL scheme validation (http/https only) at registration.
6. **No admin auth** (user's call): bind `127.0.0.1` by default; loud warning banner + startup log line when bound wider. Flagged as v2: admin token.
7. **Shared peer token** ⇒ an accepted peer can impersonate another accepted peer on the wire. Mitigated for attribution by per-peer identity from registration + routing log; **per-peer issued tokens are the v2 upgrade path** (pi-a2a already supports `peerTokens`).
8. **Logging hygiene** — routing log records metadata only (src, dst, method, status, bytes, latency); never message bodies. PII/secret-safe by construction.
9. Plain HTTP default — self-hosted behind your TLS terminator (reverse proxy); document. rustls pass-through mode = v2.

## Architecture

```
┌────────────────────────── agent-gateway (single binary, axum) ─────────────────────────┐
│ Peer API (auth: gateway or bootstrap token)                                             │
│   POST /register              submit card + declared capabilities → pending/auto-accept │
│   GET  /.well-known/agent.json   gateway card + accepted peer directory (auth-aware)    │
│   ANY  /peer/{name}/*         reverse proxy → registered peer URL (+ its upstream token)│
│                                                                                          │
│ Admin UI (no auth, localhost default)                                                    │
│   /  dashboard (connected, pending, health, throughput)   /peers  accept/reject/revoke   │
│   /logs  live routing log (SSE)                           /graph  vis-network comm graph │
│   /settings  show/regenerate bootstrap token                                              │
│                                                                                          │
│ State: peers+tokens → data/state.json (atomic write) · routing log → data/routing.jsonl  │
│ Live updates: tokio broadcast channel → SSE (htmx hx-sse)                                │
│ Health: background task probes each accepted peer's agent card every 30s + last-seen     │
└──────────────────────────────────────────────────────────────────────────────────────────┘
```

**Stack** (all mainstream, ~8 crates): `axum`, `tokio`, `tower`, `serde`/`serde_json`, `askama`, `rust-embed` (vendor htmx + sse ext + vis-network UMD into the binary — committed to repo, no runtime CDN), `subtle`, `rand`, `tracing`.

**Persistence (lazy but durable)**: JSON snapshot for peer registry (atomic tmp+rename), append-only JSONL for routing log, in-memory ring buffer (last 1000 entries) for the UI. No DB dependency.

**Why proxy-not-relay**: path-based proxying (`/peer/{name}/`) keeps the wire protocol exactly A2A JSON-RPC as peers already speak it — pi-a2a peers just configure peer URL `http://gw:9920/peer/hermes/` and it works, zero SDK changes. Method names (`message/send` vs `SendMessage` version drift) are irrelevant since bodies forward opaquely.

**Peer identity**: registration declares name + agent card + (optional) upstream auth token the gateway uses when proxying to it. Identity key = name (unique); duplicate name registration from different fingerprint → rejected as conflict.

## Implementation steps

1. **Scaffold** Cargo workspace (single bin crate), deps above, `config.toml` + env overrides (`AGW_BIND`, `AGW_DATA_DIR`, `AGW_TOKEN` auto-generated on first run and echoed). Tracing subscriber.
2. **Core state**: `Peer { name, url, card, state: Pending|Accepted|Revoked, fingerprint, upstream_token, last_seen, health }`; registry with RwLock; persistence; routing-log ring buffer + JSONL appender; broadcast channel.
3. **Auth middleware**: constant-time check of `Authorization: Bearer` / `X-Gateway-Token` against gateway token (valid but unknown fingerprint → pending queue) and bootstrap token (→ auto-accept). Simple per-peer fixed-window rate limit (~20 lines, in-memory).
4. **Peer API**: `/register`, auth-aware `.well-known/agent.json` (+ v1.0 alias), `/peer/{name}/*` proxy with body-size limit + timeout, deny-by-default egress (only accepted peers' pinned URLs).
5. **Health prober**: background loop fetching each accepted peer's card; update `health` + `last_seen`; feed SSE.
6. **Admin UI**: Askama layout + 5 pages, htmx actions (accept/reject/revoke/regenerate bootstrap), SSE-driven log/graph/dashboard refresh. **UI discipline**: adopt a medium-tuned preset token set as the implicit DESIGN.md system (hand-rolled ~200-line CSS with variables — 8px grid, one accent, named elevation, full `:hover/:focus-visible/:disabled` states); run `ux_audit` gate on the final CSS before handoff.
7. **Vendor assets**: `assets/vendor/htmx.min.js`, `htmx-sse.min.js`, `vis-network.min.js` committed to repo; embedded via `rust-embed`.
8. **Tests** (assert-based, focused): token compare constant-time correctness, admission state machine (unknown→pending, bootstrap→accepted, reject, revoke→403), proxy end-to-end with two in-memory axum fake peers, auth-aware directory (pending peers hidden from peers), URL validation.
9. **README**: quickstart, pi-a2a peer config example, security notes, TLS-terminator guidance.

## Verification plan

- `cargo test` — unit + integration flows above (incl. two-fake-peer proxy roundtrip).
- `cargo clippy -- -D warnings`.
- Manual smoke script (documented in README): start gateway → curl `/register` with gateway token (appears pending in UI) → accept in UI → second fake peer's card appears in directory → JSON-RPC call via `/peer/x/` succeeds → log + graph update live.
- `ux_audit` pass on the stylesheet (APCA contrast, token coverage, states, no slop tells).

## Risks / non-blocking open questions

- **Shared token impersonation** within accepted peers — documented, per-peer tokens are the v2 fix (pi-a2a `peerTokens` already supports it).
- **No admin auth** — mitigated by localhost default bind + warning; v2 admin token.
- **HTTP-only** — assumed behind reverse proxy TLS; v2 optional built-in rustls.
- Gateway probing peers every 30s on localhost-only setups: fine; config knob `heartbeat_sec` if noisy.
- Rejected: per-peer JWT/OAuth, OTel export, guardrails, k8s controller — what agentgateway already does; out of scope for a self-hosted admission gateway.
