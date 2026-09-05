use std::fs::{self, File};
use std::io::{Cursor, Read};
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use axum::body::Body;
use axum::http::{HeaderMap, Request, StatusCode};
use axum::response::Redirect;
use axum::routing::get;
use axum::{Json, Router};
use flate2::Compression;
use flate2::write::GzEncoder;
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tar::{Builder, EntryType, Header};
use tokio::net::TcpListener;
use tower::ServiceExt;

struct Fixture {
    root: PathBuf,
    host_config: PathBuf,
    experiments: PathBuf,
    gateways: PathBuf,
}

static FIXTURE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

impl Fixture {
    fn new() -> Self {
        let nonce = FIXTURE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let root =
            std::env::temp_dir().join(format!("fugue-lab-test-{}-{nonce}", std::process::id()));
        let experiments = root.join("experiments");
        let gateways = root.join("gateways");
        fs::create_dir_all(&experiments).unwrap();
        fs::create_dir_all(&gateways).unwrap();
        let host_config = root.join("host.toml");
        fs::write(
            &host_config,
            format!(
                "experiments_dir = {:?}\ngateways_dir = {:?}\nvm_bridge_host = \"127.0.0.1\"\n",
                experiments, gateways
            ),
        )
        .unwrap();
        Self {
            root,
            host_config,
            experiments,
            gateways,
        }
    }

    fn experiment(&self, id: &str, paused: bool) -> PathBuf {
        let package = self.experiments.join(id);
        fs::create_dir_all(package.join("agents/homes")).unwrap();
        fs::write(
            package.join("fugue.toml"),
            "package config is not an endpoint\n",
        )
        .unwrap();
        if paused {
            fs::write(package.join(".paused"), "paused\n").unwrap();
        }
        self.binding(id, 9);
        package
    }

    fn binding(&self, id: &str, api_port: u16) {
        let gateway = self.gateways.join(id);
        fs::create_dir_all(&gateway).unwrap();
        fs::write(
            gateway.join("binding.json"),
            serde_json::to_vec(&json!({
                "instanceId": id,
                "apiHost": "127.0.0.1",
                "apiPort": api_port
            }))
            .unwrap(),
        )
        .unwrap();
    }

    fn snapshot(&self, id: &str, snapshot_instance: &str) {
        let evidence = self.experiments.join(id).join("evidence");
        fs::create_dir_all(&evidence).unwrap();
        fs::write(
            evidence.join("observer.json"),
            serde_json::to_vec(&json!({
                "instanceId": snapshot_instance,
                "capturedAt": "2026-08-29T12:00:00Z",
                "overview": {"source": "paused-snapshot"},
                "agentsByAddress": {
                    "robinson": {"agent": {"address": "robinson"}, "source": "paused-snapshot"}
                }
            }))
            .unwrap(),
        )
        .unwrap();
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[tokio::test]
async fn catalog_does_not_follow_experiment_symlinks() {
    let fixture = Fixture::new();
    let outside = fixture.root.join("outside");
    fs::create_dir_all(&outside).unwrap();
    fs::write(outside.join("fugue.toml"), "untrusted\n").unwrap();
    symlink(&outside, fixture.experiments.join("escaped")).unwrap();
    fixture.binding("escaped", 9);

    let response = request(&fixture, "/api/lab/experiments").await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(serde_json::from_slice::<Value>(&body).unwrap(), json!([]));
}

#[tokio::test]
async fn catalog_rejects_a_manually_imported_default_package() {
    let fixture = Fixture::new();
    fixture.experiment("default", true);

    let response = request(&fixture, "/api/lab/experiments").await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(serde_json::from_slice::<Value>(&body).unwrap(), json!([]));

    let response = request(&fixture, "/api/lab/experiments/default/overview").await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[test]
fn host_config_rejects_overlapping_roots() {
    let fixture = Fixture::new();
    let nested = fixture.experiments.join("gateways");
    fs::create_dir(&nested).unwrap();
    fs::write(
        &fixture.host_config,
        format!(
            "experiments_dir = {:?}\ngateways_dir = {:?}\nvm_bridge_host = \"127.0.0.1\"\n",
            fixture.experiments, nested
        ),
    )
    .unwrap();
    assert!(fugue_lab::app_from_host_config(&fixture.host_config).is_err());
}

#[tokio::test]
async fn proxy_rejects_a_binding_to_the_wrong_instance() {
    let fixture = Fixture::new();
    fixture.experiment("alpha", false);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    fixture.binding("alpha", port);
    tokio::spawn(async move {
        axum::serve(
            listener,
            Router::new().route(
                "/api/health",
                get(|| async { Json(json!({ "instanceId": "beta" })) }),
            ),
        )
        .await
        .unwrap();
    });

    let response = request(&fixture, "/api/lab/experiments/alpha/overview").await;
    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
}

#[tokio::test]
async fn proxy_does_not_follow_a_health_redirect() {
    let fixture = Fixture::new();
    fixture.experiment("alpha", false);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    fixture.binding("alpha", port);
    let upstream = Router::new()
        .route(
            "/api/health",
            get(|| async { Redirect::temporary("/redirected-health") }),
        )
        .route(
            "/redirected-health",
            get(|| async { Json(json!({ "instanceId": "alpha" })) }),
        );
    tokio::spawn(async move { axum::serve(listener, upstream).await.unwrap() });

    let response = request(&fixture, "/api/lab/experiments/alpha/overview").await;
    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
}

#[tokio::test]
async fn proxy_does_not_follow_an_admin_redirect() {
    let fixture = Fixture::new();
    fixture.experiment("alpha", false);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    fixture.binding("alpha", port);
    let upstream = Router::new()
        .route(
            "/api/health",
            get(|| async { Json(json!({ "instanceId": "alpha" })) }),
        )
        .route(
            "/api/society/admin",
            get(|| async { Redirect::temporary("/redirected-admin") }),
        )
        .route(
            "/redirected-admin",
            get(|| async { Json(json!({ "mustNotBeReached": true })) }),
        );
    tokio::spawn(async move { axum::serve(listener, upstream).await.unwrap() });

    let response = request(&fixture, "/api/lab/experiments/alpha/overview").await;
    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
}

#[tokio::test]
async fn proxy_rejects_a_non_loopback_binding() {
    let fixture = Fixture::new();
    fixture.experiment("alpha", false);
    fs::write(
        fixture.gateways.join("alpha/binding.json"),
        serde_json::to_vec(&json!({
            "instanceId": "alpha",
            "apiHost": "10.0.0.5",
            "apiPort": 49321
        }))
        .unwrap(),
    )
    .unwrap();

    let response = request(&fixture, "/api/lab/experiments/alpha/overview").await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn proxy_uses_verified_local_admin_without_authority() {
    let fixture = Fixture::new();
    fixture.experiment("alpha", false);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    fixture.binding("alpha", port);
    let upstream = Router::new()
        .route(
            "/api/health",
            get(|| async { Json(json!({ "instanceId": "alpha" })) }),
        )
        .route(
            "/api/society/admin",
            get(|headers: HeaderMap| async move {
                assert!(!headers.contains_key("authorization"));
                Json(json!({ "verified": true }))
            }),
        );
    tokio::spawn(async move { axum::serve(listener, upstream).await.unwrap() });

    let response = request(&fixture, "/api/lab/experiments/alpha/overview").await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(
        serde_json::from_slice::<Value>(&body).unwrap(),
        json!({"verified": true})
    );
}

#[tokio::test]
async fn prepared_experiment_is_listed_and_proxied_through_the_live_api() {
    let fixture = Fixture::new();
    let package = fixture.experiment("alpha", false);
    fs::write(package.join(".prepared"), "prepared\n").unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    fixture.binding("alpha", port);
    let upstream = Router::new()
        .route(
            "/api/health",
            get(|| async { Json(json!({ "instanceId": "alpha" })) }),
        )
        .route(
            "/api/society/admin",
            get(|| async { Json(json!({ "source": "prepared-live-api" })) }),
        );
    tokio::spawn(async move { axum::serve(listener, upstream).await.unwrap() });

    let catalog = request(&fixture, "/api/lab/experiments").await;
    assert_eq!(catalog.status(), StatusCode::OK);
    let catalog = catalog.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(
        serde_json::from_slice::<Value>(&catalog).unwrap(),
        json!([{
            "id": "alpha",
            "state": "prepared",
            "apiReachable": true,
            "apiPort": port
        }])
    );

    let overview = request(&fixture, "/api/lab/experiments/alpha/overview").await;
    assert_eq!(overview.status(), StatusCode::OK);
    let overview = overview.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(
        serde_json::from_slice::<Value>(&overview).unwrap(),
        json!({"source": "prepared-live-api"})
    );
}

#[tokio::test]
async fn catalog_rejects_conflicting_paused_and_prepared_markers() {
    let fixture = Fixture::new();
    let package = fixture.experiment("alpha", true);
    fs::write(package.join(".prepared"), "prepared\n").unwrap();

    let catalog = request(&fixture, "/api/lab/experiments").await;
    assert_eq!(catalog.status(), StatusCode::OK);
    let catalog = catalog.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(
        serde_json::from_slice::<Value>(&catalog).unwrap(),
        json!([])
    );

    let overview = request(&fixture, "/api/lab/experiments/alpha/overview").await;
    assert_eq!(overview.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn failed_experiment_is_listed_but_exposes_no_observer_surface() {
    let fixture = Fixture::new();
    let package = fixture.experiment("alpha", false);
    fs::write(package.join(".failed"), "failed\n").unwrap();

    let catalog = request(&fixture, "/api/lab/experiments").await;
    assert_eq!(catalog.status(), StatusCode::OK);
    let catalog = catalog.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(
        serde_json::from_slice::<Value>(&catalog).unwrap(),
        json!([{
            "id": "alpha",
            "state": "failed",
            "apiReachable": false,
            "apiPort": 9
        }])
    );

    let overview = request(&fixture, "/api/lab/experiments/alpha/overview").await;
    assert_eq!(overview.status(), StatusCode::CONFLICT);
    let files = request(&fixture, "/api/lab/experiments/alpha/agents/robinson/files").await;
    assert_eq!(files.status(), StatusCode::CONFLICT);
}

#[tokio::test]
async fn catalog_rejects_failed_marker_conflicts() {
    for conflicting_marker in [".paused", ".prepared"] {
        let fixture = Fixture::new();
        let package = fixture.experiment("alpha", false);
        fs::write(package.join(".failed"), "failed\n").unwrap();
        fs::write(package.join(conflicting_marker), "conflict\n").unwrap();

        let catalog = request(&fixture, "/api/lab/experiments").await;
        let catalog = catalog.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(
            serde_json::from_slice::<Value>(&catalog).unwrap(),
            json!([])
        );
        let overview = request(&fixture, "/api/lab/experiments/alpha/overview").await;
        assert_eq!(overview.status(), StatusCode::NOT_FOUND);
    }
}

#[tokio::test]
async fn catalog_rejects_a_nonregular_failed_marker() {
    let fixture = Fixture::new();
    let package = fixture.experiment("alpha", false);
    let outside = fixture.root.join("outside-failed-marker");
    fs::write(&outside, "failed\n").unwrap();
    symlink(&outside, package.join(".failed")).unwrap();

    let catalog = request(&fixture, "/api/lab/experiments").await;
    let catalog = catalog.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(
        serde_json::from_slice::<Value>(&catalog).unwrap(),
        json!([])
    );
    let overview = request(&fixture, "/api/lab/experiments/alpha/overview").await;
    assert_eq!(overview.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn paused_overview_and_agent_come_from_the_observer_snapshot() {
    let fixture = Fixture::new();
    fixture.experiment("alpha", true);
    fixture.snapshot("alpha", "alpha");

    let overview = request(&fixture, "/api/lab/experiments/alpha/overview").await;
    assert_eq!(overview.status(), StatusCode::OK);
    let overview = overview.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(
        serde_json::from_slice::<Value>(&overview).unwrap(),
        json!({"source": "paused-snapshot"})
    );

    let agent = request(&fixture, "/api/lab/experiments/alpha/agents/robinson").await;
    assert_eq!(agent.status(), StatusCode::OK);
    let agent = agent.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(
        serde_json::from_slice::<Value>(&agent).unwrap(),
        json!({"agent":{"address":"robinson"},"source":"paused-snapshot"})
    );
}

#[tokio::test]
async fn paused_snapshot_rejects_the_wrong_instance() {
    let fixture = Fixture::new();
    fixture.experiment("alpha", true);
    fixture.snapshot("alpha", "beta");

    let response = request(&fixture, "/api/lab/experiments/alpha/overview").await;
    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
}

#[tokio::test]
async fn paused_snapshot_rejects_a_symlink() {
    let fixture = Fixture::new();
    let package = fixture.experiment("alpha", true);
    let evidence = package.join("evidence");
    fs::create_dir(&evidence).unwrap();
    let outside = fixture.root.join("outside-observer.json");
    fs::write(&outside, b"{}").unwrap();
    symlink(&outside, evidence.join("observer.json")).unwrap();

    let response = request(&fixture, "/api/lab/experiments/alpha/overview").await;
    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
}

#[tokio::test]
async fn paused_snapshot_rejects_oversized_content() {
    let fixture = Fixture::new();
    let package = fixture.experiment("alpha", true);
    let evidence = package.join("evidence");
    fs::create_dir(&evidence).unwrap();
    File::create(evidence.join("observer.json"))
        .unwrap()
        .set_len(128 * 1024 * 1024 + 1)
        .unwrap();

    let response = request(&fixture, "/api/lab/experiments/alpha/overview").await;
    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
}

#[tokio::test]
async fn paused_snapshot_rejects_malformed_json() {
    let fixture = Fixture::new();
    let package = fixture.experiment("alpha", true);
    let evidence = package.join("evidence");
    fs::create_dir(&evidence).unwrap();
    fs::write(evidence.join("observer.json"), b"{not-json").unwrap();

    let response = request(&fixture, "/api/lab/experiments/alpha/overview").await;
    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
}

#[tokio::test]
async fn archive_listing_excludes_symlinks_and_oversized_files() {
    let fixture = Fixture::new();
    let package = fixture.experiment("alpha", true);
    let mut archive = archive_builder(&package);
    append_file(&mut archive, "work/note.txt", b"hello");
    let mut link = Header::new_gnu();
    link.set_entry_type(EntryType::Symlink);
    link.set_size(0);
    link.set_mode(0o777);
    link.set_cksum();
    archive
        .append_link(&mut link, "work/link", Path::new("/etc/passwd"))
        .unwrap();
    append_repeated_file(&mut archive, "work/oversized.txt", 10 * 1024 * 1024 + 1);
    finish_archive(archive);

    let response = request(&fixture, "/api/lab/experiments/alpha/agents/robinson/files").await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let body: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        body["files"],
        json!([{"path":"work/note.txt","byteLength":5,"kind":"text"}])
    );
}

#[tokio::test]
async fn archive_rejects_an_expanded_gzip_bomb() {
    let fixture = Fixture::new();
    let package = fixture.experiment("alpha", true);
    let mut archive = archive_builder(&package);
    append_repeated_file(&mut archive, "work/bomb", 33 * 1024 * 1024);
    finish_archive(archive);

    let response = request(&fixture, "/api/lab/experiments/alpha/agents/robinson/files").await;
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn archive_counts_non_file_entries_toward_the_entry_limit() {
    let fixture = Fixture::new();
    let package = fixture.experiment("alpha", true);
    let mut archive = archive_builder(&package);
    for index in 0..10_001 {
        let mut header = Header::new_gnu();
        header.set_entry_type(EntryType::Directory);
        header.set_size(0);
        header.set_mode(0o755);
        header.set_cksum();
        archive
            .append_data(&mut header, format!("metadata-{index}/"), Cursor::new([]))
            .unwrap();
    }
    finish_archive(archive);

    let response = request(&fixture, "/api/lab/experiments/alpha/agents/robinson/files").await;
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn archive_access_requires_a_paused_snapshot() {
    let fixture = Fixture::new();
    fixture.experiment("alpha", false);
    let response = request(&fixture, "/api/lab/experiments/alpha/agents/robinson/files").await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
}

#[tokio::test]
async fn archive_access_does_not_follow_a_homes_directory_symlink() {
    let fixture = Fixture::new();
    let package = fixture.experiment("alpha", true);
    let outside = fixture.root.join("outside-homes");
    fs::create_dir_all(&outside).unwrap();
    fs::remove_dir(package.join("agents/homes")).unwrap();
    symlink(&outside, package.join("agents/homes")).unwrap();

    let response = request(&fixture, "/api/lab/experiments/alpha/agents/robinson/files").await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
}

async fn request(fixture: &Fixture, uri: &str) -> axum::response::Response {
    fugue_lab::app_from_host_config(&fixture.host_config)
        .unwrap()
        .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
        .await
        .unwrap()
}

fn archive_builder(package: &Path) -> Builder<GzEncoder<File>> {
    let archive_path = package.join("agents/homes/robinson.tar.gz");
    Builder::new(GzEncoder::new(
        File::create(archive_path).unwrap(),
        Compression::default(),
    ))
}

fn append_file(archive: &mut Builder<GzEncoder<File>>, path: &str, bytes: &[u8]) {
    let mut header = regular_header(bytes.len() as u64);
    archive
        .append_data(&mut header, path, Cursor::new(bytes))
        .unwrap();
}

fn append_repeated_file(archive: &mut Builder<GzEncoder<File>>, path: &str, size: u64) {
    let mut header = regular_header(size);
    archive
        .append_data(&mut header, path, std::io::repeat(b'x').take(size))
        .unwrap();
}

fn regular_header(size: u64) -> Header {
    let mut header = Header::new_gnu();
    header.set_entry_type(EntryType::Regular);
    header.set_size(size);
    header.set_mode(0o644);
    header.set_cksum();
    header
}

fn finish_archive(archive: Builder<GzEncoder<File>>) {
    archive.into_inner().unwrap().finish().unwrap();
}
