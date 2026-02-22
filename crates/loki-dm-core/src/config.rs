use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EngineSettings {
    pub global_speed_limit_bps: Option<u64>,
    pub max_parallel_downloads: usize,
    pub connect_timeout_secs: u64,
    pub request_timeout_secs: u64,
    pub default_user_agent: String,
}

impl Default for EngineSettings {
    fn default() -> Self {
        Self {
            global_speed_limit_bps: None,
            max_parallel_downloads: 128,
            connect_timeout_secs: 10,
            request_timeout_secs: 30,
            default_user_agent: "Loki-DM/0.1 (+https://github.com/navi/loki-dm)".to_owned(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ProxyKind {
    Http,
    Https,
    Socks5,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProxyConfig {
    pub kind: ProxyKind,
    pub host: String,
    pub port: u16,
    pub username: Option<String>,
    pub password: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AuthConfig {
    Basic { username: String, password: String },
    Bearer { token: String },
    Ntlm,
    Kerberos,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadConfig {
    pub url: String,
    pub mirror_urls: Vec<String>,
    pub output_path: PathBuf,
    pub initial_connections: u16,
    pub min_connections: u16,
    pub max_connections: u16,
    pub min_segment_size: u64,
    pub max_retries: u8,
    pub speed_limit_bps: Option<u64>,
    pub hour_quota_mb: Option<u64>,
    pub overwrite: bool,
    pub headers: BTreeMap<String, String>,
    pub proxy: Option<ProxyConfig>,
    pub auth: Option<AuthConfig>,
    pub user_agent: Option<String>,
    pub category: Option<String>,
    pub av_hook_command: Option<String>,
}

impl DownloadConfig {
    #[must_use]
    pub fn normalized(mut self) -> Self {
        self.min_connections = self.min_connections.max(1);
        self.max_connections = self.max_connections.max(self.min_connections);
        self.initial_connections = self
            .initial_connections
            .clamp(self.min_connections, self.max_connections);
        self.min_segment_size = self.min_segment_size.max(128 * 1024);
        self.mirror_urls = self
            .mirror_urls
            .into_iter()
            .map(|v| v.trim().to_owned())
            .filter(|v| !v.is_empty())
            .filter(|v| v != &self.url)
            .collect();
        self
    }
}

impl Default for DownloadConfig {
    fn default() -> Self {
        Self {
            url: String::new(),
            mirror_urls: Vec::new(),
            output_path: PathBuf::from("download.bin"),
            initial_connections: 8,
            min_connections: 2,
            max_connections: 16,
            min_segment_size: 512 * 1024,
            max_retries: 6,
            speed_limit_bps: None,
            hour_quota_mb: None,
            overwrite: false,
            headers: BTreeMap::new(),
            proxy: None,
            auth: None,
            user_agent: None,
            category: None,
            av_hook_command: None,
        }
    }
}
