#![allow(
    clippy::cast_sign_loss,
    clippy::cast_possible_truncation,
    clippy::expect_used,
    clippy::items_after_statements,
    clippy::unwrap_used
)]

use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::extract::State;
use axum::http::header::{ACCEPT_RANGES, CONTENT_LENGTH, CONTENT_RANGE, RANGE};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::Response;
use axum::routing::get;
use loki_dm_core::metadata::{build_initial_segments, save_metadata, sidecar_paths};
use loki_dm_core::{DownloadConfig, DownloadEngine, EngineSettings};
use tempfile::tempdir;
use tokio::net::TcpListener;

#[derive(Clone)]
struct AppState {
    payload: Arc<Vec<u8>>,
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn segmented_download_completes() {
    let payload = Arc::new((0..2_400_000).map(|i| (i % 251) as u8).collect::<Vec<_>>());
    let server = spawn_server(Arc::clone(&payload)).await;

    let dir = tempdir().expect("tempdir");
    let output = dir.path().join("sample.bin");

    let engine = DownloadEngine::new(EngineSettings::default()).expect("engine");
    let cfg = DownloadConfig {
        url: format!("{server}/file"),
        output_path: output.clone(),
        initial_connections: 8,
        max_connections: 12,
        min_connections: 2,
        overwrite: true,
        ..DownloadConfig::default()
    };

    let mut handle = engine.start(cfg).expect("start download");
    let path = handle.wait().await.expect("download should succeed");

    let downloaded = std::fs::read(path).expect("read output");
    assert_eq!(downloaded, *payload);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn resumes_from_existing_lokidm_and_part() {
    let payload = Arc::new((0..1_800_000).map(|i| (i % 199) as u8).collect::<Vec<_>>());
    let server = spawn_server(Arc::clone(&payload)).await;

    let dir = tempdir().expect("tempdir");
    let output = dir.path().join("resume.bin");
    let (part_path, meta_path) = sidecar_paths(&output);

    {
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&part_path)
            .expect("create part");
        use std::io::Write;
        f.write_all(&payload[..900_000]).expect("seed part");
        f.set_len(payload.len() as u64).expect("set len");
    }

    let mut segments = build_initial_segments(payload.len() as u64, 8);
    for seg in &mut segments {
        if seg.end < 900_000 {
            seg.downloaded = seg.len();
            seg.state = loki_dm_core::metadata::SegmentState::Done;
        }
    }

    let mut metadata = loki_dm_core::metadata::DownloadMetadata::for_new(
        format!("{server}/file"),
        output.clone(),
        part_path.clone(),
        payload.len() as u64,
        None,
        None,
        segments,
    );
    metadata.normalize_for_resume();
    save_metadata(&meta_path, &metadata).expect("save metadata");

    let engine = DownloadEngine::new(EngineSettings::default()).expect("engine");
    let cfg = DownloadConfig {
        url: format!("{server}/file"),
        output_path: output.clone(),
        initial_connections: 8,
        max_connections: 12,
        min_connections: 2,
        overwrite: true,
        ..DownloadConfig::default()
    };

    let mut handle = engine.start(cfg).expect("start resume");
    handle.wait().await.expect("resume should complete");

    let downloaded = std::fs::read(output).expect("read output");
    assert_eq!(downloaded, *payload);
}

async fn spawn_server(payload: Arc<Vec<u8>>) -> String {
    let state = AppState { payload };
    let app = Router::new()
        .route("/file", get(file_handler).head(file_handler))
        .with_state(state);

    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");

    tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve");
    });

    format!("http://{addr}")
}

async fn file_handler(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let total = state.payload.len() as u64;

    if let Some(range_header) = headers.get(RANGE).and_then(|v| v.to_str().ok())
        && let Some((start, end)) = parse_range(range_header, total)
    {
        let bytes = state.payload[start as usize..=end as usize].to_vec();
        let mut response = Response::new(Body::from(bytes));
        *response.status_mut() = StatusCode::PARTIAL_CONTENT;

        response.headers_mut().insert(
            CONTENT_RANGE,
            HeaderValue::from_str(&format!("bytes {start}-{end}/{total}")).expect("content-range"),
        );
        response
            .headers_mut()
            .insert(ACCEPT_RANGES, HeaderValue::from_static("bytes"));
        response.headers_mut().insert(
            CONTENT_LENGTH,
            HeaderValue::from_str(&(end - start + 1).to_string()).expect("content-length"),
        );
        return response;
    }

    let mut response = Response::new(Body::from(state.payload.as_ref().clone()));
    *response.status_mut() = StatusCode::OK;
    response
        .headers_mut()
        .insert(ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    response.headers_mut().insert(
        CONTENT_LENGTH,
        HeaderValue::from_str(&total.to_string()).expect("content-length"),
    );
    response
}

fn parse_range(value: &str, total: u64) -> Option<(u64, u64)> {
    let value = value.strip_prefix("bytes=")?;
    let (start, end) = value.split_once('-')?;
    let start = start.parse::<u64>().ok()?;
    let end = if end.is_empty() {
        total.saturating_sub(1)
    } else {
        end.parse::<u64>().ok()?.min(total.saturating_sub(1))
    };
    (start <= end && end < total).then_some((start, end))
}
