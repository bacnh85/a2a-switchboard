# a2a-switchboard

**Self-hosted A2A (Agent2Agent) switchboard — admit, route, and observe your agents.**

A single Rust binary that brokers A2A traffic between agents the way an operator
switchboard connects calls: agents wait at the board until **you** connect them.
No config-file wrestling, no control plane — a live admin UI for admission,
routing logs, and a live routing topology.

```
Peer A ──┐                            ┌── Peer B (public URL)
         ├── a2a-switchboard (you) ───┤
Peer C ──┘                            └── Peer D (firewalled — reverse channel)
```

## Why a switchboard instead of a gateway?

`agentgateway` and similar projects solve **config-driven proxying for platform
teams** — YAML, RBAC policies, LLM/MCP routing. This project solves
**human-in-the-loop admission for self-hosters**: your agents (Pi, Hermes, any
A2A client) register, wait in a pending queue, and YOU accept them — with
tokens, a directory, live routing logs, and a live topology view in one binary.

## Features

- **Peer admission** — gateway token puts new peers in a pending queue;
  bootstrap token auto-accepts; accept/reject/revoke from the admin UI.
- **Token exchange** — peers authenticate with the gateway; the gateway
  authenticates to each peer with their registered `upstream_token`.
- **Proxy routing** — any A2A JSON-RPC call via `/peer/{name}/...`, deny-by-
  default egress (only accepted peers' pinned URLs, no redirects).
- **Reverse channels** — firewalled/NAT'd peers hold an outbound SSE connection;
  the switchboard delivers requests down it and receives responses back. All
  connections are peer-initiated — one open inbound port is enough.
- **Directory as Agent Card** — `/.well-known/agent.json` (alias
  `agent-card.json`) lists accepted peers, auth-aware.
- **Admin UI** — sidebar layout, dashboard with **live routing topology**
  (requests animate as packets caller → gateway → destination, live
  communication log of every routed request), pending-peer queue, routing log.
- **Caller attribution** — optional `X-Gateway-Caller` header (advisory,
  display-only, stripped before forwarding) so the dashboard shows which peer
  made each call.
- **Password protection** — optional admin password, set in Settings (initial
  set requires a localhost connection). Auth is off until a password is set;
  then all admin pages/APIs require a 12h session cookie. Peer/token endpoints
  are unaffected.
- **Zero-dep runtime** — rust-embed bakes the UI in; single 7MB static binary,
  no Node, no CDN, no database.

## Quickstart

### Docker (recommended)

```bash
docker run -d --name switchboard -p 9920:9920 -v ./data:/data \
  ghcr.io/bacnh85/a2a-switchboard:latest
```

### Or the binary

```bash
cargo build --release        # or download from Releases
./target/release/a2a-switchboard
```

First run prints the two tokens (also saved to `data/state.json` and shown in
Settings):

```
gateway API token : agw_...
bootstrap token   : agw_...
```

Open **http://127.0.0.1:9920** — that's the admin UI.

### Register a peer (gateway token → pending queue)

```bash
curl -X POST http://127.0.0.1:9920/register \
  -H "Authorization: Bearer $GATEWAY_TOKEN" -H "Content-Type: application/json" \
  -d '{"name":"hermes","url":"http://192.168.1.20:9900/",
       "card":{...optional agent card...},
       "upstream_token":"optional-token-gateway-uses-when-calling-this-peer"}'
# → {"status":"registered","peer":"hermes","state":"pending"}
```

Accept it at http://127.0.0.1:9920/peers. With the **bootstrap token** the same
call returns `"state":"accepted"` immediately.

### Call a peer through the switchboard

```bash
curl -X POST http://127.0.0.1:9920/peer/hermes/ \
  -H "Authorization: Bearer $GATEWAY_TOKEN" -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"message/send","params":{...}}'
```

## Documentation

| Guide | What it covers |
|---|---|
| [docs/DEPLOYMENT.md](docs/DEPLOYMENT.md) | Docker/compose/binary, TLS termination, firewall, systemd, backup |
| [docs/INTEGRATION.md](docs/INTEGRATION.md) | Registration, directory, proxy, reverse-channel protocol, pi-a2a config |
| [docs/SECURITY.md](docs/SECURITY.md) | Threat model, token classes, SSRF posture, known limitations |
| [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md) | Build, test, architecture, adding features, release checklist |
| [CHANGELOG.md](CHANGELOG.md) | Version history |

## Comparison

| | **a2a-switchboard** | agentgateway |
|---|---|---|
| Admission model | human-in-the-loop (pending queue, UI) | config/YAML-driven |
| Deployment | single binary, self-hosted | config files, k8s/Gateway API |
| Firewalled peers | reverse channel (built-in) | needs tunnels |
| Admin UI | live: admission, log, graph | read-only explorer |
| Scope | A2A only | A2A + MCP + LLM routing |

## License

Apache-2.0. See [LICENSE](LICENSE).
