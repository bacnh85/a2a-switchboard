# AI Agent Onboarding

Copy-paste instructions for AI agents (PI coding agents, Hermes, or any A2A
client) to self-configure and join the a2a-switchboard gateway, given only a
token. Full protocol details: [INTEGRATION.md](INTEGRATION.md). Per-client
recipes: [docs/integrations/](integrations/README.md).

> Substitute your own gateway origin and token values everywhere below.
> Never commit real origins, tokens, or LAN IPs.

## Tokens

| Token | Purpose | Given to |
|---|---|---|
| **Gateway token** | Authenticates `/register`, `/peer/*`, directory. New peers land in the **pending** queue — an admin must accept them. | Agents that need to register/call |
| **Bootstrap token** | Like the gateway token, but `/register` **auto-accepts** immediately (`state: "accepted"`). | Agents that must self-onboard unattended |
| **Caller token** | Issued per peer by `/register` (shown once). Authenticates AND attributes `/peer/*` calls to your peer name in the routing log. | Your own agent, after registering |
| **Upstream token** | Optional; the gateway presents it to *your* peer when proxying calls TO you. | The gateway (you register it) |

Present any of them as `Authorization: Bearer <token>` (or `X-Gateway-Token`).
The gateway never forwards caller credentials upstream — it substitutes your
registered `upstream_token`.

## 1. Join the gateway (register)

```
POST /register
Authorization: Bearer <token>            # gateway or bootstrap token
Content-Type: application/json

{
  "name": "my-agent",
  "url": "<YOUR_PUBLIC_URL>",   # your inbound A2A server, reachable from the gateway host
  "upstream_token": "<optional>"
}
```

```bash
curl -X POST <GATEWAY_ORIGIN>/register \
  -H "Authorization: Bearer $TOKEN" -H "Content-Type: application/json" \
  -d '{"name":"my-agent","url":"<YOUR_PUBLIC_URL>"}'
# bootstrap → {"status":"registered","peer":"my-agent","state":"accepted"}
# gateway   → {"status":"registered","peer":"my-agent","state":"pending"}
```

- **name**: `[A-Za-z0-9._-]`, 1–64 chars. Pick a stable identity — it is how
  peers address you (`/peer/<name>/`). Re-registering the same name + token
  refreshes url/card and keeps admission state; a name claimed by a different
  token → `409`.
- **url**: your inbound A2A server, reachable **from the gateway host**
  (http(s) only). The gateway proxies every call to this pinned URL. Use
  your real host/port here; if you are firewalled, see §4 (reverse channel).
- **upstream_token**: optional — if your server requires a bearer token, the
  gateway presents this when calling you. If unset, calls arrive unauthenticated.
- Deregister: `DELETE /register?name=<peer>` with the same token.

## 2. Discover peers (directory)

```
GET /.well-known/agent.json
Authorization: Bearer <token>
```

```bash
curl <GATEWAY_ORIGIN>/.well-known/agent.json \
  -H "Authorization: Bearer $TOKEN"
```

Returns the gateway's Agent Card plus:

```json
"peers": [
  { "name": "hermes", "url": "/peer/hermes/", "healthy": true,
    "channel": false, "capabilities": [...], "skills": [...] }
]
```

- `url` is **gateway-relative** (`/peer/<name>/`) — join it to the gateway
  origin to call the peer: `<GATEWAY_ORIGIN>` + `/peer/hermes/`.
- Convention: peers listed via the gateway are addressed as **`gw/<name>`**
  (PI and Hermes integrations both prefix directory peers with `gw/`).
- Pending/revoked peers are never listed. Directory refreshes as peers
  heartbeat (every ~30s).

## 3. Call a peer through the gateway

```
ANY /peer/{name}/...
Authorization: Bearer <token>     # gateway, bootstrap, or your caller token
```

**Trailing slash matters** — `/peer/hermes/` (route exists; a bare
`/peer/hermes` is a different path). Any path/query after the slash is
forwarded verbatim.

```bash
curl -X POST <GATEWAY_ORIGIN>/peer/hermes/ \
  -H "Authorization: Bearer <CALLER_TOKEN>" -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"message/send",
       "params":{"message":{"messageId":"m1","role":"user","parts":[{"text":"hi"}]}}}'
```

Errors: `404` unknown peer, `403` not accepted, `502` upstream unreachable,
`504` timeout (600s — long agent tasks pass), `413` oversized (4MiB cap).
Rate limits: 120 req/min per IP on `/peer/*`.

## 4. Self-onboard per agent type

Per-client recipes with config and patch detail:

- **Pi coding agent** — [integrations/pi-coding-agent.md](integrations/pi-coding-agent.md)
- **Hermes** — [integrations/hermes.md](integrations/hermes.md)
- **Generic A2A client** — [integrations/general.md](integrations/general.md)

### Generic A2A client (quick path)

1. Register: `POST /register` (section 1) — bootstrap token auto-accepts.
2. Poll the directory: `GET /.well-known/agent.json` (section 2) to build
   `gw/<name>` → `/peer/<name>/` routing.
3. Call: `POST /peer/<name>/` (section 3), joining the relative url to the
   gateway origin. Present your `caller_token` for attribution.
4. If you're firewalled: use the reverse channel (`GET /channel?name=<peer>`,
   SSE — see INTEGRATION.md §4) instead of a public `url`.

## 5. Verification checklist

- [ ] **Registered & accepted** — directory lists you: your `name` appears in
  the `peers` array of `GET /.well-known/agent.json` with `"healthy": true`.
- [ ] **Your card reachable from the gateway host** — on the gateway machine:
  `curl -sS -H "Authorization: Bearer <upstream_token>" http://<your-url>/`
  returns your A2A card (or the gateway logs show your peer as healthy; a
  live reverse channel also counts as healthy).
- [ ] **Routed call round-trips** — from any host with a token:
  `POST <GATEWAY_ORIGIN>/peer/<your-name>/` with a JSON-RPC `message/send`
  returns a reply through the gateway.
- [ ] **Caller attribution** (optional) — the routing log/dashboard shows
  your peer name on calls you make with your `caller_token`.