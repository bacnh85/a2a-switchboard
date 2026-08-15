# Deployment

a2a-switchboard is a single static binary. This guide covers Docker, bare
binary, TLS, firewalls, systemd, and backups.

## Docker (recommended)

```bash
docker pull ghcr.io/bacnh85/a2a-switchboard:latest
docker run -d --name switchboard \
  -p 9920:9920 \
  -v ./data:/data \
  ghcr.io/bacnh85/a2a-switchboard:latest
```

- The image contains only the binary + CA certificates (~20MB). Templates,
  CSS, and JS are compiled into the binary (rust-embed) — nothing else to mount.
- State lives in `/data` (`state.json` = peers + tokens, `routing.jsonl` =
  append-only routing log). Mount a persistent volume.
- Healthcheck: the compose file uses a TCP probe on 9920.

### docker-compose

```bash
curl -O https://raw.githubusercontent.com/bacnh85/a2a-switchboard/main/docker-compose.yml
docker compose up -d
```

## Bare binary

```bash
curl -LO https://github.com/bacnh85/a2a-switchboard/releases/latest/download/a2a-switchboard
chmod +x a2a-switchboard
AGW_DATA_DIR=/var/lib/switchboard ./a2a-switchboard
```

First run prints the gateway token and bootstrap token. They're also in
`data/state.json` and visible in the admin UI → Settings.

## Configuration reference

| Config key (`config.toml`) | Env | Default | Meaning |
|---|---|---|---|
| `server.bind` | `AGW_BIND` | `127.0.0.1:9920` | Listen address |
| `server.data_dir` | `AGW_DATA_DIR` | `data/` | State + routing log directory |
| `server.heartbeat_sec` | `AGW_HEARTBEAT_SEC` | `30` | Peer health probe interval |

Optional `config.toml` in the working directory (see `config.toml.example`).
Env vars override config file; defaults fill the rest.

## TLS termination

The switchboard speaks plain HTTP by design (self-hosted; TLS is a
terminator's job). Admin UI is **unauthenticated** — you should put TLS + auth
in front of it for remote access.

### Caddy

```caddy
switchboard.example.com {
    reverse_proxy 127.0.0.1:9920
    # optional: basicauth { … }
}
```

### nginx

```nginx
server {
    listen 443 ssl;
    server_name switchboard.example.com;
    ssl_certificate     /etc/ssl/certs/cert.pem;
    ssl_certificate_key /etc/ssl/certs/key.pem;
    location / { proxy_pass http://127.0.0.1:9920; }
}
```

> **Security warning**: if `AGW_BIND` is widened beyond `127.0.0.1`, the admin
> UI is exposed without authentication. The UI shows a warning banner in this
> state. Restrict at the network/firewall layer or put an authenticating proxy
> in front.

## Firewall rules

- **Inbound**: one port (9920) — peers reach the switchboard here.
- **Outbound**: to accepted peers' URLs (direct-proxy peers) and to peer
  agents' public endpoints. Channel peers need no inbound rule at all — they
  hold an outbound SSE connection to the switchboard.
- Peers behind NAT need **no port forwarding** if they open a reverse channel
  (see INTEGRATION.md).

## systemd

```ini
# /etc/systemd/system/a2a-switchboard.service
[Unit]
Description=a2a-switchboard
After=network.target

[Service]
ExecStart=/usr/local/bin/a2a-switchboard
Environment=AGW_DATA_DIR=/var/lib/switchboard
Restart=on-failure
User=switchboard
DynamicUser=yes

[Install]
WantedBy=multi-user.target
```

```bash
systemctl daemon-reload && systemctl enable --now a2a-switchboard
```

## Backups

Two files matter:

- `data/state.json` — peers + tokens. Restoring this restores your admission
  state exactly. **Tokens are in plaintext** — back it up as a secret.
- `data/routing.jsonl` — append-only routing metadata (no message bodies).

Copy both on a schedule; atomic writes mean a plain `cp` is safe.

## Upgrading

1. Pull/stop the old container.
2. Start the new one against the same `/data` volume — state persists.
3. Check the UI dashboard for peer health after boot (channels + probes
   rebind automatically).

Channels and heartbeats reconnect with capped backoff, so a switchboard
restart is transparent to connected peers.
