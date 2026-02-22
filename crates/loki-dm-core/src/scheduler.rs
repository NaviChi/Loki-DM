use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime};

use tokio::sync::{Mutex, watch};
use tokio::task::JoinHandle;
use tracing::{error, info};

use crate::config::DownloadConfig;
use crate::engine::DownloadEngine;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScheduleKind {
    Once,
    Recurring,
}

#[derive(Debug, Clone)]
pub struct ScheduleSpec {
    pub start_at: SystemTime,
    pub interval: Option<Duration>,
}

impl ScheduleSpec {
    #[must_use]
    pub fn kind(&self) -> ScheduleKind {
        if self.interval.is_some() {
            ScheduleKind::Recurring
        } else {
            ScheduleKind::Once
        }
    }

    #[must_use]
    pub fn next_after(&self, now: SystemTime) -> Option<SystemTime> {
        match self.interval {
            Some(step) => {
                if now <= self.start_at {
                    return Some(self.start_at);
                }

                let elapsed = now.duration_since(self.start_at).ok()?;
                let step_secs = step.as_secs().max(1);
                let hops = elapsed.as_secs() / step_secs;
                let factor = u32::try_from(hops).ok()?.saturating_add(1);
                step.checked_mul(factor).map(|next| self.start_at + next)
            }
            None => (now <= self.start_at).then_some(self.start_at),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ScheduledDownload {
    pub id: u64,
    pub name: String,
    pub spec: ScheduleSpec,
    pub next_run: SystemTime,
    pub enabled: bool,
    pub config: DownloadConfig,
}

#[derive(Clone)]
pub struct DownloadScheduler {
    jobs: Arc<Mutex<Vec<ScheduledDownload>>>,
    next_id: Arc<AtomicU64>,
    stop_tx: watch::Sender<bool>,
}

impl DownloadScheduler {
    #[must_use]
    pub fn new() -> Self {
        let (stop_tx, _stop_rx) = watch::channel(false);
        Self {
            jobs: Arc::new(Mutex::new(Vec::new())),
            next_id: Arc::new(AtomicU64::new(1)),
            stop_tx,
        }
    }

    pub async fn add_job(
        &self,
        name: impl Into<String>,
        spec: ScheduleSpec,
        config: DownloadConfig,
    ) -> u64 {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let next_run = spec.next_after(SystemTime::now()).unwrap_or(spec.start_at);

        self.jobs.lock().await.push(ScheduledDownload {
            id,
            name: name.into(),
            spec,
            next_run,
            enabled: true,
            config,
        });
        id
    }

    pub async fn remove_job(&self, id: u64) -> bool {
        let mut jobs = self.jobs.lock().await;
        let before = jobs.len();
        jobs.retain(|job| job.id != id);
        jobs.len() != before
    }

    pub async fn list_jobs(&self) -> Vec<ScheduledDownload> {
        self.jobs.lock().await.clone()
    }

    pub async fn set_enabled(&self, id: u64, enabled: bool) -> bool {
        let mut jobs = self.jobs.lock().await;
        if let Some(job) = jobs.iter_mut().find(|job| job.id == id) {
            job.enabled = enabled;
            return true;
        }
        false
    }

    pub fn start(&self, engine: DownloadEngine) -> JoinHandle<()> {
        let jobs = Arc::clone(&self.jobs);
        let mut stop_rx = self.stop_tx.subscribe();

        tokio::spawn(async move {
            loop {
                if *stop_rx.borrow() {
                    break;
                }

                let now = SystemTime::now();
                let due_jobs = {
                    let mut jobs_guard = jobs.lock().await;
                    let mut due = Vec::new();

                    for job in jobs_guard.iter_mut() {
                        if !job.enabled || now < job.next_run {
                            continue;
                        }

                        due.push(job.clone());

                        if let Some(next) = job.spec.next_after(now + Duration::from_millis(1)) {
                            job.next_run = next;
                        } else {
                            job.enabled = false;
                        }
                    }

                    due
                };

                for job in due_jobs {
                    let mut handle = match engine.start(job.config.clone()) {
                        Ok(handle) => handle,
                        Err(err) => {
                            error!("failed to start scheduled job {}: {}", job.name, err);
                            continue;
                        }
                    };

                    info!("scheduled job started: id={} name={}", job.id, job.name);
                    tokio::spawn(async move {
                        if let Err(err) = handle.wait().await {
                            error!("scheduled download failed: {}", err);
                        }
                    });
                }

                tokio::select! {
                    changed = stop_rx.changed() => {
                        if changed.is_err() || *stop_rx.borrow() {
                            break;
                        }
                    }
                    _ = tokio::time::sleep(Duration::from_secs(1)) => {}
                }
            }
        })
    }

    pub fn stop(&self) {
        let _ = self.stop_tx.send(true);
    }
}
