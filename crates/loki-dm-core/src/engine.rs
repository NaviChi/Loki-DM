use std::collections::VecDeque;
use std::fs::OpenOptions;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU16, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use rand::Rng;
use reqwest::header::{
    ACCEPT_RANGES, CONTENT_LENGTH, CONTENT_RANGE, ETAG, IF_RANGE, LAST_MODIFIED, RANGE,
};
use reqwest::{Client, Proxy, StatusCode};
use tokio::sync::{Mutex, Notify, mpsc, watch};
use tokio::task::{JoinHandle, JoinSet};

use crate::av::run_av_hook;
use crate::config::{AuthConfig, DownloadConfig, EngineSettings, ProxyConfig, ProxyKind};
use crate::error::{LokiDmError, Result};
use crate::external_downloader::{download_with_curl, needs_external_downloader};
use crate::metadata::{
    DownloadMetadata, SegmentMetadata, SegmentState, build_initial_segments, load_metadata,
    save_metadata, sidecar_paths,
};
use crate::rate_limit::RateLimiter;
use crate::types::{DownloadEvent, DownloadProgress};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlState {
    Running,
    Paused,
    Cancelled,
}

pub struct DownloadHandle {
    id: u64,
    control_tx: watch::Sender<ControlState>,
    events: mpsc::UnboundedReceiver<DownloadEvent>,
    join: Option<JoinHandle<Result<PathBuf>>>,
}

impl DownloadHandle {
    #[must_use]
    pub fn id(&self) -> u64 {
        self.id
    }

    pub fn pause(&self) -> Result<()> {
        self.control_tx
            .send(ControlState::Paused)
            .map_err(|_| LokiDmError::Message("failed to pause download".to_owned()))
    }

    pub fn resume(&self) -> Result<()> {
        self.control_tx
            .send(ControlState::Running)
            .map_err(|_| LokiDmError::Message("failed to resume download".to_owned()))
    }

    pub fn cancel(&self) -> Result<()> {
        self.control_tx
            .send(ControlState::Cancelled)
            .map_err(|_| LokiDmError::Message("failed to cancel download".to_owned()))
    }

    pub fn try_recv_event(
        &mut self,
    ) -> std::result::Result<DownloadEvent, tokio::sync::mpsc::error::TryRecvError> {
        self.events.try_recv()
    }

    pub async fn recv_event(&mut self) -> Option<DownloadEvent> {
        self.events.recv().await
    }

    pub async fn wait(&mut self) -> Result<PathBuf> {
        let Some(join) = self.join.take() else {
            return Err(LokiDmError::Message(
                "download already awaited for completion".to_owned(),
            ));
        };

        join.await?
    }
}

struct RemoteProbe {
    total_size: u64,
    range_supported: bool,
    etag: Option<String>,
    last_modified: Option<String>,
}

struct SharedState {
    metadata: Mutex<DownloadMetadata>,
    queue: Mutex<VecDeque<usize>>,
    queue_notify: Notify,
    stop: AtomicBool,
    cancelled: AtomicBool,
    total_downloaded: AtomicU64,
    active_connections: AtomicU16,
    target_connections: AtomicU16,
    error_counter: AtomicU64,
    quota: Mutex<QuotaWindow>,
    hourly_quota_bytes: Option<u64>,
}

#[derive(Debug)]
struct QuotaWindow {
    started_at: Instant,
    bytes_used: u64,
}

#[derive(Clone)]
pub struct DownloadEngine {
    settings: EngineSettings,
    base_client: Client,
    next_id: Arc<AtomicU64>,
    global_limiter: Arc<RateLimiter>,
}

impl DownloadEngine {
    pub fn new(settings: EngineSettings) -> Result<Self> {
        let base_client = Client::builder()
            .connect_timeout(Duration::from_secs(settings.connect_timeout_secs))
            .timeout(Duration::from_secs(settings.request_timeout_secs))
            .tcp_nodelay(true)
            .user_agent(settings.default_user_agent.clone())
            .build()?;

        Ok(Self {
            global_limiter: Arc::new(RateLimiter::new(settings.global_speed_limit_bps)),
            settings,
            base_client,
            next_id: Arc::new(AtomicU64::new(1)),
        })
    }

    pub fn start(&self, config: DownloadConfig) -> Result<DownloadHandle> {
        let normalized = config.normalized();
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let client = self.build_download_client(&normalized)?;
        let (events_tx, events_rx) = mpsc::unbounded_channel();
        let (control_tx, control_rx) = watch::channel(ControlState::Running);
        let global_limiter = Arc::clone(&self.global_limiter);
        let cfg = Arc::new(normalized);

        let join = tokio::spawn(run_download(
            id,
            client,
            cfg,
            global_limiter,
            events_tx,
            control_rx,
        ));

        Ok(DownloadHandle {
            id,
            control_tx,
            events: events_rx,
            join: Some(join),
        })
    }

    fn build_download_client(&self, config: &DownloadConfig) -> Result<Client> {
        if config.proxy.is_none() && config.user_agent.is_none() {
            return Ok(self.base_client.clone());
        }

        let mut builder = Client::builder()
            .connect_timeout(Duration::from_secs(self.settings.connect_timeout_secs))
            .timeout(Duration::from_secs(self.settings.request_timeout_secs))
            .tcp_nodelay(true);

        let agent = config
            .user_agent
            .as_deref()
            .unwrap_or(&self.settings.default_user_agent);
        builder = builder.user_agent(agent.to_owned());

        if let Some(proxy_cfg) = &config.proxy {
            builder = builder.proxy(build_proxy(proxy_cfg)?);
        }

        Ok(builder.build()?)
    }
}

async fn run_download(
    id: u64,
    client: Client,
    config: Arc<DownloadConfig>,
    global_limiter: Arc<RateLimiter>,
    events_tx: mpsc::UnboundedSender<DownloadEvent>,
    control_rx: watch::Receiver<ControlState>,
) -> Result<PathBuf> {
    let started_at = Instant::now();
    let (part_path, meta_path) = sidecar_paths(&config.output_path);
    let candidate_sources = source_urls(&config);

    if let Some(parent) = config.output_path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }

    if config.output_path.exists() && !config.overwrite && !meta_path.exists() {
        return Err(LokiDmError::Message(format!(
            "destination already exists: {}",
            config.output_path.display()
        )));
    }

    if needs_external_downloader(&config) {
        let external = match download_with_curl(
            id,
            &config,
            &part_path,
            &meta_path,
            &events_tx,
            control_rx.clone(),
        )
        .await
        {
            Ok(outcome) => outcome,
            Err(LokiDmError::Cancelled) => return Err(LokiDmError::Cancelled),
            Err(err) => {
                let _ = events_tx.send(DownloadEvent::Failed {
                    id,
                    error: err.to_string(),
                });
                return Err(err);
            }
        };

        if let Some(hook) = &config.av_hook_command {
            match run_av_hook(hook, &config.output_path).await {
                Ok(result) => {
                    let _ = events_tx.send(DownloadEvent::HookExecuted {
                        id,
                        command: result.command,
                        success: result.success,
                        code: result.code,
                        stderr: result.stderr,
                    });
                }
                Err(err) => {
                    let _ = events_tx.send(DownloadEvent::HookExecuted {
                        id,
                        command: hook.clone(),
                        success: false,
                        code: None,
                        stderr: err.to_string(),
                    });
                }
            }
        }

        let _ = events_tx.send(DownloadEvent::Completed {
            id,
            output_path: config.output_path.clone(),
            duration_ms: external.duration_ms.max(started_at.elapsed().as_millis()),
        });

        return Ok(config.output_path.clone());
    }

    let (selected_url, probe, source_count) = select_best_source(&client, &config).await?;
    let can_multi = probe.range_supported && probe.total_size > config.min_segment_size;

    let resumed = meta_path.exists() && part_path.exists();
    let mut metadata = if resumed {
        let mut loaded = load_metadata(&meta_path)?;
        if !candidate_sources
            .iter()
            .any(|candidate| candidate == &loaded.url)
            || loaded.total_size != probe.total_size
        {
            return Err(LokiDmError::Message(
                "resume metadata does not match source URL or remote size".to_owned(),
            ));
        }

        loaded.url = selected_url.clone();
        loaded.etag = probe.etag.clone();
        loaded.last_modified = probe.last_modified.clone();
        loaded
    } else {
        if config.output_path.exists() && config.overwrite {
            tokio::fs::remove_file(&config.output_path).await?;
        }

        let initial_connections = if can_multi {
            config.initial_connections
        } else {
            1
        };

        DownloadMetadata::for_new(
            selected_url.clone(),
            config.output_path.clone(),
            part_path.clone(),
            probe.total_size,
            probe.etag,
            probe.last_modified,
            build_initial_segments(probe.total_size, initial_connections),
        )
    };

    metadata.normalize_for_resume();

    let part_file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&part_path)?;
    part_file.set_len(metadata.total_size)?;

    let mut queue = VecDeque::new();
    for segment in &metadata.segments {
        if !segment.is_complete() {
            queue.push_back(segment.id);
        }
    }

    let max_connections = if can_multi { config.max_connections } else { 1 };
    let min_connections = if can_multi {
        config.min_connections.min(max_connections)
    } else {
        1
    };
    let initial_target = if can_multi {
        config
            .initial_connections
            .clamp(min_connections, max_connections)
    } else {
        1
    };

    let initial_downloaded = metadata.bytes_downloaded;

    let shared = Arc::new(SharedState {
        metadata: Mutex::new(metadata),
        queue: Mutex::new(queue),
        queue_notify: Notify::new(),
        stop: AtomicBool::new(false),
        cancelled: AtomicBool::new(false),
        total_downloaded: AtomicU64::new(initial_downloaded),
        active_connections: AtomicU16::new(0),
        target_connections: AtomicU16::new(initial_target),
        error_counter: AtomicU64::new(0),
        quota: Mutex::new(QuotaWindow {
            started_at: Instant::now(),
            bytes_used: 0,
        }),
        hourly_quota_bytes: config
            .hour_quota_mb
            .map(|mb| mb.saturating_mul(1024 * 1024)),
    });

    {
        let snapshot = shared.metadata.lock().await.clone();
        save_metadata(&meta_path, &snapshot)?;
        if source_count > 1 {
            let _ = events_tx.send(DownloadEvent::MirrorSelected {
                id,
                url: selected_url,
                source_count,
            });
        }
        let _ = events_tx.send(DownloadEvent::Started {
            id,
            url: config.url.clone(),
            output_path: config.output_path.clone(),
            total_bytes: snapshot.total_size,
            resumed,
        });
    }

    let monitor_task = tokio::spawn(control_event_monitor(
        id,
        events_tx.clone(),
        control_rx.clone(),
    ));
    let progress_task = tokio::spawn(progress_reporter(
        id,
        Arc::clone(&shared),
        events_tx.clone(),
    ));
    let saver_task = tokio::spawn(metadata_saver(Arc::clone(&shared), meta_path.clone()));
    let adaptive_task = tokio::spawn(adaptive_connection_loop(
        id,
        Arc::clone(&shared),
        Arc::clone(&config),
        events_tx.clone(),
        can_multi,
        min_connections,
        max_connections,
    ));

    let file = Arc::new(part_file);
    let download_limiter = Arc::new(RateLimiter::new(config.speed_limit_bps));
    let mut workers = JoinSet::new();
    for worker in 0..max_connections {
        workers.spawn(worker_loop(
            worker,
            id,
            client.clone(),
            Arc::clone(&config),
            Arc::clone(&shared),
            events_tx.clone(),
            control_rx.clone(),
            Arc::clone(&global_limiter),
            Arc::clone(&download_limiter),
            Arc::clone(&file),
            can_multi,
        ));
    }

    let mut worker_error: Option<LokiDmError> = None;
    while let Some(result) = workers.join_next().await {
        match result {
            Ok(Ok(())) => {}
            Ok(Err(LokiDmError::Cancelled)) => {
                shared.cancelled.store(true, Ordering::Relaxed);
            }
            Ok(Err(err)) => {
                worker_error = Some(err);
                shared.stop.store(true, Ordering::Relaxed);
                shared.queue_notify.notify_waiters();
                break;
            }
            Err(err) => {
                worker_error = Some(err.into());
                shared.stop.store(true, Ordering::Relaxed);
                shared.queue_notify.notify_waiters();
                break;
            }
        }
    }

    shared.stop.store(true, Ordering::Relaxed);
    shared.queue_notify.notify_waiters();

    monitor_task.abort();
    let _ = monitor_task.await;
    let _ = progress_task.await;
    let _ = saver_task.await;
    let _ = adaptive_task.await;

    if shared.cancelled.load(Ordering::Relaxed)
        || matches!(*control_rx.borrow(), ControlState::Cancelled)
    {
        let _ = events_tx.send(DownloadEvent::Cancelled { id });
        return Err(LokiDmError::Cancelled);
    }

    if let Some(err) = worker_error {
        let _ = events_tx.send(DownloadEvent::Failed {
            id,
            error: err.to_string(),
        });
        return Err(err);
    }

    let snapshot = shared.metadata.lock().await.clone();
    if !snapshot.is_complete() {
        let err =
            LokiDmError::Message("download ended before all segments were complete".to_owned());
        let _ = events_tx.send(DownloadEvent::Failed {
            id,
            error: err.to_string(),
        });
        return Err(err);
    }

    save_metadata(&meta_path, &snapshot)?;

    if config.output_path.exists() && config.overwrite {
        tokio::fs::remove_file(&config.output_path).await?;
    }
    tokio::fs::rename(&part_path, &config.output_path).await?;
    if meta_path.exists() {
        tokio::fs::remove_file(&meta_path).await?;
    }

    if let Some(hook) = &config.av_hook_command {
        match run_av_hook(hook, &config.output_path).await {
            Ok(result) => {
                let _ = events_tx.send(DownloadEvent::HookExecuted {
                    id,
                    command: result.command,
                    success: result.success,
                    code: result.code,
                    stderr: result.stderr,
                });
            }
            Err(err) => {
                let _ = events_tx.send(DownloadEvent::HookExecuted {
                    id,
                    command: hook.clone(),
                    success: false,
                    code: None,
                    stderr: err.to_string(),
                });
            }
        }
    }

    let _ = events_tx.send(DownloadEvent::Completed {
        id,
        output_path: config.output_path.clone(),
        duration_ms: started_at.elapsed().as_millis(),
    });

    Ok(config.output_path.clone())
}

async fn worker_loop(
    worker_idx: u16,
    id: u64,
    client: Client,
    config: Arc<DownloadConfig>,
    shared: Arc<SharedState>,
    events_tx: mpsc::UnboundedSender<DownloadEvent>,
    mut control_rx: watch::Receiver<ControlState>,
    global_limiter: Arc<RateLimiter>,
    local_limiter: Arc<RateLimiter>,
    part_file: Arc<std::fs::File>,
    can_multi: bool,
) -> Result<()> {
    loop {
        if shared.stop.load(Ordering::Relaxed) {
            return Ok(());
        }

        wait_if_running(&mut control_rx).await?;

        let target = shared.target_connections.load(Ordering::Relaxed);
        if worker_idx >= target {
            if all_segments_done(&shared).await {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
            continue;
        }

        let segment_id = match claim_segment(&shared).await {
            Some(v) => v,
            None => {
                if all_segments_done(&shared).await {
                    return Ok(());
                }

                let notified = shared.queue_notify.notified();
                tokio::select! {
                    _ = notified => {}
                    _ = tokio::time::sleep(Duration::from_millis(150)) => {}
                }
                continue;
            }
        };

        shared.active_connections.fetch_add(1, Ordering::Relaxed);
        let segment_result = download_segment_with_retries(
            id,
            segment_id,
            &client,
            &config,
            &shared,
            &events_tx,
            &mut control_rx,
            &global_limiter,
            &local_limiter,
            &part_file,
            can_multi,
        )
        .await;
        shared.active_connections.fetch_sub(1, Ordering::Relaxed);

        match segment_result {
            Ok(()) => {}
            Err(LokiDmError::Cancelled) => {
                shared.cancelled.store(true, Ordering::Relaxed);
                shared.stop.store(true, Ordering::Relaxed);
                shared.queue_notify.notify_waiters();
                return Err(LokiDmError::Cancelled);
            }
            Err(err) => {
                shared.error_counter.fetch_add(1, Ordering::Relaxed);
                shared.stop.store(true, Ordering::Relaxed);
                shared.queue_notify.notify_waiters();
                return Err(err);
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn download_segment_with_retries(
    id: u64,
    segment_id: usize,
    client: &Client,
    config: &DownloadConfig,
    shared: &Arc<SharedState>,
    events_tx: &mpsc::UnboundedSender<DownloadEvent>,
    control_rx: &mut watch::Receiver<ControlState>,
    global_limiter: &RateLimiter,
    local_limiter: &RateLimiter,
    part_file: &std::fs::File,
    can_multi: bool,
) -> Result<()> {
    let mut attempt: u8 = 0;

    loop {
        wait_if_running(control_rx).await?;

        match download_segment_once(
            segment_id,
            client,
            config,
            shared,
            control_rx,
            global_limiter,
            local_limiter,
            part_file,
            can_multi,
        )
        .await
        {
            Ok(()) => return Ok(()),
            Err(err) => {
                if matches!(err, LokiDmError::Cancelled) {
                    return Err(err);
                }

                attempt = attempt.saturating_add(1);
                if attempt > config.max_retries {
                    set_segment_pending(shared, segment_id).await;
                    enqueue_segment(shared, segment_id).await;
                    return Err(err);
                }

                set_segment_pending(shared, segment_id).await;
                enqueue_segment(shared, segment_id).await;

                let wait_ms = backoff_with_jitter(attempt);
                let _ = events_tx.send(DownloadEvent::Retrying {
                    id,
                    segment_id,
                    attempt,
                    wait_ms,
                    reason: err.to_string(),
                });

                tokio::time::sleep(Duration::from_millis(wait_ms)).await;
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn download_segment_once(
    segment_id: usize,
    client: &Client,
    config: &DownloadConfig,
    shared: &Arc<SharedState>,
    control_rx: &mut watch::Receiver<ControlState>,
    global_limiter: &RateLimiter,
    local_limiter: &RateLimiter,
    part_file: &std::fs::File,
    can_multi: bool,
) -> Result<()> {
    let (url, etag, last_modified, range_start, range_end) = {
        let meta = shared.metadata.lock().await;
        let segment = meta
            .segments
            .get(segment_id)
            .ok_or_else(|| LokiDmError::Message("invalid segment id".to_owned()))?;

        let start = segment.start.saturating_add(segment.downloaded);
        (
            meta.url.clone(),
            meta.etag.clone(),
            meta.last_modified.clone(),
            start,
            segment.end,
        )
    };

    if range_start > range_end {
        mark_segment_done(shared, segment_id).await;
        return Ok(());
    }

    let mut request = client.get(url);

    if can_multi {
        request = request.header(RANGE, format!("bytes={range_start}-{range_end}"));
    }

    if let Some(etag) = etag {
        request = request.header(IF_RANGE, etag);
    } else if let Some(last_modified) = last_modified {
        request = request.header(IF_RANGE, last_modified);
    }

    request = apply_auth(request, &config.auth)?;

    for (key, value) in &config.headers {
        request = request.header(key, value);
    }

    let response = request.send().await?;
    let status = response.status();

    if !status.is_success() {
        return Err(LokiDmError::HttpStatus(status));
    }

    if can_multi && range_start > 0 && status != StatusCode::PARTIAL_CONTENT {
        return Err(LokiDmError::RangeNotHonored);
    }

    let mut stream = response.bytes_stream();
    while let Some(chunk_result) = stream.next().await {
        wait_if_running(control_rx).await?;

        let chunk = chunk_result?;
        if chunk.is_empty() {
            continue;
        }

        global_limiter.acquire(chunk.len()).await;
        local_limiter.acquire(chunk.len()).await;
        enforce_quota(shared, chunk.len() as u64).await;

        let Some((offset, len)) = next_write_window(shared, segment_id, chunk.len()).await else {
            mark_segment_done(shared, segment_id).await;
            break;
        };

        if len == 0 {
            mark_segment_done(shared, segment_id).await;
            break;
        }

        write_all_at(part_file, &chunk[..len], offset)?;
        apply_written(shared, segment_id, len as u64).await;
    }

    if is_segment_complete(shared, segment_id).await {
        mark_segment_done(shared, segment_id).await;
        return Ok(());
    }

    Err(LokiDmError::Message(
        "segment ended before expected byte range was complete".to_owned(),
    ))
}

async fn progress_reporter(
    id: u64,
    shared: Arc<SharedState>,
    events_tx: mpsc::UnboundedSender<DownloadEvent>,
) {
    let mut last_bytes = shared.total_downloaded.load(Ordering::Relaxed);
    let mut last_tick = Instant::now();

    loop {
        tokio::time::sleep(Duration::from_millis(500)).await;

        let now = Instant::now();
        let elapsed = now
            .saturating_duration_since(last_tick)
            .as_secs_f64()
            .max(0.001);
        last_tick = now;

        let downloaded = shared.total_downloaded.load(Ordering::Relaxed);
        let delta = downloaded.saturating_sub(last_bytes);
        last_bytes = downloaded;

        let speed = delta as f64 / elapsed;
        let (total, active, target) = {
            let meta = shared.metadata.lock().await;
            (
                meta.total_size,
                shared.active_connections.load(Ordering::Relaxed),
                shared.target_connections.load(Ordering::Relaxed),
            )
        };

        let remaining = total.saturating_sub(downloaded);
        let eta = (speed > 0.0).then_some((remaining as f64 / speed) as u64);

        let _ = events_tx.send(DownloadEvent::Progress(DownloadProgress {
            id,
            downloaded_bytes: downloaded,
            total_bytes: total,
            speed_bps: speed,
            eta_seconds: eta,
            active_connections: active,
            target_connections: target,
        }));

        if shared.stop.load(Ordering::Relaxed) {
            break;
        }
    }
}

async fn metadata_saver(shared: Arc<SharedState>, meta_path: PathBuf) {
    loop {
        tokio::time::sleep(Duration::from_millis(700)).await;

        let snapshot = shared.metadata.lock().await.clone();
        let _ = save_metadata(&meta_path, &snapshot);

        if shared.stop.load(Ordering::Relaxed) {
            break;
        }
    }
}

async fn adaptive_connection_loop(
    id: u64,
    shared: Arc<SharedState>,
    config: Arc<DownloadConfig>,
    events_tx: mpsc::UnboundedSender<DownloadEvent>,
    can_multi: bool,
    min_connections: u16,
    max_connections: u16,
) {
    if !can_multi {
        return;
    }

    let mut last_bytes = shared.total_downloaded.load(Ordering::Relaxed);
    let mut last_time = Instant::now();
    let mut previous_speed = 0.0_f64;

    loop {
        tokio::time::sleep(Duration::from_secs(2)).await;
        if shared.stop.load(Ordering::Relaxed) {
            break;
        }

        let now = Instant::now();
        let elapsed = now
            .saturating_duration_since(last_time)
            .as_secs_f64()
            .max(0.001);
        last_time = now;

        let bytes = shared.total_downloaded.load(Ordering::Relaxed);
        let delta = bytes.saturating_sub(last_bytes);
        last_bytes = bytes;

        let speed = delta as f64 / elapsed;
        let current = shared.target_connections.load(Ordering::Relaxed);
        let mut next = current;

        if previous_speed > 0.0 {
            if speed > previous_speed * 1.15 && current < max_connections {
                next = next.saturating_add(1).min(max_connections);
            } else if speed < previous_speed * 0.65 && current > min_connections {
                next = next.saturating_sub(1).max(min_connections);
            }
        }

        if shared.error_counter.swap(0, Ordering::Relaxed) > 0 && next > min_connections {
            next = next.saturating_sub(1).max(min_connections);
        }

        if next != current {
            shared.target_connections.store(next, Ordering::Relaxed);
            if next > current {
                for _ in 0..(next - current) {
                    if !split_largest_remaining_segment(&shared, config.min_segment_size).await {
                        break;
                    }
                }
                shared.queue_notify.notify_waiters();
            }

            let _ = events_tx.send(DownloadEvent::ConnectionsAdjusted {
                id,
                from: current,
                to: next,
            });
        }

        previous_speed = speed.max(1.0);
    }
}

async fn control_event_monitor(
    id: u64,
    events_tx: mpsc::UnboundedSender<DownloadEvent>,
    mut control_rx: watch::Receiver<ControlState>,
) {
    let mut last = *control_rx.borrow();
    loop {
        if control_rx.changed().await.is_err() {
            break;
        }

        let current = *control_rx.borrow_and_update();
        if current != last {
            match current {
                ControlState::Paused => {
                    let _ = events_tx.send(DownloadEvent::Paused { id });
                }
                ControlState::Running => {
                    let _ = events_tx.send(DownloadEvent::Resumed { id });
                }
                ControlState::Cancelled => {
                    let _ = events_tx.send(DownloadEvent::Cancelled { id });
                }
            }
            last = current;
        }

        if current == ControlState::Cancelled {
            break;
        }
    }
}

async fn split_largest_remaining_segment(shared: &SharedState, min_segment_size: u64) -> bool {
    let mut meta = shared.metadata.lock().await;

    let mut best: Option<(usize, u64, u64, u64)> = None;
    for seg in &meta.segments {
        if seg.state == SegmentState::Done {
            continue;
        }

        let cursor = seg.start.saturating_add(seg.downloaded);
        if cursor > seg.end {
            continue;
        }

        let remaining = seg.end.saturating_sub(cursor) + 1;
        if remaining < min_segment_size.saturating_mul(2) {
            continue;
        }

        match best {
            Some((_, best_remaining, _, _)) if best_remaining >= remaining => {}
            _ => {
                best = Some((seg.id, remaining, cursor, seg.end));
            }
        }
    }

    let Some((segment_id, remaining, cursor, old_end)) = best else {
        return false;
    };

    let split_at = cursor.saturating_add(remaining / 2);
    if split_at <= cursor || split_at > old_end {
        return false;
    }

    let seg = &mut meta.segments[segment_id];
    seg.end = split_at.saturating_sub(1);

    let new_id = meta.segments.len();
    meta.segments.push(SegmentMetadata {
        id: new_id,
        start: split_at,
        end: old_end,
        downloaded: 0,
        state: SegmentState::Pending,
    });

    drop(meta);
    enqueue_segment(shared, new_id).await;
    true
}

async fn wait_if_running(control_rx: &mut watch::Receiver<ControlState>) -> Result<()> {
    loop {
        let state = *control_rx.borrow();
        match state {
            ControlState::Running => return Ok(()),
            ControlState::Paused => {
                if control_rx.changed().await.is_err() {
                    return Err(LokiDmError::Cancelled);
                }
            }
            ControlState::Cancelled => return Err(LokiDmError::Cancelled),
        }
    }
}

async fn claim_segment(shared: &SharedState) -> Option<usize> {
    loop {
        let next = {
            let mut q = shared.queue.lock().await;
            q.pop_front()
        };
        let id = next?;

        let mut meta = shared.metadata.lock().await;
        if let Some(seg) = meta.segments.get_mut(id)
            && seg.state == SegmentState::Pending
            && !seg.is_complete()
        {
            seg.state = SegmentState::InProgress;
            return Some(id);
        }
    }
}

async fn enqueue_segment(shared: &SharedState, segment_id: usize) {
    {
        let mut q = shared.queue.lock().await;
        q.push_back(segment_id);
    }
    shared.queue_notify.notify_waiters();
}

async fn set_segment_pending(shared: &SharedState, segment_id: usize) {
    let mut meta = shared.metadata.lock().await;
    if let Some(seg) = meta.segments.get_mut(segment_id)
        && !seg.is_complete()
    {
        seg.state = SegmentState::Pending;
    }
}

async fn mark_segment_done(shared: &SharedState, segment_id: usize) {
    let mut meta = shared.metadata.lock().await;
    if let Some(seg) = meta.segments.get_mut(segment_id) {
        seg.downloaded = seg.downloaded.min(seg.len());
        seg.state = SegmentState::Done;
    }
}

async fn next_write_window(
    shared: &SharedState,
    segment_id: usize,
    max_len: usize,
) -> Option<(u64, usize)> {
    let meta = shared.metadata.lock().await;
    let seg = meta.segments.get(segment_id)?;
    if seg.is_complete() {
        return None;
    }

    let offset = seg.start.saturating_add(seg.downloaded);
    if offset > seg.end {
        return None;
    }

    let allowed = (seg.end.saturating_sub(offset) + 1).min(max_len as u64) as usize;
    Some((offset, allowed))
}

async fn apply_written(shared: &SharedState, segment_id: usize, written: u64) {
    let mut delta = 0_u64;

    {
        let mut meta = shared.metadata.lock().await;
        if let Some(seg) = meta.segments.get_mut(segment_id) {
            let before = seg.downloaded;
            let after = before.saturating_add(written).min(seg.len());
            seg.downloaded = after;
            if seg.downloaded >= seg.len() {
                seg.state = SegmentState::Done;
            }
            delta = after.saturating_sub(before);
        }

        meta.bytes_downloaded = meta.segments.iter().map(|s| s.downloaded).sum();
    }

    if delta > 0 {
        shared.total_downloaded.fetch_add(delta, Ordering::Relaxed);
    }
}

async fn is_segment_complete(shared: &SharedState, segment_id: usize) -> bool {
    let meta = shared.metadata.lock().await;
    meta.segments
        .get(segment_id)
        .is_some_and(SegmentMetadata::is_complete)
}

async fn all_segments_done(shared: &SharedState) -> bool {
    let meta = shared.metadata.lock().await;
    meta.is_complete()
}

async fn enforce_quota(shared: &SharedState, incoming: u64) {
    let Some(limit) = shared.hourly_quota_bytes else {
        return;
    };

    loop {
        let sleep_for = {
            let mut quota = shared.quota.lock().await;
            let elapsed = quota.started_at.elapsed();
            if elapsed >= Duration::from_secs(3600) {
                quota.started_at = Instant::now();
                quota.bytes_used = 0;
            }

            if quota.bytes_used.saturating_add(incoming) <= limit {
                quota.bytes_used = quota.bytes_used.saturating_add(incoming);
                None
            } else {
                Some(Duration::from_secs(3600).saturating_sub(elapsed))
            }
        };

        if let Some(delay) = sleep_for {
            tokio::time::sleep(delay).await;
        } else {
            return;
        }
    }
}

async fn probe_remote(client: &Client, config: &DownloadConfig) -> Result<RemoteProbe> {
    let mut head_req = client.head(&config.url);
    head_req = apply_auth(head_req, &config.auth)?;
    for (key, value) in &config.headers {
        head_req = head_req.header(key, value);
    }

    if let Ok(resp) = head_req.send().await
        && resp.status().is_success()
    {
        let total_size = resp
            .headers()
            .get(CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u64>().ok());

        let range_supported = resp
            .headers()
            .get(ACCEPT_RANGES)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.eq_ignore_ascii_case("bytes") || v.contains("bytes"));

        if let Some(total_size) = total_size {
            return Ok(RemoteProbe {
                total_size,
                range_supported,
                etag: resp
                    .headers()
                    .get(ETAG)
                    .and_then(|v| v.to_str().ok())
                    .map(ToOwned::to_owned),
                last_modified: resp
                    .headers()
                    .get(LAST_MODIFIED)
                    .and_then(|v| v.to_str().ok())
                    .map(ToOwned::to_owned),
            });
        }
    }

    let mut get_req = client.get(&config.url).header(RANGE, "bytes=0-0");
    get_req = apply_auth(get_req, &config.auth)?;
    for (key, value) in &config.headers {
        get_req = get_req.header(key, value);
    }

    let response = get_req.send().await?;
    if !response.status().is_success() {
        return Err(LokiDmError::HttpStatus(response.status()));
    }

    let headers = response.headers();
    let total_from_range = headers
        .get(CONTENT_RANGE)
        .and_then(|v| v.to_str().ok())
        .and_then(parse_total_from_content_range);

    let total_from_length = headers
        .get(CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok());

    let total_size = total_from_range.or(total_from_length);

    let Some(total_size) = total_size else {
        return Err(LokiDmError::MissingContentLength);
    };

    Ok(RemoteProbe {
        total_size,
        range_supported: response.status() == StatusCode::PARTIAL_CONTENT,
        etag: headers
            .get(ETAG)
            .and_then(|v| v.to_str().ok())
            .map(ToOwned::to_owned),
        last_modified: headers
            .get(LAST_MODIFIED)
            .and_then(|v| v.to_str().ok())
            .map(ToOwned::to_owned),
    })
}

fn parse_total_from_content_range(value: &str) -> Option<u64> {
    let (_, total) = value.rsplit_once('/')?;
    total.parse::<u64>().ok()
}

fn backoff_with_jitter(attempt: u8) -> u64 {
    let base = 125_u64.saturating_mul(2_u64.saturating_pow(u32::from(attempt.min(8))));
    let jitter = rand::rng().random_range(0..=100_u64);
    base.saturating_add(jitter)
}

fn source_urls(config: &DownloadConfig) -> Vec<String> {
    let mut urls = Vec::with_capacity(config.mirror_urls.len().saturating_add(1));
    urls.push(config.url.clone());
    for url in &config.mirror_urls {
        if !urls.iter().any(|candidate| candidate == url) {
            urls.push(url.clone());
        }
    }
    urls
}

async fn select_best_source(
    client: &Client,
    config: &DownloadConfig,
) -> Result<(String, RemoteProbe, usize)> {
    let sources = source_urls(config);
    let source_count = sources.len();
    let mut best: Option<(String, RemoteProbe, Duration)> = None;
    let mut expected_size: Option<u64> = None;

    for url in &sources {
        let mut probe_cfg = config.clone();
        probe_cfg.url = url.clone();

        let started = Instant::now();
        let probe = match probe_remote(client, &probe_cfg).await {
            Ok(probe) => probe,
            Err(_) => continue,
        };
        let latency = started.elapsed();

        if let Some(expected) = expected_size {
            if expected != probe.total_size {
                continue;
            }
        } else {
            expected_size = Some(probe.total_size);
        }

        match &best {
            None => {
                best = Some((url.clone(), probe, latency));
            }
            Some((_, current_probe, current_latency)) => {
                let probe_better = probe.range_supported && !current_probe.range_supported;
                let latency_better = probe.range_supported == current_probe.range_supported
                    && latency < *current_latency;
                if probe_better || latency_better {
                    best = Some((url.clone(), probe, latency));
                }
            }
        }
    }

    if let Some((url, probe, _)) = best {
        return Ok((url, probe, source_count));
    }

    let probe = probe_remote(client, config).await?;
    Ok((config.url.clone(), probe, source_count))
}

fn apply_auth(
    request: reqwest::RequestBuilder,
    auth: &Option<AuthConfig>,
) -> Result<reqwest::RequestBuilder> {
    let Some(auth) = auth else {
        return Ok(request);
    };

    match auth {
        AuthConfig::Basic { username, password } => {
            Ok(request.basic_auth(username, Some(password)))
        }
        AuthConfig::Bearer { token } => Ok(request.bearer_auth(token)),
        AuthConfig::Ntlm => Err(LokiDmError::Message(
            "NTLM authentication is not implemented in this build".to_owned(),
        )),
        AuthConfig::Kerberos => Err(LokiDmError::Message(
            "Kerberos authentication is not implemented in this build".to_owned(),
        )),
    }
}

fn build_proxy(proxy_cfg: &ProxyConfig) -> Result<Proxy> {
    let scheme = match proxy_cfg.kind {
        ProxyKind::Http => "http",
        ProxyKind::Https => "https",
        ProxyKind::Socks5 => "socks5h",
    };

    let uri = format!("{scheme}://{}:{}", proxy_cfg.host, proxy_cfg.port);
    let mut proxy = Proxy::all(uri)?;

    if let (Some(user), Some(pass)) = (&proxy_cfg.username, &proxy_cfg.password) {
        proxy = proxy.basic_auth(user, pass);
    }

    Ok(proxy)
}

fn write_all_at(file: &std::fs::File, mut buf: &[u8], mut offset: u64) -> std::io::Result<()> {
    while !buf.is_empty() {
        #[cfg(unix)]
        let written = std::os::unix::fs::FileExt::write_at(file, buf, offset)?;
        #[cfg(windows)]
        let written = std::os::windows::fs::FileExt::seek_write(file, buf, offset)?;

        if written == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::WriteZero,
                "failed to write download chunk",
            ));
        }

        offset = offset.saturating_add(written as u64);
        buf = &buf[written..];
    }

    Ok(())
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::parse_total_from_content_range;

    #[test]
    fn parses_content_range_total() {
        assert_eq!(parse_total_from_content_range("bytes 0-0/42"), Some(42));
        assert_eq!(parse_total_from_content_range("bytes */42"), Some(42));
        assert_eq!(parse_total_from_content_range("invalid"), None);
    }
}
