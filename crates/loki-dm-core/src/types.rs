use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DownloadStatus {
    Queued,
    Running,
    Paused,
    Retrying,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone)]
pub struct DownloadProgress {
    pub id: u64,
    pub downloaded_bytes: u64,
    pub total_bytes: u64,
    pub speed_bps: f64,
    pub eta_seconds: Option<u64>,
    pub active_connections: u16,
    pub target_connections: u16,
}

#[derive(Debug, Clone)]
pub enum DownloadEvent {
    Started {
        id: u64,
        url: String,
        output_path: PathBuf,
        total_bytes: u64,
        resumed: bool,
    },
    Progress(DownloadProgress),
    Retrying {
        id: u64,
        segment_id: usize,
        attempt: u8,
        wait_ms: u64,
        reason: String,
    },
    ConnectionsAdjusted {
        id: u64,
        from: u16,
        to: u16,
    },
    MirrorSelected {
        id: u64,
        url: String,
        source_count: usize,
    },
    Paused {
        id: u64,
    },
    Resumed {
        id: u64,
    },
    Completed {
        id: u64,
        output_path: PathBuf,
        duration_ms: u128,
    },
    HookExecuted {
        id: u64,
        command: String,
        success: bool,
        code: Option<i32>,
        stderr: String,
    },
    Failed {
        id: u64,
        error: String,
    },
    Cancelled {
        id: u64,
    },
}
