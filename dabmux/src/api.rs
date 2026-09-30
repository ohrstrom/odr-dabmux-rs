use axum::{
    http::StatusCode,
    response::{IntoResponse, Json},
    routing::get,
    Router,
};
use serde::Serialize;

use crate::app::AppState;

mod stats;

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
        .fallback(api_404_handler)
}
