use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::Instant;

use reqwest::StatusCode;
use reqwest::header::CONTENT_LENGTH;
use tokio::process::Command;
use tokio::sync::{mpsc, watch};
use tokio::time::Duration;
use url::Url;

use crate::config::{AuthConfig, DownloadConfig, ProxyKind};
use crate::engine::ControlState;
use crate::error::{LokiDmError, Result};
use crate::metadata::{
    DownloadMetadata, SegmentMetadata, SegmentState, load_metadata, save_metadata,
};
use crate::types::{DownloadEvent, DownloadProgress};

const CTRL_RUNNING: u8 = 0;
const CTRL_PAUSED: u8 = 1;
const CTRL_CANCELLED: u8 = 2;

pub struct ExternalDownloadResult {
    pub duration_ms: u128,
}

#[must_use]
pub fn needs_external_downloader(config: &DownloadConfig) -> bool {
    let scheme = Url::parse(&config.url)
        .ok()
        .map(|url| url.scheme().to_ascii_lowercase());

    matches!(scheme.as_deref(), Some("ftp") | Some("ftps"))
        || matches!(
            config.auth,
            Some(AuthConfig::Ntlm) | Some(AuthConfig::Kerberos)
        )
}

pub async fn download_with_curl(
    id: u64,
    config: &DownloadConfig,
    part_path: &Path,
    meta_path: &Path,
    events_tx: &mpsc::UnboundedSender<DownloadEvent>,
    control_rx: watch::Receiver<ControlState>,
) -> Result<ExternalDownloadResult> {
    let started = Instant::now();

    if let Some(parent) = config.output_path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }

    if config.output_path.exists() && config.overwrite {
        tokio::fs::remove_file(&config.output_path).await?;
    }

    let resumed = part_path.exists() && meta_path.exists();
    let mut current_size = file_len(part_path).await;

    let mut total_bytes = probe_content_length(config).await.unwrap_or(0);
    let mut metadata = if resumed {
        load_metadata(meta_path).ok()
    } else {
        None
    };

    if let Some(existing_meta) = &metadata
        && total_bytes == 0
    {
        total_bytes = existing_meta.total_size;
    }

    if metadata.is_none() {
        let segments = if total_bytes > 0 {
            vec![SegmentMetadata {
                id: 0,
                start: 0,
                end: total_bytes.saturating_sub(1),
                downloaded: current_size,
                state: SegmentState::InProgress,
            }]
        } else {
            Vec::new()
        };

        let mut created = DownloadMetadata::for_new(
            config.url.clone(),
            config.output_path.clone(),
            part_path.to_path_buf(),
            total_bytes,
            None,
            None,
            segments,
        );
        created.bytes_downloaded = current_size;
        metadata = Some(created);
    }

    if let Some(meta) = metadata.as_mut() {
        meta.bytes_downloaded = current_size;
        if total_bytes > 0 {
            meta.total_size = total_bytes;
        }
    }

    let _ = events_tx.send(DownloadEvent::Started {
        id,
        url: config.url.clone(),
        output_path: config.output_path.clone(),
        total_bytes,
        resumed,
    });

    let control_state = Arc::new(AtomicU8::new(CTRL_RUNNING));
    let ctrl_for_monitor = Arc::clone(&control_state);
    let events_for_monitor = events_tx.clone();
    let mut monitor_rx = control_rx.clone();

    let monitor = tokio::spawn(async move {
        let mut previous = CTRL_RUNNING;
        loop {
            if monitor_rx.changed().await.is_err() {
                break;
            }

            let next = match *monitor_rx.borrow_and_update() {
                ControlState::Running => CTRL_RUNNING,
                ControlState::Paused => CTRL_PAUSED,
                ControlState::Cancelled => CTRL_CANCELLED,
            };

            ctrl_for_monitor.store(next, Ordering::Relaxed);
            if next != previous {
                match next {
                    CTRL_RUNNING => {
                        let _ = events_for_monitor.send(DownloadEvent::Resumed { id });
                    }
                    CTRL_PAUSED => {
                        let _ = events_for_monitor.send(DownloadEvent::Paused { id });
                    }
                    CTRL_CANCELLED => {
                        let _ = events_for_monitor.send(DownloadEvent::Cancelled { id });
                        break;
                    }
                    _ => {}
                }
                previous = next;
            }
        }
    });

    let mut wait_rx = control_rx;
    let result = async {
        loop {
            wait_until_running_or_cancelled(&control_state, &mut wait_rx).await?;

            current_size = file_len(part_path).await;
            let mut child = build_curl_command(config, part_path, current_size)
                .spawn()
                .map_err(|err| {
                    LokiDmError::Message(format!(
                        "failed to launch external curl binary (required for ftp/ntlm/kerberos): {err}"
                    ))
                })?;

            let mut last_bytes = current_size;
            let mut last_tick = Instant::now();
            let mut paused_restart = false;

            loop {
                tokio::time::sleep(Duration::from_millis(500)).await;

                let now_bytes = file_len(part_path).await;
                emit_progress(
                    id,
                    now_bytes,
                    total_bytes,
                    &mut last_bytes,
                    &mut last_tick,
                    events_tx,
                );
                persist_metadata(meta_path, &mut metadata, now_bytes, total_bytes);

                match control_state.load(Ordering::Relaxed) {
                    CTRL_CANCELLED => {
                        let _ = child.kill().await;
                        let _ = child.wait().await;
                        return Err(LokiDmError::Cancelled);
                    }
                    CTRL_PAUSED => {
                        let _ = child.kill().await;
                        let _ = child.wait().await;
                        paused_restart = true;
                        break;
                    }
                    _ => {}
                }

                if let Some(status) = child.try_wait()? {
                    if status.success() {
                        let final_bytes = file_len(part_path).await;
                        persist_metadata(meta_path, &mut metadata, final_bytes, total_bytes);

                        tokio::fs::rename(part_path, &config.output_path).await?;
                        if meta_path.exists() {
                            let _ = tokio::fs::remove_file(meta_path).await;
                        }

                        return Ok(ExternalDownloadResult {
                            duration_ms: started.elapsed().as_millis(),
                        });
                    }

                    if paused_restart {
                        break;
                    }

                    return Err(LokiDmError::Message(format!(
                        "external curl exited with status {}",
                        status.code().map_or_else(|| "signal".to_owned(), |code| code.to_string())
                    )));
                }
            }

            if !paused_restart {
                return Err(LokiDmError::Message(
                    "external transfer stopped unexpectedly".to_owned(),
                ));
            }
        }
    }
    .await;

    monitor.abort();
    let _ = monitor.await;

    result
}

fn build_curl_command(config: &DownloadConfig, part_path: &Path, resume_from: u64) -> Command {
    let mut cmd = Command::new("curl");
    cmd.arg("--location")
        .arg("--fail")
        .arg("--silent")
        .arg("--show-error")
        .arg("--output")
        .arg(part_path)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .stdin(Stdio::null());

    if resume_from > 0 {
        cmd.arg("--continue-at").arg(resume_from.to_string());
    }

    if let Some(limit) = config.speed_limit_bps {
        cmd.arg("--limit-rate").arg(limit.to_string());
    }

    if let Some(agent) = &config.user_agent {
        cmd.arg("--user-agent").arg(agent);
    }

    for (key, value) in &config.headers {
        cmd.arg("--header").arg(format!("{key}: {value}"));
    }

    if let Some(AuthConfig::Bearer { token }) = &config.auth {
        cmd.arg("--header")
            .arg(format!("Authorization: Bearer {token}"));
    }

    if let Some(proxy) = &config.proxy {
        let scheme = match proxy.kind {
            ProxyKind::Http => "http",
            ProxyKind::Https => "https",
            ProxyKind::Socks5 => "socks5h",
        };
        cmd.arg("--proxy")
            .arg(format!("{scheme}://{}:{}", proxy.host, proxy.port));

        if let (Some(user), Some(pass)) = (&proxy.username, &proxy.password) {
            cmd.arg("--proxy-user").arg(format!("{user}:{pass}"));
        }
    }

    match &config.auth {
        Some(AuthConfig::Basic { username, password }) => {
            cmd.arg("--basic")
                .arg("--user")
                .arg(format!("{username}:{password}"));
        }
        Some(AuthConfig::Ntlm) => {
            cmd.arg("--ntlm").arg("--user").arg(":");
        }
        Some(AuthConfig::Kerberos) => {
            cmd.arg("--negotiate").arg("--user").arg(":");
        }
        Some(AuthConfig::Bearer { .. }) | None => {}
    }

    cmd.arg(&config.url);
    cmd
}

fn emit_progress(
    id: u64,
    downloaded: u64,
    total: u64,
    last_bytes: &mut u64,
    last_tick: &mut Instant,
    events_tx: &mpsc::UnboundedSender<DownloadEvent>,
) {
    let elapsed = last_tick.elapsed().as_secs_f64().max(0.001);
    let delta = downloaded.saturating_sub(*last_bytes);
    let speed = delta as f64 / elapsed;

    *last_bytes = downloaded;
    *last_tick = Instant::now();

    let remaining = total.saturating_sub(downloaded);
    let eta_seconds = if speed > 0.0 && total > 0 {
        Some((remaining as f64 / speed) as u64)
    } else {
        None
    };

    let _ = events_tx.send(DownloadEvent::Progress(DownloadProgress {
        id,
        downloaded_bytes: downloaded,
        total_bytes: total,
        speed_bps: speed,
        eta_seconds,
        active_connections: 1,
        target_connections: 1,
    }));
}

fn persist_metadata(
    meta_path: &Path,
    metadata: &mut Option<DownloadMetadata>,
    downloaded: u64,
    total: u64,
) {
    let Some(meta) = metadata.as_mut() else {
        return;
    };

    if total > 0 {
        meta.total_size = total;
    }
    meta.bytes_downloaded = downloaded;

    if let Some(segment) = meta.segments.get_mut(0) {
        segment.downloaded = downloaded.min(segment.len());
        segment.state = if segment.downloaded >= segment.len() {
            SegmentState::Done
        } else {
            SegmentState::InProgress
        };
    }

    let _ = save_metadata(meta_path, meta);
}

async fn file_len(path: &Path) -> u64 {
    tokio::fs::metadata(path).await.map_or(0, |m| m.len())
}

async fn wait_until_running_or_cancelled(
    control_state: &AtomicU8,
    control_rx: &mut watch::Receiver<ControlState>,
) -> Result<()> {
    loop {
        match control_state.load(Ordering::Relaxed) {
            CTRL_RUNNING => return Ok(()),
            CTRL_CANCELLED => return Err(LokiDmError::Cancelled),
            CTRL_PAUSED => {
                if control_rx.changed().await.is_err() {
                    return Err(LokiDmError::Cancelled);
                }
            }
            _ => tokio::time::sleep(Duration::from_millis(100)).await,
        }
    }
}

async fn probe_content_length(config: &DownloadConfig) -> Option<u64> {
    let parsed = Url::parse(&config.url).ok()?;
    if !(parsed.scheme() == "http" || parsed.scheme() == "https") {
        return None;
    }

    let client = reqwest::Client::builder().tcp_nodelay(true).build().ok()?;
    let mut request = client.head(&config.url);

    if let Some(agent) = &config.user_agent {
        request = request.header(reqwest::header::USER_AGENT, agent);
    }

    for (key, value) in &config.headers {
        request = request.header(key, value);
    }

    if let Ok(resp) = request.send().await
        && resp.status() == StatusCode::OK
    {
        return resp
            .headers()
            .get(CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u64>().ok());
    }

    None
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::needs_external_downloader;
    use crate::config::{AuthConfig, DownloadConfig};

    #[test]
    fn ftp_uses_external_downloader() {
        let cfg = DownloadConfig {
            url: "ftp://example.com/file.bin".to_owned(),
            ..DownloadConfig::default()
        };
        assert!(needs_external_downloader(&cfg));
    }

    #[test]
    fn ntlm_uses_external_downloader() {
        let cfg = DownloadConfig {
            url: "https://example.com/file.bin".to_owned(),
            auth: Some(AuthConfig::Ntlm),
            ..DownloadConfig::default()
        };
        assert!(needs_external_downloader(&cfg));
    }
}
