use axum::{
    http::StatusCode,
    response::{IntoResponse, Json},
    routing::{get, post},
    Router,
};
use serde::Serialize;

use crate::app::AppState;

mod stats;

async fn push_config(
    axum::extract::State(state): axum::extract::State<AppState>,
    Json(candidate): Json<crate::config::Config>,
) -> impl IntoResponse {
    match state.config.apply(candidate).await {
        Ok(changed) => (
            StatusCode::OK,
            Json(serde_json::json!({"changed": changed})),
        )
            .into_response(),
        Err(err) => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": err.to_string()})),
        )
            .into_response(),
    }
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
        .fallback(api_404_handler)
}
