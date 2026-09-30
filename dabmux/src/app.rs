use std::net::SocketAddr;
use std::sync::Arc;

use axum::Router;
use tokio::net::TcpListener;
use tower_http::normalize_path::NormalizePathLayer;

use crate::api;
use crate::args::Args;
use crate::config::{self, SharedConfig};
use crate::runtime::{self, RuntimeStats};

pub struct App {
    args: Args,
    state: AppState,
}

#[derive(Clone)]
pub struct AppState {
    pub config: SharedConfig,
    pub stats: Arc<RuntimeStats>,
}

impl App {
    pub async fn new(args: Args) -> anyhow::Result<Self> {
        let config_path = args
            .config
            .clone()
            .map(config::resolve_path)
            .transpose()?
            .ok_or_else(|| anyhow::anyhow!("--config is required"))?;
        let initial_config = config::load_from_file(&config_path)?;

        let state = AppState {
            config: SharedConfig::new(initial_config),
            stats: Arc::new(RuntimeStats::default()),
        };

        if args.watch_config {
            let state = state.clone();
            tokio::spawn(async move {
                if let Err(e) = config::watch::watch_config_file(config_path, state.config).await {
                    tracing::error!("config watcher failed: {e}");
                }
            });
        }

        Ok(Self { args, state })
    }

    fn router(&self) -> Router {
        Router::new()
            .nest("/api", api::router())
            .with_state(self.state.clone())
            .layer(NormalizePathLayer::trim_trailing_slash())
    }

    pub async fn run(self) -> anyhow::Result<()> {
        let addr: SocketAddr = format!("{}:{}", self.args.host, self.args.port).parse()?;

        tracing::info!(%addr, "binding TCP listener");
        let listener = TcpListener::bind(addr).await?;

        tracing::debug!("starting server on: {:?}", listener);

        let config = self.state.config.clone();
        let stats = self.state.stats.clone();
        tokio::select! {
            result = async {
                tokio::try_join!(
                    async {
                        axum::serve(listener, self.router())
                            .await
                            .map_err(anyhow::Error::from)
                    },
                    runtime::run(config, stats)
                )
            } => { result?; }
            result = tokio::signal::ctrl_c() => {
                result?;
                tracing::info!("shutdown requested");
            }
        }

        Ok(())
    }
}
