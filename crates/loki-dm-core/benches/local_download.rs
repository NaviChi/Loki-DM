#![allow(clippy::expect_used, clippy::map_unwrap_or, clippy::unwrap_used)]

use std::path::PathBuf;
use std::time::Instant;

use loki_dm_core::{DownloadConfig, DownloadEngine, EngineSettings};

fn main() {
    let runtime = tokio::runtime::Runtime::new().expect("runtime");
    runtime.block_on(async move {
        let url = std::env::var("LOKI_BENCH_URL")
            .unwrap_or_else(|_| "https://proof.ovh.net/files/1Gb.dat".to_owned());
        let output = std::env::var("LOKI_BENCH_OUTPUT")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("./loki-bench.bin"));

        let engine = DownloadEngine::new(EngineSettings::default()).expect("engine");
        let cfg = DownloadConfig {
            url,
            output_path: output,
            initial_connections: 8,
            max_connections: 16,
            min_connections: 2,
            overwrite: true,
            ..DownloadConfig::default()
        };

        let start = Instant::now();
        let mut handle = engine.start(cfg).expect("start");
        handle.wait().await.expect("wait");
        let elapsed = start.elapsed();

        println!("local_download_bench elapsed_ms={}", elapsed.as_millis());
    });
}
