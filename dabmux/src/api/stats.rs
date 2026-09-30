use axum::{extract::State, Json};
use serde::Serialize;

use crate::app::AppState;
use crate::config::ServiceConfig;

#[derive(Serialize)]
pub struct StatsResponse {
    services: Vec<ServiceConfig>,
}

pub async fn get_services(State(state): State<AppState>) -> Json<Vec<ServiceConfig>> {
    Json(state.config.read().await.services.clone())
}

pub async fn get_stats(State(state): State<AppState>) -> Json<StatsResponse> {
    let services = state.config.read().await.services.clone();
    Json(StatsResponse { services })
}
