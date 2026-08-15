use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct Config {
    pub bind: String,
    pub data_dir: PathBuf,
    pub heartbeat_sec: u64,
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
}

impl Config {
    /// config.toml (optional) < environment variables < defaults.
    pub fn load() -> anyhow::Result<Self> {
        let mut bind = "127.0.0.1:9920".to_string();
        let mut data_dir = PathBuf::from("data");
        let mut heartbeat_sec = 30u64;

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
        Ok(Self {
            bind,
            data_dir,
            heartbeat_sec,
        })
    }

    pub fn binds_localhost(&self) -> bool {
        self.bind.starts_with("127.0.0.1")
            || self.bind.starts_with("localhost")
            || self.bind.starts_with("[::1]")
    }
}
