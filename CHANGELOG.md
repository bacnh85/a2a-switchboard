# Changelog

All notable changes to this project are documented in this file.

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
