use std::path::Path;

use crate::config::{Applied, SharedConfig};

pub async fn reload_config_from_file(
    path: &Path,
    shared: &SharedConfig,
) -> anyhow::Result<Applied> {
    shared.reload_file(path).await
}
