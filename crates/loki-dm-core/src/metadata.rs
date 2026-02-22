use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::error::Result;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum SegmentState {
    Pending,
    InProgress,
    Done,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SegmentMetadata {
    pub id: usize,
    pub start: u64,
    pub end: u64,
    pub downloaded: u64,
    pub state: SegmentState,
}

impl SegmentMetadata {
    #[must_use]
    pub fn len(&self) -> u64 {
        self.end.saturating_sub(self.start) + 1
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.end < self.start
    }

    #[must_use]
    pub fn remaining(&self) -> u64 {
        self.len().saturating_sub(self.downloaded)
    }

    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.downloaded >= self.len()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadMetadata {
    pub version: u16,
    pub url: String,
    pub total_size: u64,
    pub bytes_downloaded: u64,
    pub output_path: PathBuf,
    pub part_path: PathBuf,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub created_at_unix_ms: u128,
    pub updated_at_unix_ms: u128,
    pub segments: Vec<SegmentMetadata>,
}

impl DownloadMetadata {
    #[must_use]
    pub fn for_new(
        url: String,
        output_path: PathBuf,
        part_path: PathBuf,
        total_size: u64,
        etag: Option<String>,
        last_modified: Option<String>,
        segments: Vec<SegmentMetadata>,
    ) -> Self {
        let now = now_unix_ms();
        Self {
            version: 1,
            url,
            total_size,
            bytes_downloaded: 0,
            output_path,
            part_path,
            etag,
            last_modified,
            created_at_unix_ms: now,
            updated_at_unix_ms: now,
            segments,
        }
    }

    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.segments.iter().all(SegmentMetadata::is_complete)
    }

    pub fn normalize_for_resume(&mut self) {
        for segment in &mut self.segments {
            if segment.state == SegmentState::InProgress {
                segment.state = SegmentState::Pending;
            }
            segment.downloaded = segment.downloaded.min(segment.len());
        }
        self.bytes_downloaded = self.segments.iter().map(|s| s.downloaded).sum();
        self.updated_at_unix_ms = now_unix_ms();
    }
}

#[must_use]
pub fn sidecar_paths(output_path: &Path) -> (PathBuf, PathBuf) {
    let file_name = output_path
        .file_name()
        .and_then(std::ffi::OsStr::to_str)
        .map_or_else(|| "download".to_owned(), ToOwned::to_owned);

    let part_name = format!("{file_name}.part");
    let meta_name = format!("{file_name}.lokidm");

    let parent = output_path.parent().unwrap_or_else(|| Path::new("."));
    (parent.join(part_name), parent.join(meta_name))
}

pub fn save_metadata(meta_path: &Path, metadata: &DownloadMetadata) -> Result<()> {
    let mut snapshot = metadata.clone();
    snapshot.updated_at_unix_ms = now_unix_ms();
    let encoded = serde_json::to_vec_pretty(&snapshot)?;
    fs::write(meta_path, encoded)?;
    Ok(())
}

pub fn load_metadata(meta_path: &Path) -> Result<DownloadMetadata> {
    let raw = fs::read(meta_path)?;
    let mut parsed: DownloadMetadata = serde_json::from_slice(&raw)?;
    parsed.normalize_for_resume();
    Ok(parsed)
}

#[must_use]
pub fn build_initial_segments(total_size: u64, count: u16) -> Vec<SegmentMetadata> {
    if total_size == 0 {
        return Vec::new();
    }

    let count_u64 = u64::from(count.max(1));
    let chunk = (total_size / count_u64).max(1);

    let mut segments = Vec::with_capacity(count_u64 as usize);
    let mut start = 0_u64;
    let mut id = 0_usize;

    while start < total_size {
        let mut end = start.saturating_add(chunk).saturating_sub(1);
        if id as u64 == count_u64.saturating_sub(1) || end >= total_size {
            end = total_size - 1;
        }

        segments.push(SegmentMetadata {
            id,
            start,
            end,
            downloaded: 0,
            state: SegmentState::Pending,
        });

        id += 1;
        start = end.saturating_add(1);
    }

    segments
}

#[must_use]
pub fn now_unix_ms() -> u128 {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(v) => v.as_millis(),
        Err(_) => 0,
    }
}
