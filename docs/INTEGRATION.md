# Integration

Everything an A2A client needs to join the switchboard: registration,
discovery, proxied calls, and the reverse-channel protocol for firewalled
peers. All endpoints are plain HTTP/JSON-RPC — any A2A-compatible client works.

## Authentication

Two token classes (shown on first run and in Settings):

| Token | Effect at `/register` |
|---|---|
| **Gateway token** | Peer created in the **pending** queue — an admin must accept it |
| **Bootstrap token** | Peer **auto-accepted** immediately |

Peers present their token as `Authorization: Bearer <token>` (or the
`X-Gateway-Token` header). The switchboard never forwards caller credentials
to upstream peers; it substitutes each peer's registered `upstream_token`.

## 1. Register a peer

```
POST /register
Authorization: Bearer <token>
Content-Type: application/json

{
  "name": "hermes",                 // [A-Za-z0-9._-], 1-64 chars
  "url": "http://192.168.1.20:9900/",  // http(s) only; the pinned proxy target
  "card": { ... },                  // optional Agent Card (skills/capabilities)
  "upstream_token": "..."           // optional; switchboard presents this when proxying TO you
}
```

Responses:

```json
{"status":"registered","peer":"hermes","state":"pending"}
{"status":"registered","peer":"hermes","state":"accepted"}   // bootstrap token
{"status":"updated","peer":"hermes","state":"accepted"}      // re-register, same identity
```

- Re-registering the same name + token refreshes url/card and keeps admission
  state.
- A name claimed by a different token → `409 Conflict`.
- Deregister: `DELETE /register?name=<peer>` with the same token.

### pi-a2a (Pi) example

```jsonc
// ~/.pi/agent/settings.json
{
  "a2a": {
    "server": { "enabled": true, "port": 9910, "host": "127.0.0.1" },
    "discovery": {
      "gateway": {
        "url": "http://127.0.0.1:9920",
        "token": "<bootstrap or gateway token>",
        "upstreamToken": "<your inbound server token, optional>"
      }
    }
  }
}
```

Each pi session auto-registers as `<name>-<port>`, opens a reverse channel,
refreshes the peer directory every heartbeat, and deregisters on exit.

## 2. Discover peers (directory)

```
GET /.well-known/agent.json        (alias: /.well-known/agent-card.json)
Authorization: Bearer <token>      (optional — pending/revoked peers are never listed)
```

Returns the switchboard's own Agent Card plus `peers: [{ name, url, healthy,
channel, capabilities, skills }]`. `url` is switchboard-relative
(`/peer/<name>/`) — join it to your switchboard origin to call the peer.

pi-a2a peers see these automatically as `gw/<name>` entries in `a2a_list` /
`a2a_call`.

## 3. Call a peer through the switchboard

```
ANY /peer/{name}/...                (any path/query forwarded verbatim)
Authorization: Bearer <token>
```

The switchboard proxies to the peer's **pinned registration URL** with the
peer's `upstream_token` (if registered). Deny-by-default: only *accepted*
peers' URLs are ever contacted; redirects are never followed; only http/https.

```bash
curl -X POST http://switchboard:9920/peer/hermes/ \
  -H "Authorization: Bearer $TOKEN" -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"message/send",
       "params":{"message":{"messageId":"m1","role":"user","parts":[{"text":"hi"}]}}}'
```

Errors: `404` unknown peer, `403` not accepted, `502` upstream unreachable,
`504` timeout (600s — long agent tasks pass), `413` oversized (4MiB cap).

## 4. Reverse channel — firewalled peers (no tunnels)

If the switchboard cannot reach your peer's URL (NAT/firewall), the peer opens
an **outbound** SSE connection and the switchboard delivers requests down it.

```
GET /channel?name=<peer>            (SSE, held open)
Authorization: Bearer <token>
```

Event sequence:

1. `event: hello` → `data: <per-connection secret>` — **keep this secret**;
   it must be echoed in every response.
2. `event: request` → `data: <envelope JSON>`:

   ```json
   {
     "id": 42, "method": "POST", "path": "/", "query": null,
     "headers": {"content-type": "application/json"},
     "body_b64": "<base64 request body>",
     "chan_secret": "<the secret from hello>"
   }
   ```

3. Peer executes the request against its **local** A2A server
   (`http://127.0.0.1:<port>` + path), then:

   ```
   POST /channel/response/{id}?name=<peer>
   Authorization: Bearer <token>
   Content-Type: application/json

   { "id": 42, "status": 200, "headers": {"content-type": "application/json"},
     "body_b64": "<base64 response body>", "chan_secret": "<secret>" }
   ```

4. `event: ping` every 15s keeps the connection alive.

Protocol rules (security):

- The `chan_secret` binds responses to the connection that received the
  request — a peer sharing your token cannot answer your pending requests.
- Envelopes carry **no credentials** (auth is stripped at the switchboard).
- Reconnect with capped backoff (1s → 30s) when the stream drops; on
  reconnect you get a NEW secret.
- Body cap 4MiB both directions (oversized envelopes are dropped).
- On disconnect, pending callers get `502` immediately (no 600s hang).

A reference client implementation: `extensions/lib/gateway.ts` in
`bacnh85/pi-extensions` (pi-a2a), class `ChannelClient`.

## Rate limits & caps

| Endpoint | Limit |
|---|---|
| `/register` | 20 req/min per IP |
| `/peer/*`, `/channel/response/*` | 120 req/min per IP |
| Body size (proxy + channel) | 4 MiB |
| Proxy/channel request timeout | 600 s |
| Channel queue depth | 256 (excess → 503 retryable) |

## Health

Every `heartbeat_sec` (default 30s) the switchboard probes each accepted peer:
direct peers via their card URL; channel peers are healthy by construction (a
live channel IS the health signal). Health + last-seen show in the UI and
directory.
