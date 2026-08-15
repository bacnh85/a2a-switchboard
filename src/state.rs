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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Peer {
    pub name: String,
    /// Pinned upstream A2A endpoint. The ONLY url the proxy will ever call (deny-by-default egress).
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
    #[serde(default)]
    pub healthy: Option<bool>,
    #[serde(default)]
    pub last_error: Option<String>,
    #[serde(default)]
    pub auto_accepted: bool,
}

#[derive(Debug, Serialize, Deserialize)]
struct Persisted {
    gateway_token: String,
    bootstrap_token: String,
    #[serde(default)]
    peers: Vec<Peer>,
    #[serde(default)]
    admin: Option<AdminCred>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RouteEntry {
    pub ts: i64,
    pub src: String,
    pub dst: String,
    pub method: String,
    pub status: u16,
    pub bytes: u64,
    pub latency_ms: u64,
}

#[derive(Default)]
pub struct RateLimiter {
    // ip -> (window_start_unix, count)
    hits: Mutex<HashMap<String, (u64, u32)>>,
}

impl RateLimiter {
    /// Fixed window: max requests per 60s sliding window bucket.
    pub fn allow(&self, key: &str, max: u32) -> bool {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let mut hits = self.hits.lock().unwrap();
        if hits.len() > 10_000 {
            hits.retain(|_, (w, _)| now - *w < 60);
        }
        let e = hits.entry(key.to_string()).or_insert((now, 0));
        if now - e.0 >= 60 {
            *e = (now, 0);
        }
        e.1 += 1;
        e.1 <= max
    }
}

#[derive(Default)]
pub struct Inner {
    pub gateway_token: String,
    pub bootstrap_token: String,
    pub peers: Vec<Peer>,
    pub admin: Option<AdminCred>,
}

/// Salted hash of the admin dashboard password.
// ponytail: sha256+salt+ct_eq, not argon2 — state.json already stores plaintext
// gateway/bootstrap tokens; upgrade if those are ever hashed at rest.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdminCred {
    pub salt: String,
    pub hash: String,
}

pub const SESSION_TTL: i64 = 12 * 3600;

pub struct App {
    pub data_dir: PathBuf,
    pub inner: RwLock<Inner>,
    pub log_ring: RwLock<VecDeque<RouteEntry>>,
    pub log_tx: broadcast::Sender<RouteEntry>,
    pub limiter: RateLimiter,
    pub http: reqwest::Client,
    /// Reverse channels: firewalled peers hold outbound SSE connections here.
    pub channels: crate::channel::Channels,
    /// Admin UI sessions: token -> expires_unix. In-memory; restart logs out.
    pub sessions: Mutex<HashMap<String, i64>>,
}

pub type AppState = Arc<App>;

pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
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
                admin: p.admin,
            }
        } else {
            Inner {
                gateway_token: gen_token(),
                bootstrap_token: gen_token(),
                peers: Vec::new(),
                admin: None,
            }
        };
        let (log_tx, _) = broadcast::channel(256);
        let http = reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(5))
            // Never follow redirects: keeps egress pinned to the registered URL (SSRF guard).
            .redirect(reqwest::redirect::Policy::none())
            .build()?;
        Ok(Self {
            data_dir,
            inner: RwLock::new(inner),
            log_ring: RwLock::new(VecDeque::with_capacity(RING_CAP)),
            log_tx,
            limiter: RateLimiter::default(),
            http,
            channels: crate::channel::Channels::new(),
            sessions: Mutex::new(HashMap::new()),
        })
    }

    /// Atomic persist: tmp file + rename, so a crash never truncates state.json.
    pub async fn persist(&self) {
        let inner = self.inner.read().await;
        let p = Persisted {
            gateway_token: inner.gateway_token.clone(),
            bootstrap_token: inner.bootstrap_token.clone(),
            peers: inner.peers.clone(),
            admin: inner.admin.clone(),
        };
        drop(inner);
        if let Ok(json) = serde_json::to_string_pretty(&p) {
            let tmp = self.data_dir.join("state.json.tmp");
            let dst = self.data_dir.join("state.json");
            if std::fs::write(&tmp, json).is_ok() {
                let _ = std::fs::rename(&tmp, &dst);
            }
        }
    }

    pub async fn log_route(&self, e: RouteEntry) {
        if let Ok(mut json) = serde_json::to_string(&e) {
            json.push('\n');
            use std::io::Write;
            if let Ok(mut f) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(self.data_dir.join("routing.jsonl"))
            {
                let _ = f.write_all(json.as_bytes());
            }
        }
        let mut ring = self.log_ring.write().await;
        if ring.len() >= RING_CAP {
            ring.pop_front();
        }
        ring.push_back(e.clone());
        let _ = self.log_tx.send(e);
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

    /// Set or change the admin password. When one exists, `current` must match.
    pub async fn set_admin_password(
        &self,
        current: Option<&str>,
        new: &str,
    ) -> Result<(), &'static str> {
        {
            let mut inner = self.inner.write().await;
            if let Some(cred) = &inner.admin {
                let cur = current.ok_or("current password required")?;
                if hash_pw(&cred.salt, cur) != cred.hash {
                    return Err("current password is incorrect");
                }
            }
            if new.len() < 8 {
                return Err("new password must be at least 8 characters");
            }
            let salt = gen_token();
            inner.admin = Some(AdminCred {
                hash: hash_pw(&salt, new),
                salt,
            });
        }
        self.persist().await;
        Ok(())
    }

    pub async fn verify_admin_password(&self, pw: &str) -> bool {
        let inner = self.inner.read().await;
        match &inner.admin {
            Some(c) => crate::auth::ct_eq(&hash_pw(&c.salt, pw), &c.hash),
            None => false,
        }
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

fn hash_pw(salt: &str, pw: &str) -> String {
    let mut h = Sha256::new();
    h.update(salt.as_bytes());
    h.update(pw.as_bytes());
    hex(&h.finalize())
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
