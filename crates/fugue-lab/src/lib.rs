mod archive;
mod catalog;
mod config;
mod snapshot;

use std::net::IpAddr;
use std::sync::Arc;

use archive::{ArchiveReader, FileKind, is_unsafe_browser_path};
use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use catalog::{ExperimentCatalog, ExperimentState, valid_agent};
use config::HostConfig;
use serde::Deserialize;
use serde_json::json;
use snapshot::ObserverSnapshot;

#[derive(Clone)]
struct AppState {
    catalog: ExperimentCatalog,
}

pub fn app_from_env() -> anyhow::Result<Router> {
    app(HostConfig::load()?)
}

pub fn app_from_host_config(path: &std::path::Path) -> anyhow::Result<Router> {
    app(HostConfig::from_path(path)?)
}

fn app(host: HostConfig) -> anyhow::Result<Router> {
    let state = Arc::new(AppState {
        catalog: ExperimentCatalog::new(host)?,
    });
    Ok(Router::new()
        .route("/api/lab/health", get(health))
        .route("/api/lab/experiments", get(list_experiments))
        .route("/api/lab/experiments/{id}/overview", get(proxy_overview))
        .route(
            "/api/lab/experiments/{id}/agents/{address}",
            get(proxy_agent),
        )
        .route(
            "/api/lab/experiments/{id}/agents/{address}/files",
            get(list_files),
        )
        .route(
            "/api/lab/experiments/{id}/agents/{address}/files/{*path}",
            get(read_file),
        )
        .with_state(state))
}

pub fn bind_address_from_env() -> anyhow::Result<(IpAddr, u16)> {
    let host = std::env::var("FUGUE_LAB_HOST").unwrap_or_else(|_| "127.0.0.1".to_owned());
    let port = std::env::var("FUGUE_LAB_PORT").unwrap_or_else(|_| "49320".to_owned());
    Ok((host.parse()?, port.parse()?))
}

async fn health() -> Json<serde_json::Value> {
    Json(json!({ "status": "ok" }))
}

async fn list_experiments(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<catalog::ExperimentSummary>>, LabError> {
    Ok(Json(
        state.catalog.list().await.map_err(LabError::internal)?,
    ))
}

async fn proxy_overview(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Response, LabError> {
    read_admin(&state, &id, None).await
}

async fn proxy_agent(
    State(state): State<Arc<AppState>>,
    Path((id, address)): Path<(String, String)>,
) -> Result<Response, LabError> {
    if !valid_agent(&address) {
        return Err(LabError::bad_request("invalid agent address"));
    }
    read_admin(&state, &id, Some(&address)).await
}

async fn read_admin(
    state: &AppState,
    id: &str,
    address: Option<&str>,
) -> Result<Response, LabError> {
    let experiment = state
        .catalog
        .resolve(id)
        .map_err(|_| LabError::not_found("experiment not found"))?;
    if experiment.state == ExperimentState::Failed {
        return Err(LabError::conflict(
            "failed experiments do not expose observer data",
        ));
    }
    if experiment.state == ExperimentState::Paused {
        let package = experiment.package.clone();
        let instance_id = experiment.id.clone();
        let address = address.map(str::to_owned);
        let value = tokio::task::spawn_blocking(move || {
            let snapshot = ObserverSnapshot::load(&package, &instance_id)?;
            match address {
                Some(address) => snapshot.agent(&address),
                None => Ok(snapshot.overview()),
            }
        })
        .await
        .map_err(|_| LabError::internal("observer snapshot reader failed"))?
        .map_err(|error| LabError::bad_gateway(&error.to_string()))?;
        return json_response(value);
    }

    let path = match address {
        Some(address) => format!("/api/society/admin/agents/{address}"),
        None => "/api/society/admin".to_owned(),
    };
    proxy_active_admin(state, &experiment, &path).await
}

async fn proxy_active_admin(
    state: &AppState,
    experiment: &catalog::Experiment,
    path: &str,
) -> Result<Response, LabError> {
    const MAX_PROXY_BYTES: usize = 20 * 1024 * 1024;

    state
        .catalog
        .verify_instance(experiment)
        .await
        .map_err(|_| LabError::bad_gateway("experiment API identity check failed"))?;
    let url = state.catalog.url(experiment, path);
    let mut upstream = state
        .catalog
        .client()
        .get(url)
        .send()
        .await
        .map_err(|_| LabError::bad_gateway("experiment API is unreachable"))?;
    if !upstream.status().is_success() {
        return Err(LabError::bad_gateway(
            "experiment API rejected the observer request",
        ));
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = upstream
        .chunk()
        .await
        .map_err(|_| LabError::bad_gateway("experiment API returned an incomplete response"))?
    {
        if bytes.len().saturating_add(chunk.len()) > MAX_PROXY_BYTES {
            return Err(LabError::bad_gateway(
                "experiment API response exceeds the observer limit",
            ));
        }
        bytes.extend_from_slice(&chunk);
    }
    let mut response = Response::new(Body::from(bytes));
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    Ok(response)
}

fn json_response(value: serde_json::Value) -> Result<Response, LabError> {
    let bytes = serde_json::to_vec(&value)
        .map_err(|_| LabError::internal("observer snapshot JSON could not be encoded"))?;
    let mut response = Response::new(Body::from(bytes));
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    Ok(response)
}

async fn list_files(
    State(state): State<Arc<AppState>>,
    Path((id, address)): Path<(String, String)>,
) -> Result<Json<archive::FileList>, LabError> {
    let path = stopped_home_path(&state, &id, &address)?;
    let files = tokio::task::spawn_blocking(move || ArchiveReader::open(path)?.list())
        .await
        .map_err(|_| LabError::internal("stopped-home reader failed"))?
        .map_err(|_| LabError::internal("stopped-home snapshot could not be read"))?;
    Ok(Json(files))
}

#[derive(Deserialize, Default)]
struct FileQuery {
    download: Option<String>,
}

async fn read_file(
    State(state): State<Arc<AppState>>,
    Path((id, address, path)): Path<(String, String, String)>,
    Query(query): Query<FileQuery>,
) -> Result<Response, LabError> {
    let download = query.download.as_deref() == Some("1");
    let archive_path = stopped_home_path(&state, &id, &address)?;
    let requested_path = path.clone();
    let file = tokio::task::spawn_blocking(move || {
        ArchiveReader::open(archive_path)?.read(&requested_path, download)
    })
    .await
    .map_err(|_| LabError::internal("stopped-home reader failed"))?
    .map_err(|error| {
        let message = error.to_string();
        if message.contains("binary files") {
            LabError::unsupported(&message)
        } else if message.contains("size limit") {
            LabError::payload_too_large(&message)
        } else {
            LabError::not_found("archive file not found")
        }
    })?;
    let mut response = Response::new(Body::from(file.bytes));
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(match file.kind {
            FileKind::Text => "text/plain; charset=utf-8",
            FileKind::Binary => "application/octet-stream",
        }),
    );
    headers.insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        "x-source-byte-length",
        HeaderValue::from_str(&file.total_length.to_string()).expect("integer header"),
    );
    if file.truncated {
        headers.insert("x-content-truncated", HeaderValue::from_static("true"));
    }
    if download || is_unsafe_browser_path(&path) {
        headers.insert(
            header::CONTENT_DISPOSITION,
            HeaderValue::from_static("attachment; filename=\"download\""),
        );
    }
    Ok(response)
}

fn stopped_home_path(
    state: &AppState,
    id: &str,
    address: &str,
) -> Result<std::path::PathBuf, LabError> {
    if !valid_agent(address) {
        return Err(LabError::bad_request("invalid agent address"));
    }
    let experiment = state
        .catalog
        .resolve(id)
        .map_err(|_| LabError::not_found("experiment not found"))?;
    if experiment.state != ExperimentState::Paused {
        return Err(LabError::conflict(
            "stopped-home files are available only while the experiment is paused",
        ));
    }
    let agents = experiment.package.join("agents");
    let homes = agents.join("homes");
    if !regular_directory(&agents) || !regular_directory(&homes) {
        return Err(LabError::conflict("stopped-home snapshot is missing"));
    }
    let archive = homes.join(format!("{address}.tar.gz"));
    let metadata = std::fs::symlink_metadata(&archive)
        .map_err(|_| LabError::conflict("stopped-home snapshot is missing"))?;
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
        return Err(LabError::conflict("stopped-home snapshot is missing"));
    }
    Ok(archive)
}

fn regular_directory(path: &std::path::Path) -> bool {
    std::fs::symlink_metadata(path)
        .is_ok_and(|metadata| metadata.file_type().is_dir() && !metadata.file_type().is_symlink())
}

struct LabError {
    status: StatusCode,
    message: String,
}

impl LabError {
    fn bad_request(message: &str) -> Self {
        Self::new(StatusCode::BAD_REQUEST, message)
    }

    fn not_found(message: &str) -> Self {
        Self::new(StatusCode::NOT_FOUND, message)
    }

    fn conflict(message: &str) -> Self {
        Self::new(StatusCode::CONFLICT, message)
    }

    fn unsupported(message: &str) -> Self {
        Self::new(StatusCode::UNSUPPORTED_MEDIA_TYPE, message)
    }

    fn payload_too_large(message: &str) -> Self {
        Self::new(StatusCode::PAYLOAD_TOO_LARGE, message)
    }

    fn bad_gateway(message: &str) -> Self {
        Self::new(StatusCode::BAD_GATEWAY, message)
    }

    fn internal(error: impl std::fmt::Display) -> Self {
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, &error.to_string())
    }

    fn new(status: StatusCode, message: &str) -> Self {
        Self {
            status,
            message: message.to_owned(),
        }
    }
}

impl IntoResponse for LabError {
    fn into_response(self) -> Response {
        (self.status, Json(json!({ "error": self.message }))).into_response()
    }
}
