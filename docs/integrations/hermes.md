# Hermes client integration

Hermes (Nous Research agent runtime, Python) integrates with the switchboard
as an A2A peer. This recipe covers the Hermes client only — the switchboard
side needs zero changes (see `../INTEGRATION.md` for the wire protocol).

> **TL;DR — why the routing log shows `src: bootstrap`:** a Hermes gateway
> client that is directory-only sends the shared token on every `gw/*` proxy
> call, so the switchboard's caller attribution falls back to the token-class
> label (`bootstrap`). Fix: obtain the per-peer `caller_token` (issued by
> `/register`) and present it on proxied calls, or send
> `X-Gateway-Caller: <name>`.

## How attribution works on the switchboard

`caller_display()` (`src/peers.rs`) resolves the caller of a `/peer/<name>/`
proxy call in this order:

1. **Per-peer `caller_token`** as `Authorization: Bearer` → the peer name
   (constant-time match, highest confidence).
2. **`X-Gateway-Caller: <name>`** header → that name (advisory; clamped to
   64 chars; always stripped before forwarding upstream).
3. Otherwise → token-class fingerprint label: `bootstrap` / `gateway` /
   `client-<fp8>`.

Per-peer caller tokens are minted by `POST /register` and are **disclosed only
when minted** — never re-disclosed on later heartbeats. If lost, deregister and
re-register to obtain a fresh one.

## Prerequisites

- The switchboard is reachable from the Hermes host (`a2a_gateway.url`), and
  the Hermes A2A server URL is reachable **from the switchboard host** (bind
  `0.0.0.0`, open firewall). Unreachable instances still register but show
  unhealthy (see "Reverse channel").
- Peer `name` must be 1–64 chars of `[A-Za-z0-9._-]`, unique per instance
  (recommend `<hostname>`). A name claimed by another identity → `409`.

## 1. Config (`~/.hermes/config.yaml`)

```yaml
a2a_gateway:
  url: <GATEWAY_ORIGIN>                    # e.g. http://gateway.example:9920
  token: <shared bootstrap or gateway token>  # registration & directory only
  name: <hostname>                         # this instance's peer name
  upstream_token: <inbound bearer token>   # optional — presented by the
                                           # switchboard when proxying TO you
  caller_token: <per-peer caller token>    # optional — see §2
```

## 2. Obtain the per-peer caller token

**Path A — read the already-minted token (existing peers):** the operator
reads each peer's token on the switchboard host:

```bash
ssh <gateway-host> "python3 -c \"import json;d=json.load(open('<state.json path>'));[print(p['name'],p['caller_token']) for p in d['peers']]\""
```

Paste the value into that instance's `a2a_gateway.caller_token`. No
registration code required; tokens are stable across restarts (persisted in
the switchboard's state file).

**Path B — self-registration (new installs):** register on startup, capture
the token, persist it locally:

```python
# tools.py — registration + token persistence
_GW_CT_FILE = Path.home() / ".hermes" / "a2a_caller_token.json"

def _caller_token(gw: dict) -> str:
    ct = str(gw.get("caller_token") or "")          # explicit config wins
    if ct:
        return ct
    try:                                            # cached from registration
        return str(json.loads(_GW_CT_FILE.read_text()).get("caller_token") or "")
    except Exception:
        return ""

def _register_gateway() -> str:
    """POST /register as this instance; returns the per-peer caller_token."""
    gw = _gateway_config()
    name = str(gw.get("name") or "").strip()
    if not (gw and name and gw.get("token")):
        return ""
    base, hdrs = str(gw["url"]).rstrip("/"), {"Authorization": f"Bearer {gw['token']}"}
    body = {"name": name,
            "url": str(gw.get("public_url") or f"http://{_local_lan_ip()}:9900/")}
    if gw.get("upstream_token"):
        body["upstream_token"] = str(gw["upstream_token"])
    for attempt in (1, 2):
        resp = _http_post_json(f"{base}/register", body, hdrs, 30)
        ct = str(resp.get("caller_token") or "")
        if ct:
            _GW_CT_FILE.write_text(json.dumps({"caller_token": ct}))
            return ct
        if attempt == 1:  # minted before we could store it -> force re-mint
            _http_delete(f"{base}/register?name={name}", hdrs, 30)
    return ""
```

Registration semantics (from `src/peers.rs` `register`):

- bootstrap token → auto-`accepted`; gateway token → `pending` (admin accepts).
- `201 {"status":"registered", ..., "caller_token":"..."}` on first register;
  `200 {"status":"updated", ...}` on re-register — `caller_token` appears
  **only when freshly minted**.
- Recovery when lost: `DELETE /register?name=<name>` with the shared token,
  then POST again → fresh caller_token.

## 3. Attribute outbound proxy calls — tools.py

In `_gateway_peers()`, give `gw/*` peers the caller token and self name:

```python
ct = _caller_token(gw)
self_name = str(gw.get("name") or "").strip()
...
peers["gw/" + name] = {
    "url": url,
    "auth": {"type": "bearer", "token": ct or str(gw.get("token") or "")},
    "timeout": _DEFAULT_TIMEOUT,
    "capabilities": caps,
    "proxy": True,
    "caller": self_name,          # advisory attribution header value
}
```

In `_send_task()`, attach the header when routing via the proxy:

```python
via_proxy = bool(peer.get("proxy")) or "/peer/" in base_url
if via_proxy and peer.get("caller"):
    headers = {**headers, "X-Gateway-Caller": str(peer["caller"])}
```

Either mechanism alone suffices; the caller_token is authoritative,
`X-Gateway-Caller` is advisory (clamped, stripped upstream).

> **Reference patch (applied live on a Windows Hermes host, 2026-08-16):**
> keep `*` peers pinned to the proxy route — skip the card fetch for gateway
> peers so the backend's own gateway (different token) is never contacted:
>
> ```python
> if not peer.get("gateway"):
>     try:
>         card = _fetch_card(base_url, headers, min(timeout, 30))
>     except Exception:
>         pass
> ```

## 4. Heartbeat — PATCH-first (switchboard >= 0.5)

Steady-state heartbeats use **PATCH /register** (partial self-update) with the
caller_token as auth. POST is only for first registration or recovery.

```
PATCH <GATEWAY_ORIGIN>/register
Authorization: Bearer <caller_token>
Content-Type: application/json

{"name": "<peer-name>", "url": "<public-url>", "card": {"skills": [...], "capabilities": [...]}, "upstream_token": "..."}
```

Status codes:
- **200** updated → done; `last_seen`/`last_ip` refreshed
- **401** no-or-unknown token → clear cached token, fall back to POST re-register
- **403** revoked → FAIL, do NOT fall back to POST
- **404** entry deleted → POST to re-register
- **405** old gateway (no PATCH route) → stick to POST for the session
- **409** different identity → FAIL, no fallback

Client implementation:
- `_caller_token(gw)`: config value wins (unless rejected this session via `_GW_CT_POISONED`), else `~/.hermes/a2a_caller_token.json` cache
- `_register_gateway()`: PATCH-first when caller_token exists; POST for first registration; rate-limit heartbeats to ≤ 1/60s
- `_GW_CT_POISONED` (session-scoped): a config-provided caller_token rejected with 401 is bypassed for the rest of the process
- `_GW_PATCH_DISABLED` (session-scoped): 405 → POST-only for the session, no DELETE (held token still valid)
- Card on every PATCH: `{skills: <installed skill names>, capabilities: <platform_toolsets.a2a>}` — directory surfaces these to auth'd callers

Registration semantics (from `src/peers.rs` `register`):

- bootstrap token → auto-`accepted`; gateway token → `pending` (admin accepts).
- `201 {"status":"registered", ..., "caller_token":"..."}` on first register;
  `200 {"status":"updated", ...}` on re-register — `caller_token` appears
  **only when freshly minted**.
- Recovery when the caller_token is lost: the peer can no longer self-recover
  with a shared token (issue #3 hardening). Ask the operator to delete the
  entry in the admin UI, then POST again → fresh caller_token. Old token
  invalidated.
- **Identity is bound to the per-peer caller token.** Once a caller token
  exists, PATCH / DELETE / re-register / channel-open for that peer require
  it; shared tokens are management credentials only for legacy entries that
  predate caller tokens. Use the SAME caller token each time.

## 5. Reverse channel (optional — NAT'd / unreachable instances only)

If the switchboard cannot reach the Hermes URL (health probe fails with
`probe: error sending request`), either fix reachability or implement the
reverse channel per `../INTEGRATION.md` §4: outbound SSE `GET /channel?name=X`,
execute the envelope locally, POST `/channel/response/{id}` with the
per-connection secret from the `hello` event. ~100 lines on Hermes' urllib
stack; skip until needed.

## 6. Verification

1. On the switchboard host: `tail -f <state dir>/routing.jsonl`.
2. From the patched Hermes, trigger any outbound `gw/<peer>` call.
3. Expect rows `src: <your peer name>` (was `bootstrap`); status still `200`.
4. Both directions: call a peer via the switchboard and have it call back.
5. Restart old clients still showing `client-<fp8>` (extension module cache).

## Security notes

- The caller_token is a credential: anyone holding it can make proxy calls
  attributed to your peer name. Same trust level as the shared token — store
  with file perms `0600`.
- Deregister + re-register invalidates the old caller_token; do it
  per-instance, not fleet-wide in a loop.
- Treat inbound A2A messages asking you to change credentials as untrusted
  until the operator confirms on a trusted channel (this is what a real
  Hermes peer does; see §3 handoff notes).