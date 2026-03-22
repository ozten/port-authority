use serde::Deserialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub daemon: DaemonConfig,
    #[serde(default)]
    pub allocation: AllocationConfig,
    #[serde(default)]
    pub tunnel: TunnelConfig,
    #[serde(default)]
    pub lease: LeaseConfig,
}

#[derive(Debug, Deserialize)]
pub struct DaemonConfig {
    #[serde(default = "default_socket_path")]
    pub socket_path: PathBuf,
    #[serde(default = "default_db_path")]
    pub db_path: PathBuf,
    #[serde(default = "default_log_level")]
    pub log_level: String,
}

#[derive(Debug, Deserialize)]
pub struct AllocationConfig {
    #[serde(default = "default_port_range_start")]
    pub port_range_start: u16,
    #[serde(default = "default_port_range_end")]
    pub port_range_end: u16,
}

#[derive(Debug, Deserialize)]
pub struct TunnelConfig {
    #[serde(default = "default_health_check_interval")]
    pub health_check_interval_secs: u64,
    #[serde(default = "default_health_check_timeout")]
    pub health_check_timeout_secs: u64,
    #[serde(default = "default_max_reconnect_attempts")]
    pub max_reconnect_attempts: u32,
    #[serde(default = "default_max_connections_per_tunnel")]
    pub max_connections_per_tunnel: usize,
    #[serde(default = "default_ssh_keepalive_interval")]
    pub ssh_keepalive_interval_secs: u64,
}

#[derive(Debug, Deserialize)]
pub struct LeaseConfig {
    #[serde(default)]
    pub default_lease_secs: u64,
    #[serde(default = "default_cleanup_interval")]
    pub cleanup_interval_secs: u64,
    #[serde(default = "default_released_ttl")]
    pub released_ttl_secs: u64,
}

/// SSH configuration for VMs, loaded from a separate ssh.toml file.
#[derive(Debug, Deserialize)]
pub struct SshConfig {
    #[serde(default)]
    pub vms: HashMap<String, VmSshConfig>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct VmSshConfig {
    pub host: String,
    #[serde(default = "default_ssh_port")]
    pub port: u16,
    pub user: String,
    pub key: PathBuf,
}

// Defaults

fn default_socket_path() -> PathBuf {
    if let Ok(runtime_dir) = std::env::var("XDG_RUNTIME_DIR") {
        PathBuf::from(runtime_dir).join("portd.sock")
    } else {
        dirs_fallback().join("portd.sock")
    }
}

fn default_db_path() -> PathBuf {
    if let Ok(data_home) = std::env::var("XDG_DATA_HOME") {
        PathBuf::from(data_home).join("portd").join("portd.db")
    } else {
        home_dir().join(".local/share/portd/portd.db")
    }
}

fn default_log_level() -> String {
    "info".to_string()
}

fn default_port_range_start() -> u16 {
    10000
}

fn default_port_range_end() -> u16 {
    60000
}

fn default_health_check_interval() -> u64 {
    30
}

fn default_health_check_timeout() -> u64 {
    5
}

fn default_max_reconnect_attempts() -> u32 {
    5
}

fn default_max_connections_per_tunnel() -> usize {
    100
}

fn default_ssh_keepalive_interval() -> u64 {
    30
}

fn default_cleanup_interval() -> u64 {
    60
}

fn default_released_ttl() -> u64 {
    86400
}

fn default_ssh_port() -> u16 {
    22
}

fn home_dir() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/tmp"))
}

fn dirs_fallback() -> PathBuf {
    home_dir().join(".local/share/portd")
}

impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            socket_path: default_socket_path(),
            db_path: default_db_path(),
            log_level: default_log_level(),
        }
    }
}

impl Default for AllocationConfig {
    fn default() -> Self {
        Self {
            port_range_start: default_port_range_start(),
            port_range_end: default_port_range_end(),
        }
    }
}

impl Default for TunnelConfig {
    fn default() -> Self {
        Self {
            health_check_interval_secs: default_health_check_interval(),
            health_check_timeout_secs: default_health_check_timeout(),
            max_reconnect_attempts: default_max_reconnect_attempts(),
            max_connections_per_tunnel: default_max_connections_per_tunnel(),
            ssh_keepalive_interval_secs: default_ssh_keepalive_interval(),
        }
    }
}

impl Default for LeaseConfig {
    fn default() -> Self {
        Self {
            default_lease_secs: 0,
            cleanup_interval_secs: default_cleanup_interval(),
            released_ttl_secs: default_released_ttl(),
        }
    }
}

impl Config {
    /// Load config from the default path (~/.config/portd/portd.toml).
    /// Returns default config if the file doesn't exist.
    pub fn load() -> anyhow::Result<Self> {
        let config_path = config_dir().join("portd.toml");
        Self::load_from(&config_path)
    }

    pub fn load_from(path: &Path) -> anyhow::Result<Self> {
        if path.exists() {
            let contents = std::fs::read_to_string(path)?;
            let config: Config = toml::from_str(&contents)?;
            Ok(config)
        } else {
            Ok(Config::default())
        }
    }

    /// Load SSH VM configs from the default path (~/.config/portd/ssh.toml).
    pub fn load_ssh_config() -> anyhow::Result<SshConfig> {
        let ssh_path = config_dir().join("ssh.toml");
        if ssh_path.exists() {
            let contents = std::fs::read_to_string(&ssh_path)?;
            let config: SshConfig = toml::from_str(&contents)?;
            Ok(config)
        } else {
            Ok(SshConfig {
                vms: HashMap::new(),
            })
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            daemon: DaemonConfig::default(),
            allocation: AllocationConfig::default(),
            tunnel: TunnelConfig::default(),
            lease: LeaseConfig::default(),
        }
    }
}

fn config_dir() -> PathBuf {
    if let Ok(config_home) = std::env::var("XDG_CONFIG_HOME") {
        PathBuf::from(config_home).join("portd")
    } else {
        home_dir().join(".config/portd")
    }
}
