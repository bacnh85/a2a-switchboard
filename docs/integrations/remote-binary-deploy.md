# Remote binary deploy (validate-before-image workflow)

Deploy a freshly built binary straight to a LAN host over SSH, run it under
systemd (no container), validate there, and only then tag a release so
GitHub CI produces the image. This is the fast inner loop for testing `main`
against real peers before cutting `vX.Y.Z`.

Reference host in examples: `root@172.30.55.22` (Arch Linux, x86_64),
service port 9920, data dir `/var/lib/switchboard`.

## One-time host setup

The host previously ran the podman quadlet (`/etc/containers/systemd/switchboard.container`)
with data at `/root/data/switchboard` (owned by container UID 1000 — no host
user has that UID). The systemd service must run as the **same UID** so
state files stay readable by both runtime styles:

```bash
# UID 1000 must be free on the host (getent passwd 1000 → empty)
useradd --system --uid 1000 switchboard
mv /root/data/switchboard /var/lib/switchboard   # same-fs rename, keeps uid-1000 ownership
```

## Build + deploy

Build on the host in podman (same environment as the CI image; bookworm
glibc runs fine on Arch's newer glibc). `--network=host` is required —
the default pasta networking cannot create tap devices on that host
(`/dev/net/tun` unavailable):

```bash
# from the repo root (local): ship the source over
rsync -aq --delete --exclude .git --exclude target ./ root@172.30.55.22:/tmp/agw-src/

# on the host: build + extract the binary
podman build --network=host -t agw-build --target build /tmp/agw-src
CID=$(podman create agw-build)
podman cp $CID:/usr/local/bin/a2a-switchboard /usr/local/bin/a2a-switchboard
podman rm $CID && podman rmi agw-build
```

Then stop the container/quadlet (port 9920 conflict) and start the unit:

```bash
systemctl disable --now switchboard.service   # quadlet, if present
podman rm -f a2a-switchboard 2>/dev/null || true
install -m 0755 /usr/local/bin/a2a-switchboard /usr/local/bin/a2a-switchboard

cat > /etc/systemd/system/a2a-switchboard.service <<'EOF'
[Unit]
Description=a2a-switchboard (A2A switchboard gateway)
After=network.target

[Service]
ExecStart=/usr/local/bin/a2a-switchboard
Environment=AGW_DATA_DIR=/var/lib/switchboard
Environment=AGW_BIND=0.0.0.0:9920
User=switchboard
Restart=on-failure
RestartSec=3

[Install]
WantedBy=multi-user.target
EOF

systemctl daemon-reload && systemctl enable --now a2a-switchboard
curl -sf http://127.0.0.1:9920/login >/dev/null && echo health OK
```

## Rollback (zero chown)

The podman image is kept on the host. Files stay uid-1000 owned in both
directions, so rollback is:

```bash
systemctl disable --now a2a-switchboard
# point the quadlet at the same dir and start it
podman run -d --name a2a-switchboard -p 9920:9920 \
  -v /var/lib/switchboard:/data ghcr.io/bacnh85/a2a-switchboard:latest
```

## Validation checklist before tagging

- `systemctl status a2a-switchboard` → `active (running)`, clean `journalctl -u a2a-switchboard`
- `ls -ln /var/lib/switchboard` → all files still uid 1000 after writes
- Old peers present in the directory (`/.well-known/agent.json` with a token)
- `PONG` smoke test through `/peer/<name>/`
- `curl -H "Authorization: Bearer <token>" http://host:9920/metrics` scrapes
- Admin UI: peer detail page renders expandable audit rows

Then: `git tag vX.Y.Z && git push --tags` → release.yml builds and pushes
the multi-arch image.
