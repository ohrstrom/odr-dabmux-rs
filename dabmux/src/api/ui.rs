//! Endpoints for the web UI: the operator configuration, its preview,
//! replacement and saving, and edits of the ensemble and of single services.
//! Every edit changes a copy of the running configuration, which replaces it
//! only when valid. Applied changes live in memory until the configuration
//! is saved to the config file.
//!
//! Responses carry the configuration revision as `ETag`; requests may send
//! it back as `If-Match` to refuse edits based on an outdated configuration
//! (412), for example after the file was reloaded.

use std::collections::HashSet;

use axum::{
    body::Bytes,
    extract::{Path, Query, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::de::DeserializeOwned;
use serde_json::{json, Value};

use crate::app::AppState;
use crate::config::schema::ServiceConfig;
use crate::config::{service_name, Applied, Config, EditError, EnsembleConfig};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/config", get(get_config).put(replace_config))
        .route("/config/preview", post(preview_config))
        .route("/config/save", post(save_config))
        .route(
            "/ensemble",
            get(get_ensemble)
                .put(replace_ensemble)
                .patch(patch_ensemble),
        )
        .route("/services", get(list_services).post(create_service))
        .route(
            "/services/:sid",
            get(get_service)
                .put(replace_service)
                .patch(patch_service)
                .delete(delete_service),
        )
}

/// The running configuration and whether the configuration file holds it.
async fn get_config(State(state): State<AppState>) -> Response {
    let source = state.config.source().await;
    let file = state.config.file_status().await;
    with_revision(
        source.number,
        json!({ "config": source.config, "file": file }),
    )
}

#[derive(serde::Deserialize)]
struct SaveOptions {
    /// Overwrite the file even if it was changed on disk.
    #[serde(default)]
    force: bool,
}

/// Write the running configuration to the configuration file.
async fn save_config(
    State(state): State<AppState>,
    Query(options): Query<SaveOptions>,
    headers: HeaderMap,
) -> Response {
    let result = async { state.config.save(if_match(&headers)?, options.force).await }.await;
    match result {
        Ok(saved) => Json(json!(saved)).into_response(),
        Err(err) => error_response(err),
    }
}

/// Replace the whole operator configuration, as the web UI does when it
/// applies the changes collected in the browser.
async fn replace_config(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let result = async {
        let candidate: Config = parse_body(&body)?;
        state
            .config
            .edit(if_match(&headers)?, |config| {
                *config = candidate;
                Ok(())
            })
            .await
    }
    .await;
    match result {
        Ok(((), applied)) => applied_response(StatusCode::OK, json!({}), applied),
        Err(err) => error_response(err),
    }
}

/// Validate a configuration without applying it: the resolved view as the
/// mux would run it, and the warnings applying it would give.
async fn preview_config(State(state): State<AppState>, body: Bytes) -> Response {
    let result = async {
        let candidate: Config = parse_body(&body)?;
        let (validated, warnings) = state.config.preview(candidate).await?;
        let resolved = serde_json::to_value(validated.resolved())
            .map_err(|err| EditError::Rejected(err.into()))?;
        Ok::<_, EditError>(json!({ "resolved": resolved, "warnings": warnings }))
    }
    .await;
    match result {
        Ok(body) => Json(body).into_response(),
        Err(err) => error_response(err),
    }
}

async fn get_ensemble(State(state): State<AppState>) -> Response {
    let source = state.config.source().await;
    with_revision(source.number, json!({ "ensemble": source.config.ensemble }))
}

async fn replace_ensemble(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let result = async {
        let ensemble: EnsembleConfig = parse_body(&body)?;
        state
            .config
            .edit(if_match(&headers)?, |config| {
                config.ensemble = ensemble.clone();
                Ok(ensemble)
            })
            .await
    }
    .await;
    edited_ensemble(result)
}

/// JSON Merge Patch (RFC 7396) of the ensemble settings.
async fn patch_ensemble(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let result = async {
        let patch: Value = parse_body(&body)?;
        state
            .config
            .edit(if_match(&headers)?, |config| {
                let mut value = serde_json::to_value(&config.ensemble)
                    .map_err(|err| EditError::Rejected(err.into()))?;
                merge_patch(&mut value, patch);
                config.ensemble = from_value(value)?;
                Ok(config.ensemble.clone())
            })
            .await
    }
    .await;
    edited_ensemble(result)
}

fn edited_ensemble(result: Result<(EnsembleConfig, Applied), EditError>) -> Response {
    match result {
        Ok((ensemble, applied)) => {
            applied_response(StatusCode::OK, json!({ "ensemble": ensemble }), applied)
        }
        Err(err) => error_response(err),
    }
}

async fn list_services(State(state): State<AppState>) -> Response {
    let source = state.config.source().await;
    with_revision(source.number, json!({ "services": source.config.services }))
}

async fn get_service(State(state): State<AppState>, Path(sid): Path<String>) -> Response {
    let result = async {
        let sid = parse_sid(&sid)?;
        let source = state.config.source().await;
        let service =
            find(&source.config, sid).map(|index| source.config.services[index].clone())?;
        Ok::<_, EditError>((source.number, service))
    }
    .await;
    match result {
        Ok((revision, service)) => with_revision(revision, json!({ "service": service })),
        Err(err) => error_response(err),
    }
}

/// Appends the service, so that the SubChIds allocated to the existing
/// services stay as they are.
async fn create_service(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let result = async {
        let service: ServiceConfig = parse_body(&body)?;
        state
            .config
            .edit(if_match(&headers)?, |config| {
                if find(config, service.id).is_ok() {
                    return Err(EditError::Conflict(format!(
                        "service {} exists already",
                        service_name(service.id)
                    )));
                }
                config.services.push(service.clone());
                Ok(service)
            })
            .await
    }
    .await;
    edited(result, StatusCode::CREATED)
}

async fn replace_service(
    State(state): State<AppState>,
    Path(sid): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let result = async {
        let sid = parse_sid(&sid)?;
        let service: ServiceConfig = parse_body(&body)?;
        state
            .config
            .edit(if_match(&headers)?, |config| replace(config, sid, service))
            .await
    }
    .await;
    edited(result, StatusCode::OK)
}

/// JSON Merge Patch (RFC 7396) of the service: objects are merged, `null`
/// removes a field, and arrays such as `components` are replaced whole.
async fn patch_service(
    State(state): State<AppState>,
    Path(sid): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let result = async {
        let sid = parse_sid(&sid)?;
        let patch: Value = parse_body(&body)?;
        state
            .config
            .edit(if_match(&headers)?, |config| {
                let index = find(config, sid)?;
                let mut value = serde_json::to_value(&config.services[index])
                    .map_err(|err| EditError::Rejected(err.into()))?;
                merge_patch(&mut value, patch);
                let service: ServiceConfig = from_value(value)?;
                replace(config, sid, service)
            })
            .await
    }
    .await;
    edited(result, StatusCode::OK)
}

/// Shared subchannels that only this service used are removed with it.
async fn delete_service(
    State(state): State<AppState>,
    Path(sid): Path<String>,
    headers: HeaderMap,
) -> Response {
    let result = async {
        let sid = parse_sid(&sid)?;
        state
            .config
            .edit(if_match(&headers)?, |config| {
                let index = find(config, sid)?;
                let removed = config.services.remove(index);
                let used: HashSet<&String> = config
                    .services
                    .iter()
                    .flat_map(|s| &s.components)
                    .filter_map(|c| c.subchannel.as_ref())
                    .collect();
                let orphans: Vec<String> = removed
                    .components
                    .iter()
                    .filter_map(|c| c.subchannel.clone())
                    .filter(|name| !used.contains(name))
                    .collect();
                for name in &orphans {
                    config.subchannels.remove(name);
                }
                Ok(json!({ "removed_subchannels": orphans }))
            })
            .await
    }
    .await;
    match result {
        Ok((removed, applied)) => applied_response(StatusCode::OK, removed, applied),
        Err(err) => error_response(err),
    }
}

/// Replace service `sid`; the replacement may carry a new, unused SId.
fn replace(
    config: &mut Config,
    sid: u32,
    service: ServiceConfig,
) -> Result<ServiceConfig, EditError> {
    let index = find(config, sid)?;
    if service.id != sid && find(config, service.id).is_ok() {
        return Err(EditError::Conflict(format!(
            "service {} exists already",
            service_name(service.id)
        )));
    }
    config.services[index] = service.clone();
    Ok(service)
}

fn find(config: &Config, sid: u32) -> Result<usize, EditError> {
    config
        .services
        .iter()
        .position(|s| s.id == sid)
        .ok_or_else(|| EditError::NotFound(format!("no service {}", service_name(sid))))
}

/// SIds in URLs are hexadecimal, as displayed: `4DA4`, `0x4DA4` or `E1C0FFEE`.
fn parse_sid(raw: &str) -> Result<u32, EditError> {
    let digits = raw.trim_start_matches("0x").trim_start_matches("0X");
    u32::from_str_radix(digits, 16)
        .map_err(|_| EditError::NotFound(format!("{raw} is not a hexadecimal service id")))
}

fn if_match(headers: &HeaderMap) -> Result<Option<u64>, EditError> {
    let Some(value) = headers.get(header::IF_MATCH) else {
        return Ok(None);
    };
    value
        .to_str()
        .ok()
        .and_then(|v| {
            v.trim()
                .trim_start_matches("W/")
                .trim_matches('"')
                .parse()
                .ok()
        })
        .map(Some)
        .ok_or_else(|| EditError::Stale("If-Match must be a configuration revision".into()))
}

fn parse_body<T: DeserializeOwned>(body: &[u8]) -> Result<T, EditError> {
    serde_path_to_error::deserialize(&mut serde_json::Deserializer::from_slice(body))
        .map_err(|err| EditError::Rejected(anyhow::anyhow!("{}: {}", err.path(), err.inner())))
}

fn from_value<T: DeserializeOwned>(value: Value) -> Result<T, EditError> {
    serde_path_to_error::deserialize(value)
        .map_err(|err| EditError::Rejected(anyhow::anyhow!("{}: {}", err.path(), err.inner())))
}

fn merge_patch(target: &mut Value, patch: Value) {
    let Value::Object(patch) = patch else {
        *target = patch;
        return;
    };
    if !target.is_object() {
        *target = Value::Object(Default::default());
    }
    let Value::Object(target) = target else {
        unreachable!()
    };
    for (key, value) in patch {
        if value.is_null() {
            target.remove(&key);
        } else {
            merge_patch(target.entry(key).or_insert(Value::Null), value);
        }
    }
}

fn edited(result: Result<(ServiceConfig, Applied), EditError>, status: StatusCode) -> Response {
    match result {
        Ok((service, applied)) => applied_response(status, json!({ "service": service }), applied),
        Err(err) => error_response(err),
    }
}

fn applied_response(status: StatusCode, mut body: Value, applied: Applied) -> Response {
    body["changed"] = applied.changed.into();
    body["revision"] = applied.revision.into();
    body["warnings"] = applied.warnings.into();
    let mut response = (status, Json(body)).into_response();
    set_etag(&mut response, applied.revision);
    response
}

fn with_revision(revision: u64, mut body: Value) -> Response {
    body["revision"] = revision.into();
    let mut response = Json(body).into_response();
    set_etag(&mut response, revision);
    response
}

fn set_etag(response: &mut Response, revision: u64) {
    if let Ok(value) = format!("\"{revision}\"").parse() {
        response.headers_mut().insert(header::ETAG, value);
    }
}

fn error_response(err: EditError) -> Response {
    let status = match err {
        EditError::NotFound(_) => StatusCode::NOT_FOUND,
        EditError::Conflict(_) => StatusCode::CONFLICT,
        EditError::Stale(_) => StatusCode::PRECONDITION_FAILED,
        EditError::Rejected(_) => StatusCode::UNPROCESSABLE_ENTITY,
    };
    (status, Json(json!({ "error": err.to_string() }))).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_patch_merges_objects_removes_nulls_and_replaces_arrays() {
        let mut target = json!({"label": "A", "short_label": "a", "pty": 1, "components": [1, 2]});
        merge_patch(
            &mut target,
            json!({"label": "B", "short_label": null, "components": [3]}),
        );
        assert_eq!(target, json!({"label": "B", "pty": 1, "components": [3]}));
    }

    #[test]
    fn service_ids_are_hexadecimal() {
        assert_eq!(parse_sid("4DA4").unwrap(), 0x4da4);
        assert_eq!(parse_sid("0xe1c0ffee").unwrap(), 0xe1c0_ffee);
        assert!(parse_sid("radio").is_err());
    }
}
