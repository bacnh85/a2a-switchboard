use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::{broadcast, RwLock};

pub const RING_CAP: usize = 1000;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum PeerState {
    Pending,
    Accepted,
    Revoked,
}

/// What a registered identity is. Agents are callable upstream A2A
/// endpoints; humans are operator identities that call THROUGH the gateway
/// (their caller_token is the credential shared with the gateway).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum PeerKind {
    #[default]
    Agent,
    Human,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Peer {
    pub name: String,
    #[serde(default)]
    pub kind: PeerKind,
    /// Pinned upstream A2A endpoint. The ONLY url the proxy will ever call
    /// (deny-by-default egress). Humans never proxy anywhere — sentinel value.
    pub url: String,
    /// Agent Card as submitted at registration, stored verbatim.
    pub card: serde_json::Value,
    pub state: PeerState,
    /// sha256 of the token presented at registration. Identifies the registrant.
    pub fingerprint: String,
    /// Token the gateway presents when proxying TO this peer (optional).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upstream_token: Option<String>,
    /// Unique per-peer token for CALLING through this gateway (caller
    /// identity). Issued at registration; resolves the caller to this peer
    /// name even when the shared gateway token is used for auth.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub caller_token: Option<String>,
    pub registered_at: i64,
    #[serde(default)]
    pub last_seen: Option<i64>,
    /// Source IP of the peer's most recent successful exchange (register,
    /// proxied request, or reverse channel). Captured from the TCP peer
    /// address; display-only, never used for auth.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_ip: Option<String>,
    /// Source IP seen at registration time (the address the peer registered
    /// from). Useful to spot NAT'd/firewalled peers registering via a relay.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reg_ip: Option<String>,
    #[serde(default)]
    pub healthy: Option<bool>,
    #[serde(default)]
    pub last_error: Option<String>,
    #[serde(default)]
    pub auto_accepted: bool,
    /// Last health-probe attempt (success or failure) — epoch seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_probe_ts: Option<i64>,
    /// Last health probe that SUCCEEDED — "unreachable since" anchor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_ok_ts: Option<i64>,
}

impl Peer {
    /// Askama-friendly formatted registration time.
    pub fn registered_at_dt(&self) -> String {
        fmt_dt(self.registered_at)
    }
    /// Askama-friendly formatted last-seen (empty when never seen).
    pub fn last_seen_dt(&self) -> String {
        self.last_seen
            .map(fmt_dt)
            .unwrap_or_else(|| "—".to_string())
    }
    /// Last-seen IP or placeholder.
    pub fn last_ip_str(&self) -> String {
        self.last_ip.clone().unwrap_or_else(|| "—".to_string())
    }
    /// Registration source IP or placeholder.
    pub fn reg_ip_str(&self) -> String {
        self.reg_ip.clone().unwrap_or_else(|| "—".to_string())
    }
    pub fn state_str(&self) -> &'static str {
        match self.state {
            PeerState::Pending => "pending",
            PeerState::Accepted => "accepted",
            PeerState::Revoked => "revoked",
        }
    }
    pub fn kind_str(&self) -> &'static str {
        match self.kind {
            PeerKind::Agent => "agent",
            PeerKind::Human => "human",
        }
    }
    /// Short human summary of the agent card (first line of description).
    pub fn card_summary(&self) -> String {
        self.card
            .get("description")
            .and_then(|d| d.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| "no card".to_string())
    }
    /// Askama-friendly last-ok probe time (placeholder when never probed).
    pub fn last_ok_dt(&self) -> String {
        self.last_ok_ts.map(fmt_dt).unwrap_or_else(|| "—".into())
    }
    /// Askama-friendly last probe time (placeholder when never probed).
    pub fn last_probe_dt(&self) -> String {
        self.last_probe_ts.map(fmt_dt).unwrap_or_else(|| "—".into())
    }
}

/// Chat room: a named group of peers + humans. Created from the admin UI;
/// sends fan out to agent members as A2A message/send calls.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Room {
    pub id: String,
    pub name: String,
    pub members: Vec<String>,
    pub created_by: String,
    pub created_at: i64,
}

impl Room {
    pub fn created_at_dt(&self) -> String {
        fmt_dt(self.created_at)
    }
}

/// Human latency: `812 ms` · `5.4 s` · `38 s` · `3m 05s` · `1h 02m`.
pub fn fmt_ms(ms: u64) -> String {
    if ms < 1_000 {
        format!("{ms} ms")
    } else if ms < 10_000 {
        format!("{:.1} s", ms as f64 / 1_000.0)
    } else if ms < 60_000 {
        format!("{} s", ms / 1_000)
    } else if ms < 3_600_000 {
        format!("{}m {:02}s", ms / 60_000, (ms % 60_000) / 1_000)
    } else {
        format!("{}h {:02}m", ms / 3_600_000, (ms % 3_600_000) / 60_000)
    }
}

/// Human bytes: `271 B` · `1.2 kB` · `3.4 MB` · `1.1 GB`.
pub fn fmt_bytes(b: u64) -> String {
    const K: u64 = 1024;
    if b < K {
        return format!("{b} B");
    }
    let (v, u) = if b < K * K {
        (b as f64 / K as f64, "kB")
    } else if b < K * K * K {
        (b as f64 / (K * K) as f64, "MB")
    } else {
        (b as f64 / (K * K * K) as f64, "GB")
    };
    format!("{v:.1} {u}")
}

/// Nearest-rank percentile over a pre-sorted slice (0 for empty).
pub fn percentile(sorted: &[u64], p: usize) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    let idx = (sorted.len() * p / 100).min(sorted.len() - 1);
    sorted[idx]
}

/// One messenger bubble. `conv` is "dm:<a>|<b>" (sorted name pair) or
/// "room:<id>". Persisted to chat.jsonl + in-memory ring + SSE broadcast.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub id: u64,
    pub ts: i64,
    pub conv: String,
    pub src: String,
    pub text: String,
    /// "chat" = participant bubble, "system" = roster/delivery events.
    pub kind: String,
    /// "ok" | "err"
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl ChatMessage {
    pub fn ts_dt(&self) -> String {
        fmt_dt(self.ts)
    }
    pub fn is_err(&self) -> bool {
        self.status == "err"
    }
    pub fn is_system(&self) -> bool {
        self.kind == "system"
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct Persisted {
    gateway_token: String,
    bootstrap_token: String,
    #[serde(default)]
    peers: Vec<Peer>,
    #[serde(default)]
    rooms: Vec<Room>,
    #[serde(default)]
    admin: Option<AdminCred>,
}

/// Peer-registry/health change signal for the admin UI live updates
/// (assets/live.js). kinds: register|accept|reject|revoke|delete|health.
#[derive(Clone, serde::Serialize)]
pub struct PeerEvent {
    pub kind: String,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouteEntry {
    pub ts: i64,
    pub src: String,
    pub dst: String,
    pub method: String,
    pub status: u16,
    pub bytes: u64,
    pub latency_ms: u64,
    /// JSON-RPC method from the request body (audit; None for non-RPC calls).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rpc_method: Option<String>,
    /// JSON-RPC request id (audit correlation).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rpc_id: Option<String>,
    /// Redacted, capped preview of the request params (audit trail).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview: Option<String>,
    /// Redacted, capped preview of the response result (audit trail).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resp_preview: Option<String>,
    /// A2A task lifecycle state from the response (`result.status.state`),
    /// or "error" on a JSON-RPC error response (audit).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_state: Option<String>,
}

impl RouteEntry {
    /// Askama-friendly formatted timestamp (local date-time).
    pub fn ts_dt(&self) -> String {
        fmt_dt(self.ts)
    }
    /// Method shown in the UI: JSON-RPC method when captured, else HTTP method.
    pub fn method_display(&self) -> &str {
        self.rpc_method.as_deref().unwrap_or(&self.method)
    }
    /// Human latency (`812 ms`, `5.4 s`, `3m 05s`).
    pub fn latency_human(&self) -> String {
        fmt_ms(self.latency_ms)
    }
    /// Human payload size (`271 B`, `1.2 kB`).
    pub fn bytes_human(&self) -> String {
        fmt_bytes(self.bytes)
    }
    /// A2A task state for display: `TASK_STATE_COMPLETED` → `completed`.
    pub fn task_state_display(&self) -> String {
        self.task_state
            .as_deref()
            .unwrap_or_default()
            .trim_start_matches("TASK_STATE_")
            .to_ascii_lowercase()
    }
    /// Color class for the task state: errors red, action-needed warn,
    /// everything else quiet ("" = default muted rendering).
    pub fn task_state_class(&self) -> &'static str {
        match self.task_state.as_deref() {
            Some("TASK_STATE_FAILED") | Some("TASK_STATE_REJECTED") => "bad",
            Some("TASK_STATE_INPUT_REQUIRED") => "warn",
            _ => "",
        }
    }
}

/// Request-side audit info extracted from a (potential) JSON-RPC body.
#[derive(Debug, Default, Clone)]
pub struct AuditInfo {
    pub rpc_method: Option<String>,
    pub rpc_id: Option<String>,
    pub preview: Option<String>,
}

const PREVIEW_MAX: usize = 2048;
const PREVIEW_DEPTH: u8 = 8;
const PREVIEW_ARRAY_CAP: usize = 50;

/// True when a key looks like it holds a secret → replaced with "[redacted]"
/// in previews. Heuristic denylist, not a security boundary.
fn key_is_secret(k: &str) -> bool {
    // normalize separators so x-api-key / api-key / api.key all match api_key
    let k = k.to_lowercase().replace(['-', ' ', '.'], "_");
    [
        "token",
        "authorization",
        "api_key",
        "apikey",
        "secret",
        "password",
        "cookie",
    ]
    .iter()
    .any(|s| k.contains(s))
}

fn redact_json(v: &serde_json::Value, depth: u8) -> serde_json::Value {
    use serde_json::Value;
    match v {
        Value::Object(m) => {
            if depth == 0 {
                return Value::String("…".into());
            }
            let mut out = serde_json::Map::with_capacity(m.len());
            for (k, val) in m {
                out.insert(
                    k.clone(),
                    if key_is_secret(k) {
                        Value::String("[redacted]".into())
                    } else {
                        redact_json(val, depth - 1)
                    },
                );
            }
            Value::Object(out)
        }
        Value::Array(a) => {
            let mut out: Vec<Value> = a
                .iter()
                .take(PREVIEW_ARRAY_CAP)
                .map(|x| redact_json(x, depth))
                .collect();
            if a.len() > PREVIEW_ARRAY_CAP {
                out.push(Value::String(format!("…+{}", a.len() - PREVIEW_ARRAY_CAP)));
            }
            Value::Array(out)
        }
        Value::String(s) if s.chars().count() > 256 => {
            Value::String(s.chars().take(255).collect::<String>() + "…")
        }
        _ => v.clone(),
    }
}

/// Extract audit info from a request body: JSON-RPC `method`/`id` plus a
/// redacted, size-capped preview of `params` (whole document when not a
/// JSON-RPC envelope). Non-JSON bodies yield no preview — we never log
/// arbitrary payloads.
pub fn audit_extract(body: &[u8]) -> AuditInfo {
    let Ok(v) = serde_json::from_slice::<serde_json::Value>(body) else {
        return AuditInfo::default();
    };
    let rpc_id = match v.get("id") {
        Some(serde_json::Value::Null) | None => None,
        // capped like rpc_method: the ring + jsonl retain every entry
        Some(serde_json::Value::String(s)) => Some(s.chars().take(64).collect()),
        // numbers/null-ish and any other JSON scalar → their JSON text
        Some(i) => Some(i.to_string().chars().take(64).collect()),
    };
    let mut preview = redact_json(v.get("params").unwrap_or(&v), PREVIEW_DEPTH).to_string();
    if preview.len() > PREVIEW_MAX {
        let mut end = PREVIEW_MAX;
        while !preview.is_char_boundary(end) {
            end -= 1;
        }
        preview.truncate(end);
        preview.push('…');
    }
    AuditInfo {
        rpc_method: v
            .get("method")
            .and_then(|m| m.as_str())
            .map(|s| s.chars().take(64).collect()),
        rpc_id,
        preview: Some(preview),
    }
}

/// Extract audit info from a response body: the A2A task lifecycle state
/// (`result.status.state`) or "error" on a JSON-RPC error, plus a redacted,
/// size-capped preview of the result. Returns `(task_state, resp_preview)`.
/// Guard extraction to JSON only at the call site (see peers.rs) — if SSE
/// passthrough ever lands there, non-JSON bodies must not be buffered.
pub fn audit_extract_response(body: &[u8]) -> (Option<String>, Option<String>) {
    let Ok(v) = serde_json::from_slice::<serde_json::Value>(body) else {
        return (None, None);
    };
    // A2A Task objects carry lifecycle in result.status.state.
    let task_state = if v.get("error").is_some() {
        Some("error".to_string())
    } else {
        v.get("result")
            .and_then(|r| r.get("status"))
            .and_then(|s| s.get("state"))
            .and_then(|s| s.as_str())
            .map(|s| s.chars().take(64).collect())
    };
    let mut preview = redact_json(v.get("result").unwrap_or(&v), PREVIEW_DEPTH).to_string();
    if preview.len() > PREVIEW_MAX {
        let mut end = PREVIEW_MAX;
        while !preview.is_char_boundary(end) {
            end -= 1;
        }
        preview.truncate(end);
        preview.push('…');
    }
    (task_state, Some(preview))
}

#[derive(Default)]
pub struct RateLimiter {
    // key -> accepted request timestamps (unix secs), ascending.
    // Rolling window (issue #5): no 2x boundary burst like the fixed window.
    hits: Mutex<HashMap<String, VecDeque<i64>>>,
}

impl RateLimiter {
    /// Rolling window: at most `max` accepted requests in the last 60s.
    pub fn allow(&self, key: &str, max: u32) -> bool {
        let now: i64 = now();
        let cutoff = now - 60;
        let mut hits = self.hits.lock().unwrap();
        if hits.len() > 10_000 {
            // cheap cleanup: drop keys with no recent accepted request
            hits.retain(|_, v| v.back().is_some_and(|t| *t >= cutoff));
        }
        let q = hits.entry(key.to_string()).or_default();
        while q.front().is_some_and(|t| *t < cutoff) {
            q.pop_front();
        }
        let allowed = q.len() < max as usize;
        if allowed {
            q.push_back(now);
        }
        allowed
    }
}

#[derive(Default)]
pub struct Inner {
    pub gateway_token: String,
    pub bootstrap_token: String,
    pub peers: Vec<Peer>,
    pub rooms: Vec<Room>,
    pub admin: Option<AdminCred>,
}

/// Admin password credential. `hash` is argon2id (PHC string) for new
/// passwords; pre-0.6.0 entries are a bare 64-hex salted SHA-256 (no marker)
/// and are transparently upgraded to argon2id on the next successful login.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdminCred {
    pub salt: String,
    pub hash: String,
}

impl AdminCred {
    /// argon2id hash, PHC-string encoded.
    pub fn hash_pw(pw: &str) -> AdminCred {
        use argon2::password_hash::{rand_core::OsRng, SaltString};
        use argon2::{Algorithm, Argon2, Params, PasswordHasher, Version};
        let salt = SaltString::generate(&mut OsRng);
        let params = Params::new(19456, 2, 1, None).expect("valid argon2 params");
        let a2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
        let hash = a2
            .hash_password(pw.as_bytes(), &salt)
            .expect("argon2 hash")
            .to_string();
        AdminCred {
            salt: String::new(), // argon2 PHC string carries its own salt
            hash,
        }
    }

    /// Verify against argon2id or the legacy sha256 format.
    fn verify(&self, pw: &str) -> bool {
        if self.is_legacy() {
            return crate::auth::ct_eq(&hash_pw_legacy(&self.salt, pw), &self.hash);
        }
        use argon2::PasswordVerifier;
        let Ok(parsed) = argon2::PasswordHash::new(&self.hash) else {
            return false;
        };
        argon2::Argon2::default()
            .verify_password(pw.as_bytes(), &parsed)
            .is_ok()
    }

    /// True when this credential still uses the pre-0.6.0 format: a bare
    /// 64-hex-char salted SHA-256 (0.5.x wrote it without any marker, so
    /// legacy = everything that is not an argon2 PHC string).
    fn is_legacy(&self) -> bool {
        !self.hash.starts_with("$argon2")
    }
}

pub const SESSION_TTL: i64 = 12 * 3600;

pub struct App {
    pub data_dir: PathBuf,
    pub inner: RwLock<Inner>,
    pub log_ring: RwLock<VecDeque<RouteEntry>>,
    pub log_tx: broadcast::Sender<RouteEntry>,
    /// Registry/health flip signals for the live peers pages (SSE `peers`).
    pub peers_tx: broadcast::Sender<PeerEvent>,
    /// Messenger history: ring for the UI, chat.jsonl on disk, SSE `chat`.
    pub chat_ring: RwLock<VecDeque<ChatMessage>>,
    pub chat_tx: broadcast::Sender<ChatMessage>,
    /// Monotonic chat message id (survives restart via chat.jsonl tail).
    pub chat_seq: std::sync::atomic::AtomicU64,
    pub limiter: RateLimiter,
    pub http: reqwest::Client,
    /// Reverse channels: firewalled peers hold outbound SSE connections here.
    pub channels: crate::channel::Channels,
    /// Admin UI sessions: token -> expires_unix. In-memory; restart logs out.
    pub sessions: Mutex<HashMap<String, i64>>,
    /// Prometheus counters keyed "{src}\t{dst}\t{method}\t{status}" — bumped
    /// in log_route (the single choke point), rendered by /metrics.
    pub metrics: Mutex<HashMap<String, u64>>,
    /// Process start (unix secs) for the uptime gauge.
    pub started_at: i64,
}

/// routing.jsonl size cap before rotation (bytes). 0 disables the file log.
/// Set once from config at startup (issue #5).
pub static ROUTING_LOG_MAX_BYTES: std::sync::RwLock<u64> = std::sync::RwLock::new(64 * 1024 * 1024);

/// Messenger ring cap (loaded bubbles) + chat.jsonl rotation cap.
pub const CHAT_RING_CAP: usize = 2000;
pub static CHAT_LOG_MAX_BYTES: std::sync::RwLock<u64> = std::sync::RwLock::new(16 * 1024 * 1024);

/// Sorted dm conversation id for a name pair.
pub fn dm_conv(a: &str, b: &str) -> String {
    if a <= b {
        format!("dm:{a}|{b}")
    } else {
        format!("dm:{b}|{a}")
    }
}

/// When false, audit previews (redacted param snapshots) are dropped from
/// the routing log — free-text parts can carry secrets key-name redaction
/// cannot see. Set once from config at startup (issue #5).
pub static PREVIEW_ENABLED: std::sync::RwLock<bool> = std::sync::RwLock::new(true);

/// chmod 0600 on unix (no-op elsewhere) — state.json/routing.jsonl hold
/// cleartext tokens and audit data.
fn restrict_perms(path: &std::path::Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    #[cfg(not(unix))]
    let _ = path;
}

pub type AppState = Arc<App>;

pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

/// Format a unix timestamp as local date-time `YYYY-MM-DD HH:MM:SS`.
/// Used by templates (Askama can't format dates inline). Pure std, no
/// chrono dep — keeps the zero-dep runtime story intact.
pub fn fmt_dt(ts: i64) -> String {
    let secs = if ts < 0 { 0 } else { ts as u64 };
    // days since epoch → civil date (Howard Hinnant's algorithm)
    let days = (secs / 86400) as i64;
    let rem = (secs % 86400) as i64;
    let (h, m, s) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    let _ = (m, s);
    let z = days + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02} {h:02}:{m:02}:{s:02}")
}

pub fn fingerprint(token: &str) -> String {
    let mut h = Sha256::new();
    h.update(token.as_bytes());
    hex(&h.finalize())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn gen_token() -> String {
    use rand::RngCore;
    let mut b = [0u8; 32];
    rand::rng().fill_bytes(&mut b);
    format!("agw_{}", hex(&b))
}

impl App {
    pub async fn load(data_dir: PathBuf) -> anyhow::Result<Self> {
        std::fs::create_dir_all(&data_dir)?;
        let path = data_dir.join("state.json");
        let inner = if path.exists() {
            let raw = std::fs::read_to_string(&path)?;
            let p: Persisted = serde_json::from_str(&raw)?;
            Inner {
                gateway_token: p.gateway_token,
                bootstrap_token: p.bootstrap_token,
                peers: p.peers,
                rooms: p.rooms,
                admin: p.admin,
            }
        } else {
            Inner {
                gateway_token: gen_token(),
                bootstrap_token: gen_token(),
                peers: Vec::new(),
                rooms: Vec::new(),
                admin: None,
            }
        };
        let (log_tx, _) = broadcast::channel(256);
        let (peers_tx, _) = broadcast::channel(64);
        let (chat_tx, _) = broadcast::channel(64);
        // Reload the messenger history: ring gets the tail, chat_seq stays
        // monotonic via the MAX id across the whole file (ids may reset in
        // files written before the newline fix).
        let chat_all = read_chat_records(&data_dir);
        // Empty file → first id is 0; continuing file → max + 1.
        let chat_seq = chat_all
            .iter()
            .map(|m| m.id)
            .max()
            .map(|m| m + 1)
            .unwrap_or(0);
        let chat_tail: Vec<ChatMessage> = chat_all
            .into_iter()
            .rev()
            .take(CHAT_RING_CAP)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        // Seed the routing ring from the persisted log tail so dashboard
        // KPIs, the flow log, and peer activity survive restarts (history
        // lives in routing.jsonl; the ring is only the last RING_CAP).
        let mut routes = crate::admin::read_routing_log(&data_dir);
        let ring_seed: VecDeque<RouteEntry> = routes
            .drain(routes.len().saturating_sub(RING_CAP)..)
            .collect();
        let http = reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(5))
            // Never follow redirects: keeps egress pinned to the registered URL (SSRF guard).
            .redirect(reqwest::redirect::Policy::none())
            .build()?;
        let app = Self {
            data_dir: data_dir.clone(),
            inner: RwLock::new(inner),
            log_ring: RwLock::new(ring_seed),
            log_tx,
            peers_tx,
            chat_ring: RwLock::new(chat_tail.into_iter().collect()),
            chat_tx,
            chat_seq: std::sync::atomic::AtomicU64::new(chat_seq),
            limiter: RateLimiter::default(),
            http,
            channels: crate::channel::Channels::new(),
            sessions: Mutex::new(HashMap::new()),
            metrics: Mutex::new(HashMap::new()),
            started_at: now(),
        };
        // Tighten an existing state.json/routing.jsonl left world-readable by
        // an older build (issue #5).
        for name in ["state.json", "routing.jsonl", "chat.jsonl"] {
            let p = data_dir.join(name);
            if p.exists() {
                restrict_perms(&p);
            }
        }
        Ok(app)
    }

    /// Atomic persist: tmp file + rename, so a crash never truncates state.json.
    /// The tmp file is created 0600 — state.json carries every token in
    /// cleartext and must never be world-readable (issue #5).
    pub async fn persist(&self) {
        let inner = self.inner.read().await;
        let p = Persisted {
            gateway_token: inner.gateway_token.clone(),
            bootstrap_token: inner.bootstrap_token.clone(),
            peers: inner.peers.clone(),
            rooms: inner.rooms.clone(),
            admin: inner.admin.clone(),
        };
        drop(inner);
        if let Ok(json) = serde_json::to_string_pretty(&p) {
            let tmp = self.data_dir.join("state.json.tmp");
            let dst = self.data_dir.join("state.json");
            let mut ok = std::fs::write(&tmp, json).is_ok();
            if ok {
                restrict_perms(&tmp);
                ok = std::fs::rename(&tmp, &dst).is_ok();
            }
            if ok {
                restrict_perms(&dst);
            }
        }
    }

    /// Size-capped append to routing.jsonl (issue #5): when the file exceeds
    /// `max_bytes`, rotate it to routing.jsonl.1 (previous .1 dropped) before
    /// appending. `max_bytes == 0` disables the file log entirely (ring + SSE
    /// still work; admin audit pages show only the in-memory ring).
    fn append_routing_log(&self, line: &str) {
        use std::io::Write;
        let path = self.data_dir.join("routing.jsonl");
        let cap = *ROUTING_LOG_MAX_BYTES.read().unwrap();
        if cap == 0 {
            return;
        }
        if let Ok(meta) = std::fs::metadata(&path) {
            if meta.len() + line.len() as u64 > cap {
                let _ = std::fs::rename(&path, self.data_dir.join("routing.jsonl.1"));
            }
        }
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
        {
            let _ = f.write_all(line.as_bytes());
        }
        restrict_perms(&path);
    }

    pub async fn log_route(&self, e: RouteEntry) {
        let preview_enabled = *PREVIEW_ENABLED.read().unwrap();
        let mut e = e;
        if !preview_enabled {
            e.preview = None;
            e.resp_preview = None;
        }
        // Prometheus counter — key order matches the /metrics label set.
        let key = format!("{}\t{}\t{}\t{}", e.src, e.dst, e.method, e.status);
        *self.metrics.lock().unwrap().entry(key).or_insert(0) += 1;
        if let Ok(mut json) = serde_json::to_string(&e) {
            json.push('\n');
            self.append_routing_log(&json);
        }
        let mut ring = self.log_ring.write().await;
        if ring.len() >= RING_CAP {
            ring.pop_front();
        }
        ring.push_back(e.clone());
        let _ = self.log_tx.send(e);
    }

    /// Fire-and-forget registry/health signal for live admin UI updates.
    /// Sync fn: broadcast send is non-blocking and never fails on a live bus.
    /// Record one messenger bubble: assign id/ts, append to chat.jsonl,
    /// push the ring, broadcast on the SSE chat channel. Returns the stored
    /// message (with id) so handlers can echo it to the client.
    pub async fn log_chat(&self, mut m: ChatMessage) -> ChatMessage {
        if m.ts == 0 {
            m.ts = now();
        }
        m.id = self
            .chat_seq
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        if let Ok(mut json) = serde_json::to_string(&m) {
            // Newline-delimited: log_route does the same — without this the
            // file is one giant line and read_chat_records can't parse it,
            // losing all history (and chat_seq) on restart.
            json.push('\n');
            self.append_chat_log(&json);
        }
        {
            let mut ring = self.chat_ring.write().await;
            if ring.len() >= CHAT_RING_CAP {
                ring.pop_front();
            }
            ring.push_back(m.clone());
        }
        let _ = self.chat_tx.send(m.clone());
        m
    }

    /// Size-capped append to chat.jsonl, rotation like routing.jsonl.
    fn append_chat_log(&self, line: &str) {
        use std::io::Write;
        let path = self.data_dir.join("chat.jsonl");
        let cap = *CHAT_LOG_MAX_BYTES.read().unwrap();
        if cap == 0 {
            return;
        }
        if let Ok(meta) = std::fs::metadata(&path) {
            if meta.len() + line.len() as u64 > cap {
                let _ = std::fs::rename(&path, self.data_dir.join("chat.jsonl.1"));
            }
        }
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
        {
            let _ = f.write_all(line.as_bytes());
        }
        restrict_perms(&path);
    }

    pub fn emit_peers(&self, kind: &str, name: &str) {
        let _ = self.peers_tx.send(PeerEvent {
            kind: kind.to_string(),
            name: name.to_string(),
        });
    }

    pub async fn recent_log(&self, n: usize) -> Vec<RouteEntry> {
        let ring = self.log_ring.read().await;
        ring.iter().rev().take(n).cloned().collect()
    }

    pub async fn regenerate_bootstrap(&self) -> String {
        let t = gen_token();
        self.inner.write().await.bootstrap_token = t.clone();
        self.persist().await;
        t
    }

    /// Ensure an admin password exists (issue #4): returns Some(plaintext)
    /// when a random password was just generated — main() logs it once; the
    /// cleartext is never stored. None = a password was already set.
    pub async fn ensure_admin_password(&self) -> Option<String> {
        if self.admin_set().await {
            return None;
        }
        let pw = gen_admin_password();
        self.set_admin_password(None, &pw).await.ok().map(|_| pw)
    }

    /// Set or change the admin password. When one exists, `current` must match.
    /// Legacy sha256 credentials are transparently upgraded on successful auth.
    pub async fn set_admin_password(
        &self,
        current: Option<&str>,
        new: &str,
    ) -> Result<(), &'static str> {
        {
            let mut inner = self.inner.write().await;
            if let Some(cred) = &inner.admin {
                let cur = current.ok_or("current password required")?;
                if !cred.verify(cur) {
                    return Err("current password is incorrect");
                }
            }
            if new.len() < 8 {
                return Err("new password must be at least 8 characters");
            }
            inner.admin = Some(AdminCred::hash_pw(new));
        }
        self.persist().await;
        Ok(())
    }

    /// Verify a login attempt. Returns true on success; when the stored
    /// credential is the legacy sha256 format and the password is correct,
    /// it is transparently re-hashed with argon2id.
    pub async fn verify_admin_password(&self, pw: &str) -> bool {
        let mut upgrade_salt = None;
        let ok = {
            let inner = self.inner.read().await;
            match &inner.admin {
                Some(c) => {
                    let ok = c.verify(pw);
                    if ok && c.is_legacy() {
                        upgrade_salt = Some(c.hash.clone()); // any marker; upgrade below
                    }
                    ok
                }
                None => false,
            }
        };
        if ok && upgrade_salt.is_some() {
            // Re-hash with argon2id on first successful login.
            let mut inner = self.inner.write().await;
            if let Some(_c) = &inner.admin {
                inner.admin = Some(AdminCred::hash_pw(pw));
                drop(inner);
                self.persist().await;
            }
        }
        ok
    }

    pub async fn admin_set(&self) -> bool {
        self.inner.read().await.admin.is_some()
    }

    pub fn create_session(&self) -> String {
        let t = gen_token();
        let mut s = self.sessions.lock().unwrap();
        s.retain(|_, exp| *exp > now());
        s.insert(t.clone(), now() + SESSION_TTL);
        t
    }

    pub fn session_valid(&self, token: &str) -> bool {
        let mut s = self.sessions.lock().unwrap();
        s.retain(|_, exp| *exp > now());
        s.contains_key(token)
    }

    pub fn drop_session(&self, token: &str) {
        self.sessions.lock().unwrap().remove(token);
    }
}

/// Load chat.jsonl: ALL newline-delimited records, oldest first. Corrupt
/// lines are skipped. (Pre-fix files may hold one giant concatenated line —
/// repair tooling splits those; this loader only reads proper NDJSON.)
fn read_chat_records(data_dir: &std::path::Path) -> Vec<ChatMessage> {
    let path = data_dir.join("chat.jsonl");
    let Ok(raw) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    raw.lines()
        .filter_map(|l| serde_json::from_str::<ChatMessage>(l).ok())
        .collect()
}

fn hash_pw_legacy(salt: &str, pw: &str) -> String {
    let mut h = Sha256::new();
    h.update(salt.as_bytes());
    h.update(pw.as_bytes());
    hex(&h.finalize())
}

/// Generate a readable random admin password: 4 groups of 4 lowercase
/// letters/digits, hyphen-separated (no ambiguous chars, ~64 bits).
pub fn gen_admin_password() -> String {
    use rand::Rng;
    const ALPHABET: &[u8] = b"abcdefghjkmnpqrstuvwxyz23456789";
    let mut rng = rand::rng();
    (0..4)
        .map(|_| {
            (0..4)
                .map(|_| ALPHABET[rng.random_range(0..ALPHABET.len())] as char)
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("-")
}

/// Validate a peer-declared URL: http(s) only, has host. Deny-by-default egress starts here.
pub fn validate_url(url: &str) -> anyhow::Result<()> {
    let u = url::Url::parse(url).map_err(|_| anyhow::anyhow!("invalid URL"))?;
    match u.scheme() {
        "http" | "https" => {
            if u.host_str().is_none() {
                anyhow::bail!("URL must have a host");
            }
            Ok(())
        }
        s => anyhow::bail!("scheme '{s}' not allowed (http/https only)"),
    }
}

// tiny inline url parser usage — avoid an extra dependency? `url` is already a reqwest dep; declare it.
