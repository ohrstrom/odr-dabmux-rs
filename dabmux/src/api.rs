use axum::{
    http::StatusCode,
    response::{IntoResponse, Json},
    routing::{get, post},
    Router,
};
use serde::Serialize;

use crate::app::AppState;

mod live;
mod stats;
mod ui;

async fn push_config(
    axum::extract::State(state): axum::extract::State<AppState>,
    body: axum::body::Bytes,
) -> impl IntoResponse {
    let result = match crate::config::parse_json(&body) {
        Ok(candidate) => state.config.apply(candidate).await,
        Err(err) => Err(err),
    };
    match result {
        Ok(applied) => (
            StatusCode::OK,
            Json(serde_json::json!({"changed": applied.changed})),
        )
            .into_response(),
        Err(err) => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": format!("{err:#}")})),
        )
            .into_response(),
    }
}

/// The active configuration with SubChIds, CU layout and defaults resolved.
async fn resolved_config(
    axum::extract::State(state): axum::extract::State<AppState>,
) -> impl IntoResponse {
    Json(serde_json::to_value(state.config.read().await.resolved()).unwrap_or_default())
}

#[derive(Serialize)]
struct ApiError {
    error: &'static str,
    message: String,
}

async fn api_404_handler() -> impl IntoResponse {
    (
        StatusCode::NOT_FOUND,
        Json(ApiError {
            error: "not_found",
            message: "API endpoint not found".to_string(),
        }),
    )
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/stats/services", get(stats::get_services))
        .route("/stats", get(stats::get_stats))
        .route("/config", post(push_config))
        .route("/config/resolved", get(resolved_config))
        .nest("/ui", ui::router())
        .fallback(api_404_handler)
}
