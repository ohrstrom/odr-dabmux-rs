use axum::{extract::State, Json};

use crate::app::AppState;
use crate::config::ServiceConfig;
use crate::runtime::StatsSnapshot;

pub async fn get_services(State(state): State<AppState>) -> Json<Vec<ServiceConfig>> {
    Json(state.config.read().await.source.services.clone())
}

pub async fn get_stats(State(state): State<AppState>) -> Json<StatsSnapshot> {
    Json(state.stats.snapshot())
}
