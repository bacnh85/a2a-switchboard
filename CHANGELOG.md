# Changelog

All notable changes to this project are documented in this file.

## [Unreleased]

### Fixed

- **Dashboard KPIs survive restarts**: the routing ring is now seeded from
  the `routing.jsonl` tail at boot — routed/errors/latency and the flow log
  no longer reset to zero on every restart.
- **Peer detail "Last seen"** no longer renders the timestamp twice.
- **Health states are three-state**: an unprobed peer renders as muted
  "unknown" instead of a red "unreachable"; peers/table tooltips carry the
  last probe error and last-ok time (new `last_probe_ts`/`last_ok_ts` peer
  fields, serde-defaulted).
- **Reverse channel "no"** is now neutral muted "direct" instead of an error
  badge (absence of a channel is normal for directly-reachable peers).
- **Timestamps localize in the browser**: server-rendered UTC stays as the
  no-JS fallback; JS rewrites log times to local time-of-day and last-
  seen/registered stamps to relative ("2m ago"), full date in the tooltip.

### Changed

- **Dashboard KPIs are windowed RED**: routed (last hour, with req/min),
  errors with error-rate %, p95 latency (with p50), pending, and a fleet
  row (peers healthy x/y · reverse channels); a quiet hour falls back to
  "N in history". Topology and the communication log sit side-by-side at
  ≥1200px; a legend explains the health dots.
- **Human-readable units** in all audit tables and KPIs: `5.4 s` / `3m 05s`
  latencies, `1.2 kB` sizes; new **Task** column shows the A2A task state
  (quiet by default, danger on failed/rejected, warning on input-required);
  HTTP-only rows render muted `POST` next to RPC methods; log times show
  time-of-day with the full date in the tooltip.
- **Peers table**: new **Activity** column (requests/hour from the ring),
  **Admitted** column (bootstrap token / manual), wider URL column, and a
  sticky action column so Accept/Revoke stay reachable while the table
  scrolls horizontally.
- **Peer detail**: structured **Agent card** section (name, version,
  provider, description, capability chips, skills list — string or object
  skill arrays both render) replaces the raw JSON dumps (raw JSON stays
  under a disclosure); new **Gateway identity** section shows the peer's
  masked caller token with Reveal/Copy and the `POST /peer/<name>/` path.
- **Topology** node pills size to their labels (no mid-name ellipsis on
  moderately long names) and the canvas scales/scrolls instead of clipping.
- **Chat**: load failures render as a warning banner with guidance plus a
  live/reconnecting indicator in the thread header.
- **Copy**: single term "human operator" (pill "operator", settings button
  "Create operator"); log captions no longer expose server file paths.

## [0.7.1] - 2026-09-08

### Changed

- **Chat**: messenger-style typing indicator — a lone three-dot bubble
  appears while the send request waits on an agent peer or room (cleared
  when the reply lands, on delivery failure, or after 65s; static dots
  under `prefers-reduced-motion`). Delivery ticks are now a tight
  overlapping SVG double-check — the text `✓✓` glyph gap is gone.
- **A2A v1.0 interop**: the gateway agent and the chat mirror now accept
  the v1.0 `SendMessage` method alongside the pre-1.0 `message/send`
  alias; gateway replies carry `kind: "task"`. Proxied v1.0 exchanges
  mirror into the messenger like alias traffic.
- **Chat hardening**: roster notifications are bounded by the same 60s
  fanout timeout as room sends (a stalling agent can no longer pin
  room-create/member-add); room creation is rate-limited (30/min per IP).
- **Reply extraction**: peer replies wrapped as a v1.0 `{"task": …}` /
  `{"message": …}` result (and bare message results) now mirror into chat —
  previously only the flat pre-1.0 shape extracted, so modern clients'
  room/DM replies showed as "(no reply)".
- **Room dialog**: the "New room" dialog closes on outside click or Escape
  (in addition to closing on successful create).

## [0.7.0] - 2026-09-08

### Added

- **Human peers**: operator identities created in Settings (name → minted
  token, shown with Reveal/Copy). Stored as `kind: human` peers —
  auto-accepted, never probed, excluded from the directory. The token is a
  per-peer caller token: it authenticates `/peer/*` calls and attributes
  them to the human's name in the routing log.
- **Gateway agent (talk to the switchboard itself)**: the reserved name
  `gateway` is answered by a built-in zero-dep A2A agent at
  `/peer/gateway/` — commands `/help`, `/peers`, `/rooms`, `/whoami`, and a
  friendly ack otherwise. The name is now rejected at `/register`.
  Directory-based clients discover it via a `gateway` self-entry in
  `/.well-known/agent.json`.
- **Messenger UI (`/chat`)**: Telegram-style two-pane chat — conversation
  list (rooms + DMs with last-message previews), per-node identity colors
  (8-color palette, stable by name hash), native-emoji composer with a
  picker popover, delivery ticks, unread badges, live appends over a new
  SSE `chat` event, mobile single-pane collapse. "Chat as" selector picks
  the human identity; peers page links open `?dm=<name>`.
- **Chat store**: `data/chat.jsonl` (append + 16 MB rotation, 0600) +
  in-memory ring (2000) + SSE broadcast. Proxied `message/send` exchanges
  are mirrored as DM bubbles (request text from `params.message.parts`,
  reply text from `result.artifacts`); captured traffic honors
  `AGW_AUDIT_PREVIEWS=false`, human/room/gateway messages are always kept.
  State persists across restarts (rooms in `state.json`, ids via the
  chat.jsonl tail).
- **Rooms**: create from the UI with member picker; room sends fan out to
  agent members concurrently (60 s per member) as `[room] sender: text`
  message/send calls; member replies and delivery failures become bubbles;
  roster changes notify added members and are recorded as system events.
- New JSON API under the admin session: `GET /api/chat/state`,
  `GET /api/chat/messages`, `POST /api/chat/send`, `POST /api/chat/rooms`,
  `POST /api/chat/rooms/{id}/members`.

### Changed

- Directory (`/.well-known/agent.json`) lists the `gateway` self-entry for
  authenticated callers; human peers are never listed.
- Channel-delivered calls keep the `channel-` attribution marker only for
  unattributed callers (same behavior as 0.6.x); attributed callers now
  show their name on the channel path too.

## [Unreleased]

### Added

- **Live admin UI for /peers, /logs/full, and peer detail**: registry changes
  (register/accept/reject/revoke/delete) and health flips are broadcast on
  the existing SSE stream as a new `peers` event (separate channel from
  route events; flips only — a stable fleet emits nothing per heartbeat).
  Pages ship server-rendered fragments (`?fragment=1`, same admin gate) that
  a small vanilla client (`assets/live.js`) swaps in on SSE events plus a
  30s drift poll — debounced, diff-checked (no-op when unchanged), and
  skipped while the region holds focus or user-opened state (open
  `<details>`, expanded rows). The filter form never swaps, so in-progress
  inputs are never yanked. Dashboard live behavior is unchanged.

## [0.6.2] - 2026-09-02

### Added

- **Response-side audit**: routing entries now capture the A2A task
  lifecycle state (`result.status.state`, or `error` on a JSON-RPC error
  response) and a redacted, capped preview of the response result
  (`resp_preview`, `task_state` in `routing.jsonl`). Old log lines keep
  parsing (serde defaults). Honors `AGW_AUDIT_PREVIEWS=false` for previews;
  the state field is metadata and is always kept.
- **Prometheus `/metrics`** (text format v0.0.4): token-gated — localhost
  always allowed; remotely requires a gateway, bootstrap, or peer caller
  token. Exposes `a2a_switchboard_requests_total{src,dst,method,status}`,
  `a2a_switchboard_peers{state}`, `a2a_switchboard_channels`,
  `a2a_switchboard_uptime_seconds`, `a2a_switchboard_build_info{version}`.
- **Peer detail page audit parity**: the per-peer traffic table now has
  expandable rows (request + response preview, task state, RPC id) shared
  with the Logs tab, plus a direction filter (`?dir=in|out`) with matching
  deep links into `/logs/full?src=…`/`?dst=…`.
- **Deployment runbook** for validate-before-image binary deploys
  (`docs/integrations/remote-binary-deploy.md`).

## [0.6.1] - 2026-08-20

### Fixed

- **Admin login lockout for upgraded deployments**: 0.6.0's legacy-credential
  detection looked for a `sha256$` marker that never shipped; real 0.5.x
  `state.json` stores a bare 64-hex salted SHA-256, which fell through to the
  argon2 parser and failed. Legacy credentials are now detected by the
  absence of the `$argon2` PHC prefix; the transparent upgrade-on-login
  re-hash is preserved. (`b3862bc`)

## [0.6.0] - 2026-08-19

### Security (fixes #4, #5)

- **No unauthenticated admin window (issue #4)**: the first-run admin
  password is generated at startup and logged once (same pattern as the
  tokens); the setup form, its RFC1918 "local" gate, and the associated
  takeover vector are gone. Changing the password always requires the
  current one.
- **argon2id password hashing (issue #5)**: legacy single-iteration SHA-256
  credentials are transparently upgraded on first successful login.
- **State files are 0600** (`state.json`, `routing.jsonl`) — tokens and
  audit data are no longer world-readable.
- **Directory gated (issue #5)**: `/.well-known/agent.json` requires a
  valid token (gateway, bootstrap, or peer `caller_token`) to list peers;
  unauthenticated callers get the gateway card with an empty `peers` array.
- **CSRF Origin check on admin POSTs** (issue #5) — mismatched `Origin`
  headers are rejected.
- **Rolling-window rate limiter (issue #5)** — no fixed-window boundary
  burst; `agent.json` is now throttled.
- **Routing log rotation (issue #5)**: size-capped
  (`AGW_ROUTING_LOG_MAX_MB`, default 64 MiB) with rotation to `.1`;
  previews can be disabled (`AGW_AUDIT_PREVIEWS=false`).
- **Session cookies**: optional `Secure` flag via `AGW_COOKIE_SECURE`
  for TLS-terminated deployments.
- **Docker runs as a non-root user** (UID 1000); compose publishes on
  loopback by default.

### Changed

- `App::load`/`Config` gained the `cookie_secure`, `routing_log_max_mb`,
  and `audit_previews` settings (config.toml or env).

## [0.5.0] - 2026-08-17

### Added

- **`PATCH /register` — partial self-service peer update**: peers refresh
  their `url` (IP changes), `card` (skills/capabilities), or `upstream_token`
  without re-sending the full registration body. Authenticated by the original
  registration token (fingerprint-matched) **or** the peer's own `caller_token`
  (previously unusable for updates). Admission state is never changed by
  PATCH; revoked peers are rejected (`403`); another identity's token → `409`.

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
