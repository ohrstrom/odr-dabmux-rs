//! The web UI (`web-ui/dist`), compiled into release builds and served on `/`.
//! Debug builds read the files from disk, so a rebuilt UI shows on reload.

use axum::{
    http::{header, StatusCode, Uri},
    response::{IntoResponse, Response},
};
use rust_embed::Embed;

#[derive(Embed)]
#[folder = "../web-ui/dist/"]
#[allow_missing = true]
struct Assets;

pub async fn serve(uri: Uri) -> Response {
    let path = match uri.path().trim_start_matches('/') {
        "" => "index.html",
        path => path,
    };
    match Assets::get(path) {
        Some(file) => (
            [(header::CONTENT_TYPE, file.metadata.mimetype().to_owned())],
            file.data,
        )
            .into_response(),
        None if path == "index.html" => (
            StatusCode::NOT_FOUND,
            "web UI not built: run `bun ./build.ts` in web-ui/ and rebuild dabmux",
        )
            .into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}
