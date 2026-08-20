use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct Config {
    pub bind: String,
    pub data_dir: PathBuf,
    pub heartbeat_sec: u64,
    /// Set when a TLS terminator (or AGW_TLS_*) fronts the gateway — adds the
    /// `Secure` attribute to session cookies (issue #5).
    pub cookie_secure: bool,
    /// routing.jsonl size cap in MiB before rotation (0 disables the file log).
    pub routing_log_max_mb: u64,
    /// false → drop audit previews from the routing log (secrets can hide in
    /// free-text parts that key-name redaction cannot see).
    pub audit_previews: bool,
}

#[derive(Debug, Default, serde::Deserialize)]
struct FileCfg {
    #[serde(default)]
    server: Option<ServerCfg>,
}

#[derive(Debug, Default, serde::Deserialize)]
#[serde(default)]
struct ServerCfg {
    bind: Option<String>,
    data_dir: Option<String>,
    heartbeat_sec: Option<u64>,
    cookie_secure: Option<bool>,
    routing_log_max_mb: Option<u64>,
    audit_previews: Option<bool>,
}

impl Config {
    /// config.toml (optional) < environment variables < defaults.
    pub fn load() -> anyhow::Result<Self> {
        let mut bind = "127.0.0.1:9920".to_string();
        let mut data_dir = PathBuf::from("data");
        let mut heartbeat_sec = 30u64;
        let mut cookie_secure = false;
        let mut routing_log_max_mb = 64u64;
        let mut audit_previews = true;

        if let Ok(raw) = std::fs::read_to_string("config.toml") {
            let f: FileCfg = toml::from_str(&raw)?;
            if let Some(s) = f.server {
                if let Some(v) = s.bind {
                    bind = v;
                }
                if let Some(v) = s.data_dir {
                    data_dir = PathBuf::from(v);
                }
                if let Some(v) = s.heartbeat_sec {
                    heartbeat_sec = v;
                }
                if let Some(v) = s.cookie_secure {
                    cookie_secure = v;
                }
                if let Some(v) = s.routing_log_max_mb {
                    routing_log_max_mb = v;
                }
                if let Some(v) = s.audit_previews {
                    audit_previews = v;
                }
            }
        }
        if let Ok(v) = std::env::var("AGW_BIND") {
            bind = v;
        }
        if let Ok(v) = std::env::var("AGW_DATA_DIR") {
            data_dir = PathBuf::from(v);
        }
        if let Ok(v) = std::env::var("AGW_HEARTBEAT_SEC") {
            heartbeat_sec = v
                .parse()
                .map_err(|_| anyhow::anyhow!("AGW_HEARTBEAT_SEC must be an integer"))?;
        }
        if let Ok(v) = std::env::var("AGW_COOKIE_SECURE") {
            cookie_secure = v != "0" && !v.eq_ignore_ascii_case("false");
        }
        if let Ok(v) = std::env::var("AGW_ROUTING_LOG_MAX_MB") {
            routing_log_max_mb = v
                .parse()
                .map_err(|_| anyhow::anyhow!("AGW_ROUTING_LOG_MAX_MB must be an integer"))?;
        }
        if let Ok(v) = std::env::var("AGW_AUDIT_PREVIEWS") {
            audit_previews = v != "0" && !v.eq_ignore_ascii_case("false");
        }
        Ok(Self {
            bind,
            data_dir,
            heartbeat_sec,
            cookie_secure,
            routing_log_max_mb,
            audit_previews,
        })
    }

    pub fn binds_localhost(&self) -> bool {
        self.bind.starts_with("127.0.0.1")
            || self.bind.starts_with("localhost")
            || self.bind.starts_with("[::1]")
    }
}
