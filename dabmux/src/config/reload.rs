use std::path::Path;

use crate::config::{read_file, Applied, SharedConfig};

pub async fn reload_config_from_file(
    path: &Path,
    shared: &SharedConfig,
) -> anyhow::Result<Applied> {
    shared.apply(read_file(path)?).await
}
