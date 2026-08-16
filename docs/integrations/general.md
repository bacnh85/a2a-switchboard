# General A2A client integration

Plain HTTP/JSON-RPC recipe for any A2A-compatible client (curl, Python,
Go, Rust, …). No SDK needed — every endpoint is a REST call with a bearer
token. Full wire protocol: [../INTEGRATION.md](../INTEGRATION.md).

> Replace `<GATEWAY_ORIGIN>`, `<TOKEN>`, `<NAME>`, `<URL>`, `<CALLER_TOKEN>`
> with your values. Never commit real gateway origins, tokens, or LAN IPs.

## 1. Register

```
POST /register
Authorization: Bearer <token>      # gateway or bootstrap token
Content-Type: application/json

{
  "name": "<my-agent>",
  "url": "http://<your-host>:<port>/",   # reachable from the gateway host
  "upstream_token": "<optional>",        # presented by the gateway when calling you
  "card": { ... }                        # optional Agent Card
}
```

```bash
curl -X POST <GATEWAY_ORIGIN>/register \
  -H "Authorization: Bearer $TOKEN" -H "Content-Type: application/json" \
  -d '{"name":"my-agent","url":"http://my-host:9900/"}'
# bootstrap → {"status":"registered","peer":"my-agent","state":"accepted",
#              "caller_token":"<CALLER_TOKEN>"}
# gateway   → {"status":"registered","peer":"my-agent","state":"pending"}
```

- **name**: `[A-Za-z0-9._-]`, 1–64 chars, stable identity — peers address you
  as `/peer/<name>/`. Re-registering same name+token refreshes url/card and
  keeps admission; same name + different token → `409`.
- **url**: your inbound A2A server, reachable from the gateway host
  (http/https only). The gateway pins every proxied call to this URL.
- **upstream_token**: if your server requires bearer auth, the gateway
  presents this when calling you. Unset → calls arrive unauthenticated.
- **caller_token**: returned in the register response (once, when minted).
  Keep it; it attributes your `/peer/*` calls.
- Deregister: `DELETE /register?name=<peer>` with the same token.

## 2. Discover peers

```
GET /.well-known/agent.json
Authorization: Bearer <token>
```

```bash
curl <GATEWAY_ORIGIN>/.well-known/agent.json -H "Authorization: Bearer $TOKEN"
```

Returns the gateway's Agent Card plus:

```json
"peers": [
  { "name": "hermes", "url": "/peer/hermes/", "healthy": true,
    "channel": false, "capabilities": [...], "skills": [...] }
]
```

- `url` is **gateway-relative** (`/peer/<name>/`) — join it to the gateway
  origin: `<GATEWAY_ORIGIN>` + `/peer/<name>/`. Convention: address directory
  peers as `gw/<name>` so they never collide with direct peers.
- Pending/revoked peers are never listed. Directory refreshes as peers
  heartbeat (every ~30s).

## 3. Call a peer

```
ANY /peer/{name}/...          # any path/query forwarded verbatim
Authorization: Bearer <token> # gateway, bootstrap, or your caller token
```

**Trailing slash matters** — `/peer/<name>/` is the route; a bare
`/peer/<name>` is not.

```bash
curl -X POST <GATEWAY_ORIGIN>/peer/<name>/ \
  -H "Authorization: Bearer <CALLER_TOKEN>" -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"message/send",
       "params":{"message":{"messageId":"m1","role":"user","parts":[{"text":"hi"}]}}}'
```

Errors: `404` unknown peer, `403` not accepted, `502` upstream unreachable,
`504` timeout (600s — long agent tasks pass), `413` oversized (4MiB cap),
`503` channel queue full (retryable). Rate limits: 120 req/min per IP on
`/peer/*`.

### Caller attribution

Present your **caller_token** (not the shared token) on `/peer/*` calls and
the routing log shows your peer name. Alternatively/additionally send
`X-Gateway-Caller: <name>` (advisory; 64 chars max; stripped before
forwarding). Without either, the log shows the token-class label
(`bootstrap`/`gateway`) or `client-<fp8>`.

## 4. Firewalled peers — reverse channel

If the gateway cannot reach your `url`, open an outbound SSE connection and
receive requests down it (works behind NAT):

```
GET /channel?name=<peer>       # SSE, held open
Authorization: Bearer <token>
```

1. `event: hello` → `data: <per-connection secret>` — **keep this secret**;
   echo it in every response.
2. `event: request` → `data: <envelope JSON>`
   (`{id, method, path, query, headers, body_b64, chan_secret}`; no
   credentials).
3. Execute the request against your local A2A server, then:

   ```
   POST /channel/response/{id}?name=<peer>
   Authorization: Bearer <token>
   Content-Type: application/json

   { "id": 42, "status": 200, "headers": {...},
     "body_b64": "<base64 response body>", "chan_secret": "<secret>" }
   ```

4. `event: ping` every 15s keeps it alive. Reconnect with capped backoff
   (1s → 30s); on reconnect you get a NEW secret.

Gotchas: `chan_secret` binds responses to the connection that received the
request (a peer sharing your token cannot answer yours); 4MiB body cap both
ways; on disconnect pending callers get `502` immediately. Reference client:
pi-a2a `ChannelClient` (see `pi-coding-agent.md`).

## 5. Verification checklist

- [ ] **Registered & accepted** — you appear in `GET /.well-known/agent.json`
  with `"healthy": true`.
- [ ] **Your card reachable from the gateway host** — on the gateway machine:
  `curl -sS -H "Authorization: Bearer <upstream_token>" http://<your-url>/`
  returns your A2A card (or the gateway logs show healthy; a live reverse
  channel also counts).
- [ ] **Routed call round-trips** — from any host with a token:
  `POST <GATEWAY_ORIGIN>/peer/<your-name>/` with `message/send` returns a
  reply through the gateway.
- [ ] **Caller attribution** — routing log/dashboard shows your peer name on
  calls you make with your caller_token.

## Security notes

- Keep tokens out of repos/logs; rotate by registering with a new identity.
- The gateway never forwards your credentials upstream — it substitutes your
  registered `upstream_token`.
- Treat `gateway` tokens as capable of registering peers; per-peer
  `caller_token`s can only call `/peer/*` (never `/register` or `/channel`).