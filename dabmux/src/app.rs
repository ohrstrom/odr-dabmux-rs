use std::net::SocketAddr;

use axum::Router;
use tokio::net::TcpListener;
use tower_http::normalize_path::NormalizePathLayer;

use crate::api;
use crate::args::Args;
use crate::config::{self, Config, SharedConfig};

pub struct App {
    args: Args,
    state: AppState,
}

#[derive(Clone)]
pub struct AppState {
    pub config: SharedConfig,
}

impl App {
    pub async fn new(args: Args) -> anyhow::Result<Self> {
        let config_path = args.config.clone().map(config::resolve_path).transpose()?;
        let initial_config = if let Some(path) = &args.config {
            config::load_from_file(path)?
        } else {
            tracing::info!("no config file provided");
            Config::default()
        };

        let state = AppState {
            config: SharedConfig::new(initial_config),
        };

        if args.watch_config {
            if let Some(path) = config_path {
                let state = state.clone();

                tokio::spawn(async move {
                    if let Err(e) = config::watch::watch_config_file(path, state.config).await {
                        tracing::error!("config watcher failed: {e}");
                    }
                });
            } else {
                tracing::warn!("--watch-config set but no --config provided");
            }
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

        axum::serve(listener, self.router()).await?;

        Ok(())
    }
}
