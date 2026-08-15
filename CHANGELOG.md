# Changelog

All notable changes to this project are documented in this file.

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

## [Unreleased]

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
