// retana local config — persisted SSH connection info
// Stored at ~/.retana/config.yaml

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetanaConfig {
    /// Hermes server SSH connection
    pub hermes_host: String,
    pub hermes_port: u16,
    pub hermes_user: String,
    /// Path to SSH private key (None = use default ~/.ssh/id_*)
    pub hermes_key: Option<String>,

    /// Reverse tunnel config
    pub tunnel_remote_port: u16,
    pub tunnel_local_port: u16,

    /// Auto-connect on startup
    pub auto_connect: bool,
}

impl Default for RetanaConfig {
    fn default() -> Self {
        Self {
            hermes_host: "115.159.116.195".into(),
            hermes_port: 22,
            hermes_user: "ubuntu".into(),
            hermes_key: None,
            tunnel_remote_port: 9000,
            tunnel_local_port: 9000,
            auto_connect: true,
        }
    }
}

impl RetanaConfig {
    pub fn config_path() -> PathBuf {
        dirs_next().unwrap_or_else(|| PathBuf::from("."))
            .join(".retana")
            .join("config.yaml")
    }

    pub fn load() -> Result<Self> {
        let path = Self::config_path();
        if path.exists() {
            let content = std::fs::read_to_string(&path)
                .with_context(|| format!("Failed to read {}", path.display()))?;
            serde_yaml::from_str(&content)
                .with_context(|| "Failed to parse config")
        } else {
            let config = Self::default();
            config.save()?;
            Ok(config)
        }
    }

    pub fn save(&self) -> Result<()> {
        let path = Self::config_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let content = serde_yaml::to_string(self)?;
        std::fs::write(&path, content)?;
        log::info!("Config saved to {}", path.display());
        Ok(())
    }
}

fn dirs_next() -> Option<PathBuf> {
    std::env::var("HOME")
        .or_else(|_| {
            #[cfg(target_os = "windows")]
            {
                std::env::var("USERPROFILE")
            }
            #[cfg(not(target_os = "windows"))]
            {
                Err(std::env::VarError::NotPresent)
            }
        })
        .ok()
        .map(PathBuf::from)
}
