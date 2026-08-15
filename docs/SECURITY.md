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

- Both are compared **in constant time** (`subtle` crate) — no timing oracle.
- Tokens are stored plaintext in `state.json` (back it up as a secret) and
  shown in Settings. They are **never** written to the routing log, the Agent
  Card, or envelope payloads.

**Known limitation (by design, v2):** peers currently share one token class,
so an accepted peer could present another accepted peer's *declared identity*
on the wire. Routing-log attribution is token-class level. The v2 plan is
per-peer tokens (`peerTokens` already supported by pi-a2a clients).

## SSRF posture (deny-by-default egress)

- The proxy contacts **only** accepted peers' pinned registration URLs —
  never URLs from request bodies (webhooks/file parts pass through untouched).
- Redirects are never followed (`redirect::Policy::none`).
- Registration URL allowlist: `http`/`https` only, must have a host.
- Channel delivery resolves **only** same-origin relative URLs against the
  switchboard's own origin; the client rejects `..` paths and non-rooted paths
  before any local fetch.

## Reverse channel security

- `GET /channel?name=<peer>` binds the connection to the **name** (required —
  shared tokens make fingerprint-only lookup ambiguous) and verifies the
  token's fingerprint matches that peer.
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

- **Unauthenticated by design** (your call for self-hosting). Mitigations:
  default bind `127.0.0.1`; loud warning banner + startup log when bound
  wider; docs push TLS + auth at the terminator.
- v2: admin token + optional OIDC.

## Logging hygiene

- Routing log (`routing.jsonl`) stores **metadata only**: ts, src, dst,
  method, status, bytes, latency. Never message bodies, never tokens.
- Audit trail for the admin actions is in-process; no PII captured.

## Rate limits

Per-IP fixed-window: 20/min registration, 120/min proxy + channel responses.
Channel queue depth 256; excess is 503 (retryable), not a silent drop.

## Reporting

See CONTRIBUTING.md — security findings go to the maintainer privately.
