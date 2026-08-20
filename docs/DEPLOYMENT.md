# Deployment

a2a-switchboard is a single static binary. This guide covers Docker, bare
binary, TLS, firewalls, systemd, and backups.

## Docker (recommended)

```bash
docker pull ghcr.io/bacnh85/a2a-switchboard:latest
mkdir -p data && sudo chown 1000:1000 data   # container runs as UID 1000
docker run -d --name switchboard \
  -e AGW_BIND=0.0.0.0:9920 \
  -p 127.0.0.1:9920:9920 \
  -v ./data:/data \
  ghcr.io/bacnh85/a2a-switchboard:latest
docker logs switchboard   # first start prints the admin UI password
```

The loopback publish keeps the gateway off the LAN by default. To accept
peers from the LAN, publish on all interfaces (`-p 9920:9920`) and put a TLS
terminator in front (see below).

- The image contains only the binary + CA certificates (~20MB). Templates,
  CSS, and JS are compiled into the binary (rust-embed) — nothing else to mount.
- State lives in `/data` (`state.json` = peers + tokens, `routing.jsonl` =
  append-only routing log). Mount a persistent volume.
- Healthcheck: the compose file uses a TCP probe on 9920.
- The container runs as **UID 1000** (non-root). For bind mounts, ensure the
  host directory is writable by that uid: `mkdir -p data && sudo chown 1000:1000 data`.
  Upgrading from ≤0.5? `sudo chown -R 1000:1000 /path/to/data` first, or the
  gateway cannot write its state.
- On first start the admin UI password is generated and printed to the logs
  (`docker logs switchboard` / `journalctl -u switchboard`). Change it in
  Settings → Admin access.

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
 | `server.cookie_secure` | `AGW_COOKIE_SECURE` | `false` | Mark session cookies `Secure` (TLS terminator in front) |
 | `server.routing_log_max_mb` | `AGW_ROUTING_LOG_MAX_MB` | `64` | `routing.jsonl` size cap before rotating to `.1` (0 = no file log) |
 | `server.audit_previews` | `AGW_AUDIT_PREVIEWS` | `true` | Keep redacted request previews in the audit log |

Optional `config.toml` in the working directory (see `config.toml.example`).
Env vars override config file; defaults fill the rest.

## TLS termination

The switchboard speaks plain HTTP by design (self-hosted; TLS is a
terminator's job). The admin UI requires its password (generated at first
run, printed to the logs), but bearer tokens still transit the wire in
cleartext — put a TLS terminator in front for remote access.

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

> **Security warning**: if `AGW_BIND` is widened beyond `127.0.0.1`, bearer
> tokens transit plaintext HTTP. The gateway logs a warning at startup.
> Prefer a loopback bind behind a TLS terminator; set `AGW_COOKIE_SECURE=1`
> so session cookies get the `Secure` flag. The compose file publishes
> `127.0.0.1:9920:9920` by default for this reason.

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
