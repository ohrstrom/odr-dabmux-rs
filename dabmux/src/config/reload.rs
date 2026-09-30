use std::path::Path;

use crate::config::{load_from_file, SharedConfig};

pub async fn reload_config_from_file(path: &Path, shared: &SharedConfig) -> anyhow::Result<bool> {
    let config = load_from_file(path)?;
    shared.replace_if_changed(config).await
}
