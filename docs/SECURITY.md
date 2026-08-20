# Security

This document is the threat model for a2a-switchboard. It states what the
system guarantees, what it explicitly does not, and how each decision maps to
code.

## Trust model

- The **operator** (you) is trusted: admin UI, tokens, admission decisions.
- **Accepted peers** are semi-trusted: they can send traffic to each other
  through the switchboard, but cannot read each other's credentials or
  hijack in-flight requests.
- **Unknown callers** are untrusted: they can attempt registration and
  probing; nothing more.

## Token classes

| Token | Power |
|---|---|
| Gateway token | Register (→ pending), proxy through accepted peers, read directory |
| Bootstrap token | Same as gateway + auto-accept on registration |
| Peer caller token | Per-peer, minted at registration: proxy, **and manage its own peer** (PATCH / DELETE / re-register / channel-open). Never grants any other peer. |

- Both shared tokens are compared **in constant time** (`subtle` crate) — no timing oracle.
- Tokens are stored plaintext in `state.json` (back it up as a secret) and
  shown in Settings. They are **never** written to the routing log, the Agent
  Card, or envelope payloads.

**Per-peer management (issue #3 fix):** once a peer holds a caller token,
that token is the ONLY credential that may PATCH, DELETE, re-register, or
open/replace that peer's channel — not even the shared gateway/bootstrap
tokens (which every peer may hold) can act on it. Shared tokens still work
for legacy entries registered before caller tokens existed (fingerprint must
match the registering token). A peer that loses its caller token recovers via
the operator (admin UI delete → re-register), never via a shared token.

## SSRF posture (deny-by-default egress)

- The proxy contacts **only** accepted peers' pinned registration URLs —
  never URLs from request bodies (webhooks/file parts pass through untouched).
- Redirects are never followed (`redirect::Policy::none`).
- Registration URL allowlist: `http`/`https` only, must have a host.
- Channel delivery resolves **only** same-origin relative URLs against the
  switchboard's own origin; the client rejects `..` paths and non-rooted paths
  before any local fetch.

## Reverse channel security

- `GET /channel?name=<peer>` binds the connection to the **name** (required)
  and authorizes it per-peer: the peer's own caller token, or (legacy entries
  without one only) the exact shared token that registered the peer. A shared
  token held by another registrant can no longer replace a peer's live
  channel (issue #3).
- Each connection gets a random 32-byte **`chan_secret`** (delivered in the
  `hello` event). Every response must echo it. Because pending requests are
  keyed by (peer, secret), a shared-token peer **cannot** answer another
  peer's requests — verified by the `channel_impersonation_rejected` test.
- Envelopes carry no credentials; the switchboard substitutes the destination
  peer's `upstream_token`.
- Correlation ids are single-use with a 600s deadline; unknown/foreign ids
  return 404.
- Body size cap 4MiB enforced **before** base64 decode on both sides (OOM
  guard — verified by `channel_oversized_response_rejected`).
- Hop-by-hop headers (`connection`, `keep-alive`, `te`, `trailers`,
  `transfer-encoding`, `upgrade`, proxy-*) and auth-challenge headers
  (`set-cookie`, `www-authenticate`) are stripped from channel responses.

## Proxied HTTP

- Caller `Authorization` / `X-Gateway-Token` are never forwarded; the peer's
  `upstream_token` is substituted when registered.
- Response filtering matches the channel path (hop-by-hop stripped).
- Request size cap 4MiB; timeout 600s (long agent tasks pass).

## Admin UI

- **Never unauthenticated (0.6.0+)**. The first-run admin password is
  generated at startup and printed to the logs once (same pattern as the
  tokens); the UI is gated by `require_admin` from boot — there is no
  unauthenticated setup window (issue #4). Changing it requires the current
  password; the first-set-from-LAN form and its RFC1918 "local" heuristics
  are gone.
- Passwords are stored as **argon2id** PHC strings; legacy single-iteration
  SHA-256 entries are transparently upgraded on first successful login.
- Session cookies: `HttpOnly` + `SameSite=Lax`; add `Secure` with
  `AGW_COOKIE_SECURE=1` when a TLS terminator fronts the gateway. Sessions
  are fresh random tokens per login, TTL 12h, dropped on logout.
- State files (`state.json`, `routing.jsonl`) are created **0600**.
- CSRF: state-changing POSTs on the admin router reject mismatched `Origin`
  headers (SameSite=Lax covers the rest).

## Logging hygiene

- Routing log (`routing.jsonl`) stores **metadata only**: ts, src, dst,
  method, status, bytes, latency. Never message bodies, never tokens.
- Audit trail for the admin actions is in-process; no PII captured.

## Rate limits

Per-IP **rolling 60s window**: 20/min registration, 120/min proxy, channel
responses, and `/.well-known/agent.json`. No fixed-window boundary burst.
Channel queue depth 256; excess is 503 (retryable), not a silent drop.

## Reporting

See CONTRIBUTING.md — security findings go to the maintainer privately.
