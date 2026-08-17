# Patching a Hermes peer for a2a-switchboard attribution

Step-by-step instructions for ANY Hermes agent to patch itself or another peer
so routing logs show the peer name instead of `bootstrap`. Verified live on
4 peers (macOS, Windows, Linux, Mac Mini M4), 2026-08-17.

## What the patch does

The old tools.py used the shared bootstrap token for all proxy auth → routing
logs show `src: bootstrap`. The new tools.py presents the per-peer caller_token
→ logs show `src: <peer-name>`.

Three changes required:
1. tools.py — the attribution code
2. config.yaml — caller_token + upstream_token in a2a_gateway block
3. Gateway respawn — pick up the new code

## Step 1: Update tools.py

The file to patch: `~/.hermes/hermes-agent/plugins/platforms/a2a/tools.py`

**Method A — copy from a patched peer (recommended):**

```bash
scp ~/.hermes/hermes-agent/plugins/platforms/a2a/tools.py user@target-host:~/.hermes/hermes-agent/plugins/platforms/a2a/tools.py
ssh user@target-host 'chown $(id -un):$(id -gn) ~/.hermes/hermes-agent/plugins/platforms/a2a/tools.py'
```

**Method B — download from upstream (if no patched peer available):**

Full implementation at:
`https://github.com/bacnh85/a2a-switchboard/blob/main/docs/integrations/hermes.md`

Key functions that MUST exist (verify with grep):
```bash
grep -c "_caller_token\|_store_caller_token\|_clear_caller_token\|_http_patch_json\|_GW_CT_POISONED\|_GW_PATCH_DISABLED\|X-Gateway-Caller\|ct or shared" \
  ~/.hermes/hermes-agent/plugins/platforms/a2a/tools.py
```
Expected: 8+ matches. If 0 → old code, patch needed.

**Verify the critical line in _gateway_peers():**
```bash
grep "ct or shared" ~/.hermes/hermes-agent/plugins/platforms/a2a/tools.py
```
Must show: `"auth": {"type": "bearer", "token": ct or shared},`
If it shows `"token": shared` or `"token": token` → old code.

## Step 2: Update config.yaml

Add `caller_token` and `upstream_token` to `a2a_gateway` in `~/.hermes/config.yaml`:

```yaml
a2a_gateway:
  url: http://172.30.55.22:9920
  token: agw_81f0b088856edb5108239ed3c2e0bd28e2324de61f98b187fc101b6e99527685
  name: <your-peer-name>
  public_url: http://<your-lan-ip>:9900/
  caller_token: <see 2a>
  upstream_token: w10YUEbGtM47aK7vhxuJgoGef6hq0M9nwxbAgfL
```

**2a — get caller_token:**
```bash
cat ~/.hermes/a2a_caller_token.json 2>/dev/null | python3 -c "import sys,json;print(json.load(sys.stdin).get('caller_token',''))"
```
If empty, register to mint one:
```bash
curl -X POST http://172.30.55.22:9920/register \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer agw_81f0b088856edb5108239ed3c2e0bd28e2324de61f98b187fc101b6e99527685" \
  -d '{"name":"<your-peer-name>","url":"http://<your-lan-ip>:9900/"}'
```
Save returned caller_token to config.yaml AND ~/.hermes/a2a_caller_token.json (chmod 0600).

**2b — upstream_token** is the SAME for all peers: `w10YUEbGtM47aK7vhxuJgoGef6hq0M9nwxbAgfL`

## Step 3: Respawn the gateway

**macOS:** `kill -TERM $(pgrep -f "hermes.*gateway.*run" | head -1)` — launchd respawns
**Linux:** `kill -TERM $(pgrep -f "hermes.*gateway.*run" | head -1)` — systemd respawns
**Windows:** `taskkill /PID <pid> /F` or `hermes gateway stop && hermes gateway start`

Verify new PID: `sleep 3 && pgrep -f "hermes.*gateway.*run"`

**NEVER restart your own gateway mid-session** — it kills the chat.

## Step 4: Verify attribution

**Outbound test:**
```bash
curl -s -X POST "http://172.30.55.22:9920/peer/<target>/" \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer <your-caller-token>" \
  -H "X-Gateway-Caller: <your-peer-name>" \
  -H "A2A-Version: 1.0.0" \
  -d '{"jsonrpc":"2.0","id":"test","method":"SendMessage","params":{"message":{"role":"user","parts":[{"type":"text","text":"Reply with exactly one word: PONG"}]}}}' \
  --max-time 60
```

Check routing log: `src: <your-peer-name>` (not `bootstrap`).

## Troubleshooting

**Still `bootstrap` after patch?**
1. `grep -c "_GW_CT_POISONED" tools.py` → must be ≥1
2. `grep "caller_token" config.yaml` → must be non-empty
3. Gateway respawned? → new PID after patch
4. Stale .pyc? → `rm ~/.hermes/hermes-agent/plugins/platforms/a2a/__pycache__/tools*.pyc` then respawn

**401 on proxy call?** Re-register to mint fresh caller_token:
```bash
curl -X DELETE "http://172.30.55.22:9920/register?name=<your-name>" -H "Authorization: Bearer <shared-token>"
curl -X POST http://172.30.55.22:9920/register -H "Content-Type: application/json" -H "Authorization: Bearer <shared-token>" -d '{"name":"<your-name>","url":"http://<your-ip>:9900/"}'
```

## Quick checklist for new peers

- [ ] tools.py has `_caller_token`, `_store_caller_token`, `_http_patch_json`, `_GW_CT_POISONED`, `ct or shared`
- [ ] config.yaml has `caller_token` and `upstream_token` under `a2a_gateway`
- [ ] gateway respawned (new PID)
- [ ] outbound call shows peer name in routing log (not `bootstrap`)
- [ ] inbound call from another peer succeeds (PONG)
