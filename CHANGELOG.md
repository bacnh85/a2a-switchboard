# Changelog

All notable changes to this project are documented in this file.

## [0.4.0] - 2026-08-16

### Added

- **Message-level audit trail**: every proxied JSON-RPC call now records the
  RPC method, request id, and a redacted 2KB-capped preview of `params` in
  `routing.jsonl` (secret-looking keys auto-redacted; non-JSON bodies capture
  nothing). Click a dashboard/log row to inspect. Based on RED-method / Grafana
  operator-console patterns.
- **Dashboard RED stats**: routed (ring), errors, avg latency, pending — all
  clickable (directed browsing), live-updating from SSE.
- **Logs**: method filter (JSON-RPC or HTTP), errors-only filter, JSONL export
  (`/logs/export`) honoring the same filters.

### Fixed

- Logs page filter form targeted `/logs` (live ring view) so query-string
  filters never applied; now targets `/logs/full`.

## [0.3.0] - 2026-08-16

### Added

- **Peer detail pages** (`/peers/{name}`): full agent-card inspection —
  capabilities and skills rendered as pretty JSON, raw card (collapsible),
  identity/liveness metadata (registered, last seen, source IPs, last
  error, reverse-channel state), and per-peer traffic history with ok/err
  counts read back from `routing.jsonl`.
- **Full communication log audit** (`/logs` + `/logs/full`): the routing
  log now reads the persistent `routing.jsonl` (not just the 1000-entry
  in-memory ring) with substring filters on caller/destination and exact
  status match. The logs page gained a filter bar; history pages out the
  full JSONL trail.
- **Peer source-IP capture**: the switchboard records `reg_ip` (address a
  peer registered from) and `last_ip` (address of its most recent
  successful exchange — proxied request or reverse channel). Display-only,
  never used for auth.

### Changed

- Peer list (pending + accepted) shows the peer's source IP and
  last-seen/registered as local date-time (`YYYY-MM-DD HH:MM:SS`) instead
  of raw unix epochs. Peer names link to their detail page.
- Dashboard communication log and routing-log tables show local date-time
  timestamps.
- `RouteEntry` is now deserializable so the JSONL audit trail can be read
  back into the UI.

### Fixed

- First-time admin password set now works behind podman/docker port
  publishing: the socket source IP is the container bridge gateway (e.g.
  10.88.0.35), never 127.0.0.1, so the localhost-only gate rejected it. The
  gate now accepts loopback plus RFC1918 private ranges (10/8, 172.16/12,
  192.168/16 — covering podman's 10.88/16 and docker's 172.17/16 bridges),
  including IPv4-mapped IPv6 (::ffff:a.b.c.d).

## [0.2.0] - 2026-08-15

### Added

- Optional admin password: set/change from Settings (initial set requires a
  localhost connection); salted-hash in `state.json`, in-memory 12h cookie
  sessions, login rate-limiting (5/60s per IP). Auth is off until set.
- **Live dashboard flows + communication log**: routed requests now animate
  as packets traveling caller → gateway → destination (and back) on the
  topology, with a live `from → to · method · status · ms` log beside it,
  driven by a plain `EventSource` on `/api/events`.
- **`X-Gateway-Caller` header** (advisory, display-only): callers may declare
  a display name for the routing log/dashboard; it is clamped, stripped
  before forwarding, and not an auth mechanism.
- **Per-peer caller tokens**: `/register` now issues each peer a unique
  `caller_token`, returned once when issued (registration or first
  post-upgrade heartbeat) and never re-disclosed on later heartbeats (if
  lost, deregister + re-register). Presenting it on `/peer/*` calls
  authenticates AND attributes the caller to the peer's name — no header
  needed, works even for raw curl. Shared-token impersonation risk reduced.

### Changed

- Admin UI redesigned: fixed left sidebar navigation, dashboard now shows a
  **live routing topology** — peers around the central gateway with edges that
  light up as requests route (SSE-driven) — replacing the vis-network graph
  page (`/graph` removed, vendored `vis-network` dropped, ~120 lines of
  vanilla SVG/JS instead).
- Recent-routing list now prepends live entries.

### Fixed

- `ClientIp` extractor read the wrong extension type, so live per-IP rate
  limiting keys silently saw `unknown` in production (tests injected the raw
  extension and masked it). Now reads `ConnectInfo<SocketAddr>` (with
  fallback).

## [0.2.1] - 2026-08-15

### Fixed

- No-password warning banner on non-localhost binds no longer consumes the
  whole page: moved inside the content area, shown only when no admin
  password is set, with a direct "Set a password" link to Settings.

## [0.1.1] - 2026-08-15

### Changed

- **Renamed to `a2a-switchboard`** (from `agent-gateway`) to avoid a direct
  name collision with the solo.io `agentgateway` project. Binary, crate, UI
  branding, and agent-card identity all updated.
- Env-var prefix (`AGW_*`) intentionally **unchanged** in this release;
  renaming to `SWB_*` is scheduled for 0.2.0.

### Added

- Reverse channel for firewalled peers (`GET /channel?name=`, envelope +
  response protocol, per-connection `chan_secret` binding).
- GitHub Actions CI (fmt/clippy/test) and Release (multi-arch GHCR image)
  workflows. Dockerfile + docker-compose example.
- `docs/` guides: DEPLOYMENT, INTEGRATION (incl. reverse-channel spec),
  SECURITY, DEVELOPMENT. CHANGELOG, CONTRIBUTING, LICENSE.

## [0.1.0] - 2026-08-15

### Added

- Gateway token auth → pending peer queue; bootstrap token → auto-accept.
- Deny-by-default reverse proxy to accepted peers' pinned URLs.
- Auth-aware Agent Card directory (`/.well-known/agent.json`).
- Admin UI: dashboard, pending peers, live SSE routing log, vis-network
  communication graph, settings (token display/rotate).
- Reverse channel MVP for firewalled peers (later hardened in 0.1.1).
- 12 integration tests (admission, proxy, channel, security).
