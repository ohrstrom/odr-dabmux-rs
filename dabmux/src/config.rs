use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Context;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

pub mod reload;
pub mod watch;

#[derive(Clone)]
pub struct SharedConfig(Arc<RwLock<Config>>);

impl SharedConfig {
    pub fn new(config: Config) -> Self {
        Self(Arc::new(RwLock::new(config)))
    }

    pub async fn read(&self) -> tokio::sync::RwLockReadGuard<'_, Config> {
        self.0.read().await
    }

    async fn replace_if_changed(&self, config: Config) -> bool {
        let mut current = self.0.write().await;
        if *current == config {
            return false;
        }
        *current = config;
        true
    }
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    pub services: Vec<ServiceConfig>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ServiceConfig {
    pub sid: String,
    pub bitrate: u32,
}

pub fn load_from_file(path: &Path) -> anyhow::Result<Config> {
    if !path.exists() {
        anyhow::bail!("config file does not exist: {}", path.display());
    }
    if !path.is_file() {
        anyhow::bail!("config path is not a file: {}", path.display());
    }

    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read config {}", path.display()))?;
    let config = serde_yaml::from_str(&raw)
        .with_context(|| format!("failed to parse config {}", path.display()))?;

    tracing::info!(path = %path.display(), "loaded config file");
    Ok(config)
}

pub fn resolve_path(path: PathBuf) -> anyhow::Result<PathBuf> {
    if path.is_absolute() {
        Ok(path)
    } else {
        Ok(std::env::current_dir()?.join(path))
    }
}
