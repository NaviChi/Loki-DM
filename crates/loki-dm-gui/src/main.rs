#![allow(
    clippy::assigning_clones,
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    clippy::map_unwrap_or,
    clippy::missing_errors_doc,
    clippy::must_use_candidate,
    clippy::needless_pass_by_value,
    clippy::too_many_lines,
    clippy::uninlined_format_args
)]
#![cfg_attr(test, allow(clippy::expect_used, clippy::unwrap_used))]

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::Context;
use loki_dm_core::native_messaging::{
    NativeHostManifestSpec, install_manifests, uninstall_manifests, validate_installation,
    write_manifest_pair,
};
use loki_dm_core::queue::{
    QueueItemStatus, QueuePriority, QueueState, classify_url, merge_external_queue_state,
};
use loki_dm_core::settings::ThemeMode;
use loki_dm_core::spider::{SpiderConfig, collect_urls, crawl};
use loki_dm_core::{
    AppSettings, CHROME_EXTENSION_ID_PLACEHOLDER, DownloadConfig, DownloadEngine, DownloadEvent,
    DownloadHandle, DownloadStatus, EngineSettings, NativeHostDiagnostics, NativeHostInstallReport,
    NativeHostValidationReport, ScheduleSpec, ScheduledDownload, collect_native_host_diagnostics,
};
use serde::{Deserialize, Serialize};
use tauri::State;
use tokio::sync::Mutex;
use tokio::sync::mpsc::error::TryRecvError;
use tracing::warn;
use url::Url;

#[derive(Clone)]
struct AppState {
    inner: Arc<Mutex<CoreState>>,
}

impl AppState {
    fn new(core: CoreState) -> Self {
        Self {
            inner: Arc::new(Mutex::new(core)),
        }
    }
}

struct CoreState {
    engine: DownloadEngine,
    settings: AppSettings,
    settings_path: PathBuf,
    queue: QueueState,
    queue_path: PathBuf,
    downloads: Vec<DownloadRow>,
    handles: BTreeMap<u64, DownloadHandle>,
    queue_processing_enabled: bool,
    status_message: String,
    added_counter: u64,
    scheduler_jobs: Vec<ScheduledDownload>,
    next_scheduler_id: u64,
    queue_last_mtime: Option<SystemTime>,
    queue_sync_error: Option<String>,
}

#[derive(Clone)]
struct DownloadRow {
    id: u64,
    queue_item_id: Option<u64>,
    url: String,
    category: String,
    output_path: PathBuf,
    status: DownloadStatus,
    progress: f32,
    downloaded_bytes: u64,
    total_bytes: u64,
    speed_bps: f64,
    eta_seconds: Option<u64>,
    active_connections: u16,
    target_connections: u16,
    message: String,
    added_order: u64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct HeaderPair {
    key: String,
    value: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AddDownloadRequest {
    url: String,
    save_path: Option<String>,
    connections: Option<u16>,
    category: Option<String>,
    mirrors: Option<Vec<String>>,
    headers: Option<Vec<HeaderPair>>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct QueuePriorityRequest {
    priority: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SchedulerAddRequest {
    name: Option<String>,
    request: AddDownloadRequest,
    start_in_secs: Option<u64>,
    interval_secs: Option<u64>,
    enabled: Option<bool>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SpiderScanRequest {
    root_url: String,
    depth: Option<usize>,
    extensions: Option<Vec<String>>,
    same_host_only: Option<bool>,
    respect_robots: Option<bool>,
    queue_results: Option<bool>,
    start_downloads: Option<bool>,
    output_dir: Option<String>,
    connections: Option<u16>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct NativeHostSpecRequest {
    host_name: Option<String>,
    binary_path: Option<String>,
    chrome_extension_id: Option<String>,
    firefox_extension_id: Option<String>,
    manifest_output_dir: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BrowserSettingsUpdateRequest {
    integration_enabled: Option<bool>,
    intercept_all_downloads: Option<bool>,
    native_host_name: Option<String>,
    native_host_binary_path: Option<String>,
    chrome_extension_id: Option<String>,
    firefox_extension_id: Option<String>,
    manifest_output_dir: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct DashboardState {
    theme_mode: String,
    toolbar_skin: String,
    queue_running: bool,
    queue_size: usize,
    active_downloads: usize,
    global_speed_bps: f64,
    status_message: String,
    categories: Vec<CategoryCountDto>,
    downloads: Vec<DownloadDto>,
    queue_items: Vec<QueueItemDto>,
    scheduler_jobs: Vec<SchedulerJobDto>,
    browser_settings: BrowserSettingsDto,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct CategoryCountDto {
    name: String,
    count: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct DownloadDto {
    id: u64,
    queue_item_id: Option<u64>,
    url: String,
    file_name: String,
    output_path: String,
    category: String,
    status: String,
    progress: f32,
    downloaded_bytes: u64,
    total_bytes: u64,
    speed_bps: f64,
    eta_seconds: Option<u64>,
    active_connections: u16,
    target_connections: u16,
    message: String,
    added_order: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct QueueItemDto {
    id: u64,
    url: String,
    file_name: String,
    output_path: String,
    category: String,
    priority: String,
    status: String,
    last_error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct SchedulerJobDto {
    id: u64,
    name: String,
    url: String,
    output_path: String,
    kind: String,
    interval_secs: Option<u64>,
    next_run_epoch_ms: u64,
    enabled: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct BrowserSettingsDto {
    integration_enabled: bool,
    intercept_all_downloads: bool,
    native_host_name: String,
    native_host_binary_path: Option<String>,
    chrome_extension_id: String,
    firefox_extension_id: String,
    manifest_output_dir: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct SpiderHitDto {
    url: String,
    depth: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct SpiderScanResult {
    root_url: String,
    hits: Vec<SpiderHitDto>,
    unique_url_count: usize,
    queued_count: usize,
    started_count: usize,
    duplicate_count: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct NativeHostActionResult {
    message: String,
    detail_lines: Vec<String>,
    manifest_paths: Vec<String>,
    install_report: Option<NativeHostInstallReport>,
    validation_report: Option<NativeHostValidationReport>,
    diagnostics: Option<NativeHostDiagnostics>,
}

impl CoreState {
    fn new() -> anyhow::Result<Self> {
        let settings_path = loki_dm_core::settings::default_settings_path();
        let queue_path = loki_dm_core::queue::default_queue_path();

        let settings_file_exists = settings_path.exists();
        let mut settings = AppSettings::load(Some(&settings_path)).unwrap_or_default();

        // Keep dark as default for first launch only; do not override user-selected Light/Auto.
        if !settings_file_exists && !matches!(settings.appearance.theme, ThemeMode::Dark) {
            settings.appearance.theme = ThemeMode::Dark;
            if let Err(err) = settings.save(Some(&settings_path)) {
                warn!("failed to persist first-run dark default theme: {err}");
            }
        }

        let queue = QueueState::load(Some(&queue_path)).unwrap_or_default();
        let queue_last_mtime = fs::metadata(&queue_path)
            .and_then(|meta| meta.modified())
            .ok();

        let engine = DownloadEngine::new(EngineSettings {
            global_speed_limit_bps: settings.connection.global_speed_limit_bps,
            ..EngineSettings::default()
        })
        .context("failed to initialize download engine")?;

        Ok(Self {
            engine,
            settings,
            settings_path,
            queue,
            queue_path,
            downloads: Vec::new(),
            handles: BTreeMap::new(),
            queue_processing_enabled: false,
            status_message: "Ready".to_owned(),
            added_counter: 0,
            scheduler_jobs: Vec::new(),
            next_scheduler_id: 1,
            queue_last_mtime,
            queue_sync_error: None,
        })
    }

    fn persist_queue_quiet(&mut self) {
        if let Err(err) = self.queue.save(Some(&self.queue_path)) {
            self.status_message = format!("failed to save queue: {err}");
            return;
        }

        self.queue_last_mtime = fs::metadata(&self.queue_path)
            .and_then(|meta| meta.modified())
            .ok();
        self.queue_sync_error = None;
    }

    fn poll_external_queue_updates(&mut self) {
        let observed_mtime = match fs::metadata(&self.queue_path) {
            Ok(meta) => meta.modified().ok(),
            Err(err) if err.kind() == ErrorKind::NotFound => None,
            Err(err) => {
                let msg = format!("failed to stat queue file: {err}");
                if self.queue_sync_error.as_deref() != Some(msg.as_str()) {
                    self.status_message = msg.clone();
                    self.queue_sync_error = Some(msg);
                }
                return;
            }
        };

        if observed_mtime.is_none() || observed_mtime == self.queue_last_mtime {
            return;
        }

        match QueueState::load(Some(&self.queue_path)) {
            Ok(on_disk) => {
                let report = merge_external_queue_state(&mut self.queue, on_disk);
                if report.new_items_added > 0 || report.existing_items_merged > 0 {
                    self.status_message = format!(
                        "queue synced: +{} new, {} merged",
                        report.new_items_added, report.existing_items_merged
                    );
                }
                self.queue_sync_error = None;
            }
            Err(err) => {
                let msg = format!("queue sync skipped: {err}");
                if self.queue_sync_error.as_deref() != Some(msg.as_str()) {
                    self.status_message = msg.clone();
                    self.queue_sync_error = Some(msg);
                }
            }
        }

        self.queue_last_mtime = observed_mtime;
    }

    fn persist_settings_quiet(&mut self) {
        if let Err(err) = self.settings.save(Some(&self.settings_path)) {
            self.status_message = format!("failed to save settings: {err}");
        }
    }

    fn tick(&mut self) {
        self.poll_external_queue_updates();
        self.poll_download_events();
        self.run_due_scheduler_jobs();
        if self.queue_processing_enabled {
            self.top_up_queue_downloads();
        }
    }

    fn poll_download_events(&mut self) {
        let ids = self.handles.keys().copied().collect::<Vec<_>>();
        let mut remove_ids = BTreeSet::new();

        for id in ids {
            loop {
                let polled = {
                    match self.handles.get_mut(&id) {
                        Some(handle) => match handle.try_recv_event() {
                            Ok(event) => Some(Ok(event)),
                            Err(TryRecvError::Empty) => Some(Err(TryRecvError::Empty)),
                            Err(TryRecvError::Disconnected) => {
                                Some(Err(TryRecvError::Disconnected))
                            }
                        },
                        None => None,
                    }
                };

                let Some(polled) = polled else {
                    break;
                };

                match polled {
                    Ok(event) => {
                        let is_terminal = matches!(
                            event,
                            DownloadEvent::Completed { .. }
                                | DownloadEvent::Failed { .. }
                                | DownloadEvent::Cancelled { .. }
                        );
                        self.apply_event(event);
                        if is_terminal {
                            remove_ids.insert(id);
                        }
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        remove_ids.insert(id);
                        break;
                    }
                }
            }
        }

        for id in remove_ids {
            self.handles.remove(&id);
        }
    }

    fn apply_event(&mut self, event: DownloadEvent) {
        match event {
            DownloadEvent::Started {
                id,
                output_path,
                total_bytes,
                resumed,
                ..
            } => {
                if let Some(row) = self.downloads.iter_mut().find(|row| row.id == id) {
                    row.output_path = output_path;
                    row.total_bytes = total_bytes;
                    row.status = DownloadStatus::Running;
                    row.message = if resumed {
                        "resumed".to_owned()
                    } else {
                        "running".to_owned()
                    };
                }
            }
            DownloadEvent::MirrorSelected {
                id,
                url,
                source_count,
            } => {
                if let Some(row) = self.downloads.iter_mut().find(|row| row.id == id) {
                    row.message = format!("mirror selected ({source_count}): {url}");
                }
            }
            DownloadEvent::Progress(progress) => {
                if let Some(row) = self.downloads.iter_mut().find(|row| row.id == progress.id) {
                    row.downloaded_bytes = progress.downloaded_bytes;
                    row.total_bytes = progress.total_bytes;
                    row.speed_bps = progress.speed_bps;
                    row.eta_seconds = progress.eta_seconds;
                    row.active_connections = progress.active_connections;
                    row.target_connections = progress.target_connections;
                    row.progress = if row.total_bytes == 0 {
                        0.0
                    } else {
                        (row.downloaded_bytes as f32 / row.total_bytes as f32).clamp(0.0, 1.0)
                    };
                    if matches!(row.status, DownloadStatus::Queued | DownloadStatus::Paused) {
                        row.status = DownloadStatus::Running;
                    }
                }
            }
            DownloadEvent::Retrying {
                id,
                segment_id,
                attempt,
                wait_ms,
                reason,
            } => {
                if let Some(row) = self.downloads.iter_mut().find(|row| row.id == id) {
                    row.status = DownloadStatus::Retrying;
                    row.message = format!(
                        "retry segment={segment_id} attempt={attempt} in {wait_ms}ms ({reason})"
                    );
                }
            }
            DownloadEvent::ConnectionsAdjusted { id, from, to } => {
                if let Some(row) = self.downloads.iter_mut().find(|row| row.id == id) {
                    row.target_connections = to;
                    row.message = format!("connections adjusted {from} -> {to}");
                }
            }
            DownloadEvent::Paused { id } => {
                if let Some(row) = self.downloads.iter_mut().find(|row| row.id == id) {
                    row.status = DownloadStatus::Paused;
                    row.message = "paused".to_owned();
                }
            }
            DownloadEvent::Resumed { id } => {
                if let Some(row) = self.downloads.iter_mut().find(|row| row.id == id) {
                    row.status = DownloadStatus::Running;
                    row.message = "running".to_owned();
                }
            }
            DownloadEvent::Completed {
                id,
                output_path,
                duration_ms,
            } => {
                if let Some(row) = self.downloads.iter_mut().find(|row| row.id == id) {
                    row.output_path = output_path;
                    row.progress = 1.0;
                    row.status = DownloadStatus::Completed;
                    row.message = format!("done in {duration_ms}ms");
                    if let Some(queue_id) = row.queue_item_id {
                        self.queue
                            .set_status(queue_id, QueueItemStatus::Completed, None);
                        self.persist_queue_quiet();
                    }
                }
            }
            DownloadEvent::HookExecuted {
                id,
                success,
                code,
                stderr,
                ..
            } => {
                if let Some(row) = self.downloads.iter_mut().find(|row| row.id == id) {
                    row.message =
                        format!("AV hook success={success} code={code:?} stderr={stderr}");
                }
            }
            DownloadEvent::Failed { id, error } => {
                if let Some(row) = self.downloads.iter_mut().find(|row| row.id == id) {
                    row.status = DownloadStatus::Failed;
                    row.message = error.clone();
                    if let Some(queue_id) = row.queue_item_id {
                        self.queue
                            .set_status(queue_id, QueueItemStatus::Failed, Some(error));
                        self.persist_queue_quiet();
                    }
                }
            }
            DownloadEvent::Cancelled { id } => {
                if let Some(row) = self.downloads.iter_mut().find(|row| row.id == id) {
                    row.status = DownloadStatus::Cancelled;
                    row.message = "cancelled".to_owned();
                    if let Some(queue_id) = row.queue_item_id {
                        self.queue
                            .set_status(queue_id, QueueItemStatus::Cancelled, None);
                        self.persist_queue_quiet();
                    }
                }
            }
        }
    }

    fn start_download_from_request(&mut self, request: AddDownloadRequest) -> Result<u64, String> {
        let config = self.build_config(&request)?;
        self.start_download_internal(config, None)
    }

    fn queue_download_from_request(
        &mut self,
        request: AddDownloadRequest,
        priority: Option<String>,
    ) -> Result<u64, String> {
        let config = self.build_config(&request)?;
        let priority = priority
            .as_deref()
            .and_then(QueuePriority::parse)
            .unwrap_or(QueuePriority::Normal);

        let outcome = self.queue.add_download_dedup(
            config,
            priority,
            self.settings.advanced.prevent_duplicate_queue_entries,
        );
        let queued_id = match outcome {
            loki_dm_core::QueueAddOutcome::Added { id } => id,
            loki_dm_core::QueueAddOutcome::Duplicate { existing_id } => {
                self.status_message =
                    format!("duplicate queue URL detected (existing #{existing_id})");
                existing_id
            }
        };
        self.persist_queue_quiet();
        Ok(queued_id)
    }

    fn build_config(&self, request: &AddDownloadRequest) -> Result<DownloadConfig, String> {
        let url = request.url.trim();
        if url.is_empty() {
            return Err("URL is required".to_owned());
        }
        if Url::parse(url).is_err() {
            return Err("URL must be absolute and valid".to_owned());
        }

        let category = request
            .category
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| classify_url(url, &self.queue.rules).as_str().to_owned());

        let output_path = request
            .save_path
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                self.settings
                    .category_output_dir(Some(&category))
                    .join(default_filename(url))
            });

        let mut headers = BTreeMap::new();
        if let Some(header_pairs) = &request.headers {
            for pair in header_pairs {
                let key = pair.key.trim();
                let value = pair.value.trim();
                if !key.is_empty() && !value.is_empty() {
                    headers.insert(key.to_owned(), value.to_owned());
                }
            }
        }

        let mirrors = request
            .mirrors
            .clone()
            .unwrap_or_default()
            .into_iter()
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty())
            .filter(|value| value != url)
            .collect::<Vec<_>>();

        let min_connections = self.settings.connection.min_connections.max(1);
        let max_connections = self
            .settings
            .connection
            .max_connections
            .max(min_connections);
        let initial_connections = request
            .connections
            .unwrap_or(self.settings.connection.initial_connections)
            .clamp(min_connections, max_connections);

        Ok(DownloadConfig {
            url: url.to_owned(),
            mirror_urls: mirrors,
            output_path,
            initial_connections,
            min_connections,
            max_connections,
            min_segment_size: self.settings.connection.min_segment_size,
            max_retries: self.settings.advanced.retry_count,
            speed_limit_bps: self.settings.connection.default_download_speed_limit_bps,
            hour_quota_mb: self.settings.connection.default_hour_quota_mb,
            overwrite: false,
            headers,
            proxy: if self.settings.proxy.enabled {
                self.settings.proxy.proxy.clone()
            } else {
                None
            },
            auth: None,
            user_agent: None,
            category: Some(category),
            av_hook_command: self.settings.advanced.av_hook_command.clone(),
        })
    }

    fn start_download_internal(
        &mut self,
        config: DownloadConfig,
        queue_item_id: Option<u64>,
    ) -> Result<u64, String> {
        let output_path = config.output_path.clone();
        let file_name = output_path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("download")
            .to_owned();

        let category = config.category.clone().unwrap_or_else(|| {
            classify_url(&config.url, &self.queue.rules)
                .as_str()
                .to_owned()
        });

        let target_connections = config.initial_connections;
        let handle = self
            .engine
            .start(config.clone())
            .map_err(|err| format!("failed to start download: {err}"))?;

        let id = handle.id();
        let added_order = self.added_counter;
        self.added_counter = self.added_counter.saturating_add(1);

        self.downloads.push(DownloadRow {
            id,
            queue_item_id,
            url: config.url,
            category,
            output_path,
            status: DownloadStatus::Queued,
            progress: 0.0,
            downloaded_bytes: 0,
            total_bytes: 0,
            speed_bps: 0.0,
            eta_seconds: None,
            active_connections: 0,
            target_connections,
            message: "queued".to_owned(),
            added_order,
        });
        self.handles.insert(id, handle);

        if let Some(queue_id) = queue_item_id {
            self.queue
                .set_status(queue_id, QueueItemStatus::Running, None);
            self.persist_queue_quiet();
        }

        self.status_message = format!("started {file_name}");
        Ok(id)
    }

    fn run_queue(&mut self) {
        self.queue_processing_enabled = true;
        self.top_up_queue_downloads();
    }

    fn stop_queue(&mut self) {
        self.queue_processing_enabled = false;
        let queue_download_ids = self
            .downloads
            .iter()
            .filter(|row| {
                row.queue_item_id.is_some()
                    && matches!(
                        row.status,
                        DownloadStatus::Running | DownloadStatus::Retrying | DownloadStatus::Queued
                    )
            })
            .map(|row| row.id)
            .collect::<Vec<_>>();

        for id in queue_download_ids {
            if let Some(handle) = self.handles.get(&id) {
                let _ = handle.pause();
            }
        }

        self.status_message = "queue stopped".to_owned();
    }

    fn top_up_queue_downloads(&mut self) {
        let limit = self.settings.scheduler.queue_concurrent_downloads.max(1);
        let active_count = self
            .queue
            .items
            .iter()
            .filter(|item| item.status == QueueItemStatus::Running)
            .count();

        if active_count >= limit {
            return;
        }

        let slots = limit.saturating_sub(active_count);
        let pending = self.queue.pending_items();
        if pending.is_empty() {
            if active_count == 0 {
                self.queue_processing_enabled = false;
                self.status_message = "queue processing complete".to_owned();
            }
            return;
        }

        for item in pending.into_iter().take(slots) {
            let mut cfg = item.config.clone();
            if cfg.category.is_none() {
                cfg.category = Some(item.category.as_str().to_owned());
            }

            if let Err(err) = self.start_download_internal(cfg, Some(item.id)) {
                self.queue
                    .set_status(item.id, QueueItemStatus::Failed, Some(err.clone()));
                self.status_message = format!("queue item #{} failed: {err}", item.id);
            }
        }

        self.persist_queue_quiet();
    }

    fn run_due_scheduler_jobs(&mut self) {
        if self.scheduler_jobs.is_empty() {
            return;
        }

        let now = SystemTime::now();
        let mut due_jobs = Vec::new();

        for job in &mut self.scheduler_jobs {
            if !job.enabled || now < job.next_run {
                continue;
            }

            due_jobs.push((job.id, job.name.clone(), job.config.clone()));
            if let Some(next_run) = job.spec.next_after(now + Duration::from_millis(1)) {
                job.next_run = next_run;
            } else {
                job.enabled = false;
            }
        }

        for (id, name, cfg) in due_jobs {
            if let Err(err) = self.start_download_internal(cfg, None) {
                self.status_message = format!("scheduler #{id} ({name}) failed: {err}");
            } else {
                self.status_message = format!("scheduler #{id} ({name}) started");
            }
        }
    }

    fn add_scheduler_job(&mut self, request: SchedulerAddRequest) -> Result<u64, String> {
        let config = self.build_config(&request.request)?;
        let now = SystemTime::now();
        let start_at = now + Duration::from_secs(request.start_in_secs.unwrap_or(0));
        let interval_secs = request.interval_secs.unwrap_or(0);
        let spec = ScheduleSpec {
            start_at,
            interval: (interval_secs > 0).then(|| Duration::from_secs(interval_secs)),
        };

        let id = self.next_scheduler_id;
        self.next_scheduler_id = self.next_scheduler_id.saturating_add(1);

        let file_name = config
            .output_path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("scheduled-download")
            .to_owned();
        let name = request
            .name
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
            .unwrap_or(file_name);

        let next_run = spec.next_after(now).unwrap_or(start_at);
        self.scheduler_jobs.push(ScheduledDownload {
            id,
            name,
            spec,
            next_run,
            enabled: request.enabled.unwrap_or(true),
            config,
        });
        self.scheduler_jobs.sort_by_key(|job| job.next_run);
        self.status_message = format!("scheduled job #{id} added");
        Ok(id)
    }

    fn remove_scheduler_job(&mut self, id: u64) -> bool {
        let before = self.scheduler_jobs.len();
        self.scheduler_jobs.retain(|job| job.id != id);
        let removed = self.scheduler_jobs.len() != before;
        self.status_message = if removed {
            format!("scheduler #{id} removed")
        } else {
            format!("scheduler #{id} not found")
        };
        removed
    }

    fn set_scheduler_job_enabled(&mut self, id: u64, enabled: bool) -> bool {
        if let Some(job) = self.scheduler_jobs.iter_mut().find(|job| job.id == id) {
            job.enabled = enabled;
            self.status_message = if enabled {
                format!("scheduler #{id} enabled")
            } else {
                format!("scheduler #{id} disabled")
            };
            return true;
        }
        self.status_message = format!("scheduler #{id} not found");
        false
    }

    fn update_browser_settings(&mut self, request: BrowserSettingsUpdateRequest) {
        if let Some(enabled) = request.integration_enabled {
            self.settings.browser.integration_enabled = enabled;
        }
        if let Some(enabled) = request.intercept_all_downloads {
            self.settings.browser.intercept_all_downloads = enabled;
        }
        if let Some(host_name) = request
            .native_host_name
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            self.settings.browser.native_host_name = host_name.to_owned();
        }
        if let Some(binary_path) = request.native_host_binary_path {
            let trimmed = binary_path.trim();
            if trimmed.is_empty() {
                self.settings.browser.native_host_binary_path = None;
            } else {
                self.settings.browser.native_host_binary_path = Some(PathBuf::from(trimmed));
            }
        }
        if let Some(extension_id) = request.chrome_extension_id {
            self.settings.browser.chrome_extension_id = extension_id.trim().to_owned();
        }
        if let Some(extension_id) = request.firefox_extension_id {
            self.settings.browser.firefox_extension_id = extension_id.trim().to_owned();
        }
        if let Some(manifest_dir) = request.manifest_output_dir {
            let trimmed = manifest_dir.trim();
            if !trimmed.is_empty() {
                self.settings.browser.manifest_output_dir = PathBuf::from(trimmed);
            }
        }

        self.persist_settings_quiet();
        self.status_message = "browser settings updated".to_owned();
    }

    fn resolved_native_host_spec(
        &mut self,
        override_spec: Option<NativeHostSpecRequest>,
    ) -> Result<(NativeHostManifestSpec, PathBuf), String> {
        if let Some(overrides) = override_spec {
            self.update_browser_settings(BrowserSettingsUpdateRequest {
                integration_enabled: None,
                intercept_all_downloads: None,
                native_host_name: overrides.host_name,
                native_host_binary_path: overrides.binary_path,
                chrome_extension_id: overrides.chrome_extension_id,
                firefox_extension_id: overrides.firefox_extension_id,
                manifest_output_dir: overrides.manifest_output_dir,
            });
        }

        let browser = &self.settings.browser;
        let binary_path = browser
            .native_host_binary_path
            .clone()
            .or_else(|| std::env::current_exe().ok())
            .ok_or_else(|| "unable to resolve native host binary path".to_owned())?;
        let output_dir = browser.manifest_output_dir.clone();

        let spec = NativeHostManifestSpec {
            host_name: browser.native_host_name.clone(),
            binary_path,
            chrome_extension_id: browser.chrome_extension_id.clone(),
            firefox_extension_id: browser.firefox_extension_id.clone(),
        };
        Ok((spec, output_dir))
    }

    fn control_download(&mut self, id: u64, action: DownloadAction) -> Result<(), String> {
        let Some(handle) = self.handles.get(&id) else {
            return Err(format!("download #{id} not found"));
        };

        let result = match action {
            DownloadAction::Pause => handle.pause(),
            DownloadAction::Resume => handle.resume(),
            DownloadAction::Cancel => handle.cancel(),
        };

        result.map_err(|err| err.to_string())?;

        self.status_message = match action {
            DownloadAction::Pause => format!("paused #{id}"),
            DownloadAction::Resume => format!("resumed #{id}"),
            DownloadAction::Cancel => format!("cancel requested for #{id}"),
        };

        Ok(())
    }

    fn remove_download(&mut self, id: u64) {
        if let Some(handle) = self.handles.remove(&id) {
            let _ = handle.cancel();
        }

        let mut queue_item_ids = Vec::new();
        self.downloads.retain(|row| {
            if row.id == id {
                if let Some(queue_id) = row.queue_item_id {
                    queue_item_ids.push(queue_id);
                }
                false
            } else {
                true
            }
        });

        for queue_id in queue_item_ids {
            self.queue.remove(queue_id);
        }
        self.persist_queue_quiet();

        self.status_message = format!("removed #{id}");
    }

    fn delete_completed(&mut self) {
        let mut removed = 0usize;
        let mut queue_ids = Vec::new();

        self.downloads.retain(|row| {
            if row.status == DownloadStatus::Completed {
                removed = removed.saturating_add(1);
                if let Some(queue_id) = row.queue_item_id {
                    queue_ids.push(queue_id);
                }
                false
            } else {
                true
            }
        });

        for queue_id in queue_ids {
            self.queue.remove(queue_id);
        }
        self.persist_queue_quiet();

        self.status_message = format!("removed {removed} completed downloads");
    }

    fn set_theme_mode(&mut self, mode: &str) -> Result<(), String> {
        self.settings.appearance.theme = match mode.trim().to_ascii_lowercase().as_str() {
            "auto" => ThemeMode::Auto,
            "light" => ThemeMode::Light,
            "dark" => ThemeMode::Dark,
            _ => return Err("theme must be one of auto|light|dark".to_owned()),
        };
        self.persist_settings_quiet();
        self.status_message = format!("theme set to {}", mode.trim());
        Ok(())
    }

    fn open_path_in_manager(&mut self, path: &Path) -> Result<(), String> {
        let status = if cfg!(target_os = "windows") {
            Command::new("explorer").arg(path).status()
        } else if cfg!(target_os = "macos") {
            Command::new("open").arg(path).status()
        } else {
            Command::new("xdg-open").arg(path).status()
        }
        .map_err(|err| format!("failed to open {}: {err}", path.display()))?;

        if status.success() {
            self.status_message = format!("opened {}", path.display());
            Ok(())
        } else {
            Err(format!("open command exited with status {status}"))
        }
    }

    fn snapshot(&self) -> DashboardState {
        let mut downloads = self
            .downloads
            .iter()
            .cloned()
            .map(|row| DownloadDto {
                id: row.id,
                queue_item_id: row.queue_item_id,
                url: row.url,
                file_name: row
                    .output_path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("download")
                    .to_owned(),
                output_path: row.output_path.display().to_string(),
                category: row.category,
                status: status_label(row.status).to_owned(),
                progress: row.progress,
                downloaded_bytes: row.downloaded_bytes,
                total_bytes: row.total_bytes,
                speed_bps: row.speed_bps,
                eta_seconds: row.eta_seconds,
                active_connections: row.active_connections,
                target_connections: row.target_connections,
                message: row.message,
                added_order: row.added_order,
            })
            .collect::<Vec<_>>();

        downloads.sort_by_key(|row| row.added_order);

        let mut queue_items = self
            .queue
            .items
            .iter()
            .map(|item| QueueItemDto {
                id: item.id,
                url: item.config.url.clone(),
                file_name: item
                    .config
                    .output_path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("download")
                    .to_owned(),
                output_path: item.config.output_path.display().to_string(),
                category: item.category.as_str().to_owned(),
                priority: item.priority.as_str().to_owned(),
                status: format!("{:?}", item.status),
                last_error: item.last_error.clone(),
            })
            .collect::<Vec<_>>();
        queue_items.sort_by_key(|item| item.id);

        let active_downloads = self
            .downloads
            .iter()
            .filter(|row| {
                matches!(
                    row.status,
                    DownloadStatus::Queued
                        | DownloadStatus::Running
                        | DownloadStatus::Retrying
                        | DownloadStatus::Paused
                )
            })
            .count();

        let global_speed_bps = self
            .downloads
            .iter()
            .filter(|row| {
                matches!(
                    row.status,
                    DownloadStatus::Running | DownloadStatus::Retrying
                )
            })
            .map(|row| row.speed_bps)
            .sum::<f64>();

        let categories = [
            "All Downloads",
            "Compressed",
            "Documents",
            "Music",
            "Programs",
            "Video",
            "Other",
            "Unfinished",
            "Finished",
            "Grabber Projects",
            "Queues",
        ]
        .into_iter()
        .map(|name| CategoryCountDto {
            name: name.to_owned(),
            count: category_count(name, &downloads),
        })
        .collect::<Vec<_>>();

        let mut scheduler_jobs = self
            .scheduler_jobs
            .iter()
            .map(|job| {
                let interval_secs = job.spec.interval.map(|interval| interval.as_secs().max(1));
                SchedulerJobDto {
                    id: job.id,
                    name: job.name.clone(),
                    url: job.config.url.clone(),
                    output_path: job.config.output_path.display().to_string(),
                    kind: if interval_secs.is_some() {
                        "Recurring".to_owned()
                    } else {
                        "Once".to_owned()
                    },
                    interval_secs,
                    next_run_epoch_ms: system_time_to_epoch_ms(job.next_run),
                    enabled: job.enabled,
                }
            })
            .collect::<Vec<_>>();
        scheduler_jobs.sort_by_key(|job| job.next_run_epoch_ms);

        let browser_settings = BrowserSettingsDto {
            integration_enabled: self.settings.browser.integration_enabled,
            intercept_all_downloads: self.settings.browser.intercept_all_downloads,
            native_host_name: self.settings.browser.native_host_name.clone(),
            native_host_binary_path: self
                .settings
                .browser
                .native_host_binary_path
                .as_ref()
                .map(|path| path.display().to_string()),
            chrome_extension_id: self.settings.browser.chrome_extension_id.clone(),
            firefox_extension_id: self.settings.browser.firefox_extension_id.clone(),
            manifest_output_dir: self
                .settings
                .browser
                .manifest_output_dir
                .display()
                .to_string(),
        };

        DashboardState {
            theme_mode: match self.settings.appearance.theme {
                ThemeMode::Auto => "auto",
                ThemeMode::Light => "light",
                ThemeMode::Dark => "dark",
            }
            .to_owned(),
            toolbar_skin: format!("{:?}", self.settings.appearance.toolbar_skin),
            queue_running: self.queue_processing_enabled,
            queue_size: self.queue.items.len(),
            active_downloads,
            global_speed_bps,
            status_message: self.status_message.clone(),
            categories,
            downloads,
            queue_items,
            scheduler_jobs,
            browser_settings,
        }
    }
}

#[derive(Clone, Copy)]
enum DownloadAction {
    Pause,
    Resume,
    Cancel,
}

fn default_filename(url: &str) -> String {
    Url::parse(url)
        .ok()
        .and_then(|url| {
            url.path_segments()
                .and_then(|mut segments| segments.next_back())
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned)
        })
        .unwrap_or_else(|| "download.bin".to_owned())
}

fn system_time_to_epoch_ms(value: SystemTime) -> u64 {
    value
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| u64::try_from(duration.as_millis()).ok())
        .unwrap_or(0)
}

fn status_label(status: DownloadStatus) -> &'static str {
    match status {
        DownloadStatus::Queued => "Queued",
        DownloadStatus::Running => "Downloading",
        DownloadStatus::Paused => "Paused",
        DownloadStatus::Retrying => "Retrying",
        DownloadStatus::Completed => "Complete",
        DownloadStatus::Failed => "Failed",
        DownloadStatus::Cancelled => "Cancelled",
    }
}

fn category_count(name: &str, rows: &[DownloadDto]) -> usize {
    match name {
        "All Downloads" => rows.len(),
        "Unfinished" => rows
            .iter()
            .filter(|row| {
                matches!(
                    row.status.as_str(),
                    "Queued" | "Downloading" | "Paused" | "Retrying"
                )
            })
            .count(),
        "Finished" => rows.iter().filter(|row| row.status == "Complete").count(),
        "Grabber Projects" => rows
            .iter()
            .filter(|row| row.category.eq_ignore_ascii_case("Grabber Projects"))
            .count(),
        "Queues" => rows
            .iter()
            .filter(|row| row.queue_item_id.is_some())
            .count(),
        other => rows
            .iter()
            .filter(|row| row.category.eq_ignore_ascii_case(other))
            .count(),
    }
}

#[tauri::command]
async fn dashboard_state(state: State<'_, AppState>) -> Result<DashboardState, String> {
    let mut core = state.inner.lock().await;
    core.tick();
    Ok(core.snapshot())
}

#[tauri::command]
async fn start_download(
    state: State<'_, AppState>,
    request: AddDownloadRequest,
) -> Result<DashboardState, String> {
    let mut core = state.inner.lock().await;
    core.tick();
    core.start_download_from_request(request)?;
    Ok(core.snapshot())
}

#[tauri::command]
async fn queue_download(
    state: State<'_, AppState>,
    request: AddDownloadRequest,
    options: Option<QueuePriorityRequest>,
) -> Result<DashboardState, String> {
    let mut core = state.inner.lock().await;
    core.tick();
    let priority = options.and_then(|value| value.priority);
    core.queue_download_from_request(request, priority)?;
    Ok(core.snapshot())
}

#[tauri::command]
async fn run_queue(state: State<'_, AppState>) -> Result<DashboardState, String> {
    let mut core = state.inner.lock().await;
    core.tick();
    core.run_queue();
    Ok(core.snapshot())
}

#[tauri::command]
async fn stop_queue(state: State<'_, AppState>) -> Result<DashboardState, String> {
    let mut core = state.inner.lock().await;
    core.tick();
    core.stop_queue();
    Ok(core.snapshot())
}

#[tauri::command]
async fn pause_download(state: State<'_, AppState>, id: u64) -> Result<DashboardState, String> {
    let mut core = state.inner.lock().await;
    core.tick();
    core.control_download(id, DownloadAction::Pause)?;
    Ok(core.snapshot())
}

#[tauri::command]
async fn resume_download(state: State<'_, AppState>, id: u64) -> Result<DashboardState, String> {
    let mut core = state.inner.lock().await;
    core.tick();
    core.control_download(id, DownloadAction::Resume)?;
    Ok(core.snapshot())
}

#[tauri::command]
async fn cancel_download(state: State<'_, AppState>, id: u64) -> Result<DashboardState, String> {
    let mut core = state.inner.lock().await;
    core.tick();
    core.control_download(id, DownloadAction::Cancel)?;
    Ok(core.snapshot())
}

#[tauri::command]
async fn remove_download(state: State<'_, AppState>, id: u64) -> Result<DashboardState, String> {
    let mut core = state.inner.lock().await;
    core.tick();
    core.remove_download(id);
    Ok(core.snapshot())
}

#[tauri::command]
async fn delete_completed(state: State<'_, AppState>) -> Result<DashboardState, String> {
    let mut core = state.inner.lock().await;
    core.tick();
    core.delete_completed();
    Ok(core.snapshot())
}

#[tauri::command]
async fn set_theme_mode(
    state: State<'_, AppState>,
    mode: String,
) -> Result<DashboardState, String> {
    let mut core = state.inner.lock().await;
    core.set_theme_mode(&mode)?;
    Ok(core.snapshot())
}

#[tauri::command]
async fn open_output_folder(
    state: State<'_, AppState>,
    output_path: String,
) -> Result<DashboardState, String> {
    let mut core = state.inner.lock().await;
    let path = PathBuf::from(output_path);
    let target = path.parent().map(Path::to_path_buf).unwrap_or(path);
    core.open_path_in_manager(&target)?;
    Ok(core.snapshot())
}

#[tauri::command]
async fn update_browser_settings(
    state: State<'_, AppState>,
    request: BrowserSettingsUpdateRequest,
) -> Result<DashboardState, String> {
    let mut core = state.inner.lock().await;
    core.tick();
    core.update_browser_settings(request);
    Ok(core.snapshot())
}

#[tauri::command]
async fn add_scheduler_job(
    state: State<'_, AppState>,
    request: SchedulerAddRequest,
) -> Result<DashboardState, String> {
    let mut core = state.inner.lock().await;
    core.tick();
    core.add_scheduler_job(request)?;
    Ok(core.snapshot())
}

#[tauri::command]
async fn remove_scheduler_job(
    state: State<'_, AppState>,
    id: u64,
) -> Result<DashboardState, String> {
    let mut core = state.inner.lock().await;
    core.tick();
    core.remove_scheduler_job(id);
    Ok(core.snapshot())
}

#[tauri::command]
async fn set_scheduler_job_enabled(
    state: State<'_, AppState>,
    id: u64,
    enabled: bool,
) -> Result<DashboardState, String> {
    let mut core = state.inner.lock().await;
    core.tick();
    core.set_scheduler_job_enabled(id, enabled);
    Ok(core.snapshot())
}

#[tauri::command]
async fn spider_scan(
    state: State<'_, AppState>,
    request: SpiderScanRequest,
) -> Result<SpiderScanResult, String> {
    let root_url = request.root_url.trim();
    if root_url.is_empty() {
        return Err("Root URL is required".to_owned());
    }
    let root = Url::parse(root_url).map_err(|err| format!("invalid root URL: {err}"))?;
    let depth = request.depth.unwrap_or(2).min(8);
    let allowed_extensions = request
        .extensions
        .unwrap_or_default()
        .into_iter()
        .map(|value| value.trim().trim_start_matches('.').to_ascii_lowercase())
        .filter(|value| !value.is_empty())
        .collect::<BTreeSet<_>>();
    let client = reqwest::Client::builder()
        .tcp_nodelay(true)
        .build()
        .map_err(|err| format!("failed to create HTTP client: {err}"))?;
    let config = SpiderConfig {
        root: root.clone(),
        max_depth: depth,
        allowed_extensions,
        same_host_only: request.same_host_only.unwrap_or(true),
        respect_robots: request.respect_robots.unwrap_or(true),
        allowed_schemes: BTreeSet::new(),
    };
    let hits = crawl(&client, &config)
        .await
        .map_err(|err| err.to_string())?;
    let urls = collect_urls(&hits);

    let queue_results = request.queue_results.unwrap_or(true);
    let start_downloads = request.start_downloads.unwrap_or(false);
    let output_dir = request
        .output_dir
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from);

    let mut queued_count = 0_usize;
    let mut started_count = 0_usize;
    let mut duplicate_count = 0_usize;

    {
        let mut core = state.inner.lock().await;
        core.tick();
        let prevent_duplicates = core.settings.advanced.prevent_duplicate_queue_entries;
        for url in &urls {
            let mut per_item = AddDownloadRequest {
                url: url.as_str().to_owned(),
                save_path: None,
                connections: request.connections,
                category: None,
                mirrors: None,
                headers: None,
            };

            if let Some(base) = &output_dir {
                let file_name = default_filename(url.as_str());
                per_item.save_path = Some(base.join(file_name).display().to_string());
            }

            let cfg = match core.build_config(&per_item) {
                Ok(cfg) => cfg,
                Err(err) => {
                    core.status_message = format!("grabber skipped URL {}: {err}", url);
                    continue;
                }
            };

            if queue_results {
                match core.queue.add_download_dedup(
                    cfg.clone(),
                    QueuePriority::Normal,
                    prevent_duplicates,
                ) {
                    loki_dm_core::QueueAddOutcome::Added { .. } => {
                        queued_count = queued_count.saturating_add(1);
                    }
                    loki_dm_core::QueueAddOutcome::Duplicate { .. } => {
                        duplicate_count = duplicate_count.saturating_add(1);
                    }
                }
            }

            if start_downloads && core.start_download_internal(cfg, None).is_ok() {
                started_count = started_count.saturating_add(1);
            }
        }

        if queue_results {
            core.persist_queue_quiet();
        }
        core.status_message = format!(
            "grabber: {} URLs, queued {}, started {}",
            urls.len(),
            queued_count,
            started_count
        );
    }

    Ok(SpiderScanResult {
        root_url: root.to_string(),
        hits: hits
            .into_iter()
            .map(|hit| SpiderHitDto {
                url: hit.url.to_string(),
                depth: hit.depth,
            })
            .collect(),
        unique_url_count: urls.len(),
        queued_count,
        started_count,
        duplicate_count,
    })
}

#[tauri::command]
async fn generate_native_manifests(
    state: State<'_, AppState>,
    request: Option<NativeHostSpecRequest>,
) -> Result<NativeHostActionResult, String> {
    let mut core = state.inner.lock().await;
    let (spec, output_dir) = core.resolved_native_host_spec(request)?;
    let (chrome_path, firefox_path) =
        write_manifest_pair(&output_dir, &spec).map_err(|err| err.to_string())?;
    core.status_message = format!("native manifests generated in {}", output_dir.display());

    let mut detail_lines = vec![
        format!("host: {}", spec.host_name),
        format!("binary: {}", spec.binary_path.display()),
        format!("output dir: {}", output_dir.display()),
        format!("chromium manifest: {}", chrome_path.display()),
        format!("firefox manifest: {}", firefox_path.display()),
    ];
    if spec.chrome_extension_id == CHROME_EXTENSION_ID_PLACEHOLDER {
        detail_lines.push("warning: chrome extension id is still placeholder".to_owned());
    }

    Ok(NativeHostActionResult {
        message: "Native host manifests generated".to_owned(),
        detail_lines,
        manifest_paths: vec![
            chrome_path.display().to_string(),
            firefox_path.display().to_string(),
        ],
        install_report: None,
        validation_report: None,
        diagnostics: None,
    })
}

#[tauri::command]
async fn install_native_host(
    state: State<'_, AppState>,
    request: Option<NativeHostSpecRequest>,
) -> Result<NativeHostActionResult, String> {
    let mut core = state.inner.lock().await;
    let (spec, _) = core.resolved_native_host_spec(request)?;
    if !spec.binary_path.is_file() {
        return Err(format!(
            "native host binary does not exist: {}",
            spec.binary_path.display()
        ));
    }

    let report = install_manifests(&spec).map_err(|err| err.to_string())?;
    core.status_message = "native host install completed".to_owned();
    Ok(NativeHostActionResult {
        message: "Native host install completed".to_owned(),
        detail_lines: format_install_report("install", &report),
        manifest_paths: report
            .manifest_files_written
            .iter()
            .map(|path| path.display().to_string())
            .collect(),
        install_report: Some(report),
        validation_report: None,
        diagnostics: None,
    })
}

#[tauri::command]
async fn validate_native_host(
    state: State<'_, AppState>,
    request: Option<NativeHostSpecRequest>,
) -> Result<NativeHostActionResult, String> {
    let mut core = state.inner.lock().await;
    let (spec, output_dir) = core.resolved_native_host_spec(request)?;
    let validation = validate_installation(&spec).map_err(|err| err.to_string())?;
    let diagnostics =
        collect_native_host_diagnostics(&spec, Some(&output_dir)).map_err(|err| err.to_string())?;
    core.status_message = "native host validation complete".to_owned();

    let mut detail_lines = format_validation_report(&validation);
    for note in &diagnostics.notes {
        detail_lines.push(format!("note: {note}"));
    }

    Ok(NativeHostActionResult {
        message: "Native host validation complete".to_owned(),
        detail_lines,
        manifest_paths: validation
            .manifest_files_present
            .iter()
            .map(|path| path.display().to_string())
            .collect(),
        install_report: None,
        validation_report: Some(validation),
        diagnostics: Some(diagnostics),
    })
}

#[tauri::command]
async fn uninstall_native_host(
    state: State<'_, AppState>,
    request: Option<NativeHostSpecRequest>,
) -> Result<NativeHostActionResult, String> {
    let mut core = state.inner.lock().await;
    let (spec, _) = core.resolved_native_host_spec(request)?;
    let report = uninstall_manifests(&spec).map_err(|err| err.to_string())?;
    core.status_message = "native host uninstall completed".to_owned();

    Ok(NativeHostActionResult {
        message: "Native host uninstall completed".to_owned(),
        detail_lines: format_install_report("uninstall", &report),
        manifest_paths: report
            .manifest_files_removed
            .iter()
            .map(|path| path.display().to_string())
            .collect(),
        install_report: Some(report),
        validation_report: None,
        diagnostics: None,
    })
}

fn format_install_report(action: &str, report: &NativeHostInstallReport) -> Vec<String> {
    let mut lines = vec![
        format!(
            "{action}: manifests written={}",
            report.manifest_files_written.len()
        ),
        format!(
            "{action}: registry written={}",
            report.registry_entries_written.len()
        ),
        format!(
            "{action}: manifests removed={}",
            report.manifest_files_removed.len()
        ),
        format!(
            "{action}: registry removed={}",
            report.registry_entries_removed.len()
        ),
    ];

    for path in &report.manifest_files_written {
        lines.push(format!("manifest written: {}", path.display()));
    }
    for key in &report.registry_entries_written {
        lines.push(format!("registry written: {key}"));
    }
    for path in &report.manifest_files_removed {
        lines.push(format!("manifest removed: {}", path.display()));
    }
    for key in &report.registry_entries_removed {
        lines.push(format!("registry removed: {key}"));
    }
    for warning in &report.warnings {
        lines.push(format!("warning: {warning}"));
    }

    lines
}

fn format_validation_report(report: &NativeHostValidationReport) -> Vec<String> {
    let mut lines = vec![
        format!("manifest present={}", report.manifest_files_present.len()),
        format!("manifest missing={}", report.manifest_files_missing.len()),
        format!("registry present={}", report.registry_entries_present.len()),
        format!("registry missing={}", report.registry_entries_missing.len()),
    ];

    for path in &report.manifest_files_present {
        lines.push(format!("manifest present: {}", path.display()));
    }
    for path in &report.manifest_files_missing {
        lines.push(format!("manifest missing: {}", path.display()));
    }
    for key in &report.registry_entries_present {
        lines.push(format!("registry present: {key}"));
    }
    for key in &report.registry_entries_missing {
        lines.push(format!("registry missing: {key}"));
    }

    lines
}

fn main() -> anyhow::Result<()> {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("loki_dm_gui=info,loki_dm_core=info")
        .try_init();

    let core = CoreState::new()?;

    tauri::Builder::default()
        .manage(AppState::new(core))
        .invoke_handler(tauri::generate_handler![
            dashboard_state,
            start_download,
            queue_download,
            run_queue,
            stop_queue,
            pause_download,
            resume_download,
            cancel_download,
            remove_download,
            delete_completed,
            set_theme_mode,
            open_output_folder,
            update_browser_settings,
            add_scheduler_job,
            remove_scheduler_job,
            set_scheduler_job_enabled,
            spider_scan,
            generate_native_manifests,
            install_native_host,
            validate_native_host,
            uninstall_native_host,
        ])
        .run(tauri::generate_context!())
        .map_err(|err| anyhow::anyhow!(err.to_string()))
}
