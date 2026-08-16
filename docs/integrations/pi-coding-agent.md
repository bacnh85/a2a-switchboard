# Pi coding agent integration (pi-a2a)

Pi is the coding agent this switchboard was built around: it has **native**
gateway support in the pi-a2a extension, so integration is mostly
configuration. It auto-registers, opens a reverse channel (firewall-friendly),
refreshes the directory on every heartbeat, and deregisters on exit.

## How it works

- Each Pi session registers as `<name>-<port>` (e.g. `pi-s2-9910`) via
  `POST /register` using the configured gateway token.
- `POST /register` returns a per-peer `caller_token`; pi-a2a stores it
  (`GatewayUpstream`) and presents it on outbound `gw/<name>` overlay calls,
  so the switchboard attributes traffic to the peer name.
- It opens an outbound SSE reverse channel (`GET /channel?name=<peer>`) — no
  inbound port required, works behind NAT/firewalls.
- Directory refresh every heartbeat merges peers as `gw/<name>` entries in
  `a2a_list` / `a2a_call`.

## 1. Configure (`~/.pi/agent/settings.json`)

```jsonc
{
  "a2a": {
    "server": { "enabled": true, "port": 9910, "host": "127.0.0.1" },
    "discovery": {
      "gateway": {
        "url": "<GATEWAY_ORIGIN>",          // e.g. http://gateway.example:9920
        "token": "<bootstrap or gateway token>",
        "upstreamToken": "<your inbound server token, optional>"
      }
    }
  }
}
```

Both bootstrap and gateway tokens work: bootstrap auto-accepts; gateway lands
the peer in the pending queue until an admin accepts it.

## 2. Restart Pi

Restart the Pi session so the extension module reloads (modules are cached
per process — `/reload` may not be enough). After restart:

- Peers appear under **"Gateway-discovered peers"** as `gw/<name>`.
- The routing log shows `src: <your peer name>` on your outbound calls.

## 3. Verification

1. `a2a_list` shows `gw/` peers.
2. Make a call: `a2a_call gw/<peer> "hello"` → reply arrives.
3. On the switchboard host, the routing log shows `src: <your peer name>`.

## Notes & gotchas

- **Caller attribution requires a per-peer caller token.** Old Pi sessions
  that registered before per-peer tokens existed mint one automatically on
  the next heartbeat (`caller_token` disclosed once). Restart the Pi process
  to pick it up.
- **Extension code is cached per Pi process** — after updating pi-a2a, the
  session must be fully restarted (not just conversation-reloaded).
- **`client-<fp8>` in the routing log** = a client presenting the shared
  token without the caller token or `X-Gateway-Caller` (e.g. a stale session
  or raw curl). Restart it, or send callers through
  `X-Gateway-Caller: <name>`.
- Upstream token: if your Pi A2A server requires a bearer token, set
  `upstreamToken`; the switchboard presents it when proxying calls to you.

## Reference

- Client implementation: `extensions/lib/gateway.ts` (`GatewayUpstream`,
  `ChannelClient`) in the pi-a2a extension.
- Wire protocol: [../INTEGRATION.md](../INTEGRATION.md).