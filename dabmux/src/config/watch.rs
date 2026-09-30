use std::path::PathBuf;

use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use tokio::sync::mpsc;

use crate::config::reload::reload_config_from_file;
use crate::config::SharedConfig;

pub async fn watch_config_file(path: PathBuf, config: SharedConfig) -> anyhow::Result<()> {
    let path = crate::config::resolve_path(path)?;
    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("config path has no parent: {}", path.display()))?;

    let (tx, mut rx) = mpsc::unbounded_channel::<Result<Event, notify::Error>>();

    let mut watcher = RecommendedWatcher::new(
        move |res| {
            let _ = tx.send(res);
        },
        notify::Config::default(),
    )?;
    watcher.watch(parent, RecursiveMode::NonRecursive)?;
    tracing::info!(
        path = %path.display(),
        "watching config file for changes"
    );

    while let Some(res) = rx.recv().await {
        match res {
            Ok(event) => {
                if !matches!(
                    event.kind,
                    EventKind::Modify(_) | EventKind::Create(_) | EventKind::Remove(_)
                ) || !event.paths.iter().any(|event_path| event_path == &path)
                {
                    continue;
                }

                if !path.exists() {
                    tracing::warn!(
                        path = %path.display(),
                        "config file missing during reload"
                    );
                    continue;
                }

                match reload_config_from_file(&path, &config).await {
                    Ok(true) => tracing::info!("config reloaded successfully"),
                    Ok(false) => tracing::trace!("config reload ignored (content unchanged)"),
                    Err(e) => {
                        tracing::warn!("config reload failed: {e}");
                    }
                }
            }
            Err(e) => {
                tracing::warn!("config watch error: {e}");
            }
        }
    }

    Ok(())
}
