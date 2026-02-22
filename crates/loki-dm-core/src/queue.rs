use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use url::Url;

use crate::config::DownloadConfig;
use crate::error::Result;
use crate::metadata::now_unix_ms;
use crate::settings::{app_config_dir, default_download_dir};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum DownloadCategory {
    Music,
    Video,
    Programs,
    Documents,
    Compressed,
    Other,
}

impl DownloadCategory {
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Music => "Music",
            Self::Video => "Video",
            Self::Programs => "Programs",
            Self::Documents => "Documents",
            Self::Compressed => "Compressed",
            Self::Other => "Other",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum QueueItemStatus {
    Queued,
    Scheduled,
    Running,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Default)]
#[serde(rename_all = "snake_case")]
pub enum QueuePriority {
    Low,
    #[default]
    Normal,
    High,
    Critical,
}

impl QueuePriority {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Low => "Low",
            Self::Normal => "Normal",
            Self::High => "High",
            Self::Critical => "Critical",
        }
    }

    #[must_use]
    pub fn rank(self) -> u8 {
        match self {
            Self::Low => 0,
            Self::Normal => 1,
            Self::High => 2,
            Self::Critical => 3,
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "low" => Some(Self::Low),
            "normal" | "default" => Some(Self::Normal),
            "high" => Some(Self::High),
            "critical" | "urgent" => Some(Self::Critical),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CategoryRule {
    pub name: String,
    pub category: DownloadCategory,
    pub extensions: Vec<String>,
    pub host_contains: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueueItem {
    pub id: u64,
    pub config: DownloadConfig,
    pub category: DownloadCategory,
    #[serde(default)]
    pub priority: QueuePriority,
    pub status: QueueItemStatus,
    pub added_at_unix_ms: u128,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueueState {
    pub next_id: u64,
    pub items: Vec<QueueItem>,
    pub rules: Vec<CategoryRule>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct QueueMergeReport {
    pub new_items_added: usize,
    pub existing_items_merged: usize,
    pub rules_replaced: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueueAddOutcome {
    Added { id: u64 },
    Duplicate { existing_id: u64 },
}

impl Default for QueueState {
    fn default() -> Self {
        Self {
            next_id: 1,
            items: Vec::new(),
            rules: default_category_rules(),
        }
    }
}

impl QueueState {
    pub fn load(path: Option<&Path>) -> Result<Self> {
        let file = path.map_or_else(default_queue_path, Path::to_path_buf);
        if !file.exists() {
            return Ok(Self::default());
        }

        let raw = fs::read_to_string(&file)?;
        decode_queue(&file, &raw)
    }

    pub fn save(&self, path: Option<&Path>) -> Result<PathBuf> {
        let file = path.map_or_else(default_queue_path, Path::to_path_buf);
        if let Some(parent) = file.parent() {
            fs::create_dir_all(parent)?;
        }

        let encoded = encode_queue(&file, self)?;
        fs::write(&file, encoded)?;
        Ok(file)
    }

    pub fn add_download(&mut self, config: DownloadConfig) -> u64 {
        self.add_download_with_priority(config, QueuePriority::Normal)
    }

    pub fn add_download_with_priority(
        &mut self,
        mut config: DownloadConfig,
        priority: QueuePriority,
    ) -> u64 {
        let category = config
            .category
            .as_deref()
            .map_or_else(|| classify_url(&config.url, &self.rules), parse_category);

        if config.output_path.as_os_str().is_empty() {
            config.output_path = default_download_dir().join(default_filename(&config.url));
        }

        config.category = Some(category.as_str().to_owned());

        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        self.items.push(QueueItem {
            id,
            config,
            category,
            priority,
            status: QueueItemStatus::Queued,
            added_at_unix_ms: now_unix_ms(),
            last_error: None,
        });
        id
    }

    pub fn add_download_dedup(
        &mut self,
        config: DownloadConfig,
        priority: QueuePriority,
        prevent_duplicates: bool,
    ) -> QueueAddOutcome {
        if prevent_duplicates
            && let Some(existing) = self.find_duplicate_non_terminal_url(&config.url)
        {
            return QueueAddOutcome::Duplicate {
                existing_id: existing.id,
            };
        }

        QueueAddOutcome::Added {
            id: self.add_download_with_priority(config, priority),
        }
    }

    pub fn add_url(
        &mut self,
        url: &str,
        output_dir: Option<&Path>,
        initial_connections: u16,
    ) -> u64 {
        let output_name = default_filename(url);
        let output_path = output_dir
            .map_or_else(default_download_dir, Path::to_path_buf)
            .join(output_name);

        let config = DownloadConfig {
            url: url.to_owned(),
            output_path,
            initial_connections,
            max_connections: initial_connections.max(8),
            min_connections: 1,
            ..DownloadConfig::default()
        };

        self.add_download(config)
    }

    pub fn remove(&mut self, id: u64) -> bool {
        let before = self.items.len();
        self.items.retain(|item| item.id != id);
        self.items.len() != before
    }

    pub fn pending_items(&self) -> Vec<QueueItem> {
        let mut pending = self
            .items
            .iter()
            .filter(|item| {
                matches!(
                    item.status,
                    QueueItemStatus::Queued | QueueItemStatus::Scheduled
                )
            })
            .cloned()
            .collect::<Vec<_>>();
        pending.sort_by(|a, b| {
            b.priority
                .rank()
                .cmp(&a.priority.rank())
                .then_with(|| a.added_at_unix_ms.cmp(&b.added_at_unix_ms))
                .then_with(|| a.id.cmp(&b.id))
        });
        pending
    }

    pub fn set_status(&mut self, id: u64, status: QueueItemStatus, message: Option<String>) {
        if let Some(item) = self.items.iter_mut().find(|item| item.id == id) {
            item.status = status;
            item.last_error = message;
        }
    }

    pub fn add_from_text(
        &mut self,
        text: &str,
        output_dir: Option<&Path>,
        initial_connections: u16,
    ) -> Vec<u64> {
        let mut ids = Vec::new();
        for url in urls_from_text(text) {
            ids.push(self.add_url(&url, output_dir, initial_connections));
        }
        ids
    }

    pub fn set_priority(&mut self, id: u64, priority: QueuePriority) -> bool {
        if let Some(item) = self.items.iter_mut().find(|item| item.id == id) {
            item.priority = priority;
            true
        } else {
            false
        }
    }

    #[must_use]
    pub fn find_duplicate_non_terminal_url(&self, url: &str) -> Option<&QueueItem> {
        let normalized = normalize_url_for_compare(url);
        self.items.iter().find(|item| {
            !is_terminal_status(&item.status)
                && normalize_url_for_compare(&item.config.url)
                    .eq_ignore_ascii_case(normalized.as_str())
        })
    }
}

#[must_use]
pub fn merge_external_queue_state(
    local: &mut QueueState,
    mut on_disk: QueueState,
) -> QueueMergeReport {
    let mut report = QueueMergeReport::default();
    let mut index_by_id = BTreeMap::new();
    for (idx, item) in local.items.iter().enumerate() {
        index_by_id.insert(item.id, idx);
    }

    local.next_id = local.next_id.max(on_disk.next_id);
    if !on_disk.rules.is_empty() {
        local.rules = on_disk.rules.clone();
        report.rules_replaced = true;
    }

    for disk_item in on_disk.items.drain(..) {
        if let Some(idx) = index_by_id.get(&disk_item.id).copied() {
            let keep_local_state = should_preserve_local_state(&local.items[idx].status);
            if keep_local_state {
                let local_status = local.items[idx].status.clone();
                let local_error = local.items[idx].last_error.clone();
                let mut merged = disk_item;
                merged.status = local_status;
                merged.last_error = local_error;
                local.items[idx] = merged;
            } else {
                local.items[idx] = disk_item;
            }
            report.existing_items_merged += 1;
        } else {
            index_by_id.insert(disk_item.id, local.items.len());
            local.items.push(disk_item);
            report.new_items_added += 1;
        }
    }

    report
}

#[must_use]
pub fn urls_from_text(text: &str) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .filter(|line| !line.starts_with('#'))
        .map(ToOwned::to_owned)
        .collect()
}

#[must_use]
pub fn default_queue_path() -> PathBuf {
    app_config_dir().join("queue.json")
}

#[must_use]
pub fn classify_url(url: &str, rules: &[CategoryRule]) -> DownloadCategory {
    for rule in rules {
        if let Some(host_match) = &rule.host_contains
            && let Ok(parsed) = Url::parse(url)
            && let Some(host) = parsed.host_str()
            && host.contains(host_match)
        {
            return rule.category.clone();
        }

        let ext = extension_from_url(url);
        if let Some(ext) = ext
            && rule
                .extensions
                .iter()
                .any(|candidate| candidate.eq_ignore_ascii_case(&ext))
        {
            return rule.category.clone();
        }
    }

    DownloadCategory::Other
}

#[must_use]
pub fn default_category_rules() -> Vec<CategoryRule> {
    vec![
        CategoryRule {
            name: "music-ext".to_owned(),
            category: DownloadCategory::Music,
            extensions: vec!["mp3", "flac", "wav", "aac", "ogg", "m4a"]
                .into_iter()
                .map(ToOwned::to_owned)
                .collect(),
            host_contains: None,
        },
        CategoryRule {
            name: "video-ext".to_owned(),
            category: DownloadCategory::Video,
            extensions: vec!["mp4", "mkv", "mov", "webm", "avi", "m3u8", "ts"]
                .into_iter()
                .map(ToOwned::to_owned)
                .collect(),
            host_contains: None,
        },
        CategoryRule {
            name: "program-ext".to_owned(),
            category: DownloadCategory::Programs,
            extensions: vec!["exe", "msi", "pkg", "dmg", "appimage", "deb", "rpm"]
                .into_iter()
                .map(ToOwned::to_owned)
                .collect(),
            host_contains: None,
        },
        CategoryRule {
            name: "docs-ext".to_owned(),
            category: DownloadCategory::Documents,
            extensions: vec!["pdf", "doc", "docx", "xls", "xlsx", "ppt", "txt", "md"]
                .into_iter()
                .map(ToOwned::to_owned)
                .collect(),
            host_contains: None,
        },
        CategoryRule {
            name: "archive-ext".to_owned(),
            category: DownloadCategory::Compressed,
            extensions: vec!["zip", "rar", "7z", "tar", "gz", "xz", "bz2"]
                .into_iter()
                .map(ToOwned::to_owned)
                .collect(),
            host_contains: None,
        },
    ]
}

fn parse_category(raw: &str) -> DownloadCategory {
    match raw.to_ascii_lowercase().as_str() {
        "music" => DownloadCategory::Music,
        "video" => DownloadCategory::Video,
        "programs" => DownloadCategory::Programs,
        "documents" => DownloadCategory::Documents,
        "compressed" => DownloadCategory::Compressed,
        _ => DownloadCategory::Other,
    }
}

fn should_preserve_local_state(status: &QueueItemStatus) -> bool {
    matches!(
        status,
        QueueItemStatus::Running
            | QueueItemStatus::Completed
            | QueueItemStatus::Failed
            | QueueItemStatus::Cancelled
    )
}

fn is_terminal_status(status: &QueueItemStatus) -> bool {
    matches!(
        status,
        QueueItemStatus::Completed | QueueItemStatus::Failed | QueueItemStatus::Cancelled
    )
}

fn default_filename(url: &str) -> String {
    Url::parse(url)
        .ok()
        .and_then(|u| {
            u.path_segments()
                .and_then(|mut segs| segs.next_back())
                .map(str::trim)
                .filter(|v| !v.is_empty())
                .map(ToOwned::to_owned)
        })
        .unwrap_or_else(|| "download.bin".to_owned())
}

fn extension_from_url(url: &str) -> Option<String> {
    Url::parse(url).ok().and_then(|u| {
        u.path_segments()
            .and_then(|mut segs| segs.next_back())
            .and_then(|name| {
                name.rsplit_once('.')
                    .map(|(_, ext)| ext.to_ascii_lowercase())
            })
    })
}

fn normalize_url_for_compare(raw: &str) -> String {
    let trimmed = raw.trim();
    let Ok(mut parsed) = Url::parse(trimmed) else {
        return trimmed.to_ascii_lowercase();
    };
    parsed.set_fragment(None);
    let normalized = parsed.to_string();
    normalized.trim_end_matches('/').to_ascii_lowercase()
}

fn decode_queue(path: &Path, raw: &str) -> Result<QueueState> {
    match path
        .extension()
        .and_then(std::ffi::OsStr::to_str)
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("toml") => Ok(toml::from_str(raw)?),
        _ => {
            let json_try = serde_json::from_str(raw);
            if let Ok(parsed) = json_try {
                Ok(parsed)
            } else {
                Ok(toml::from_str(raw)?)
            }
        }
    }
}

fn encode_queue(path: &Path, queue: &QueueState) -> Result<String> {
    match path
        .extension()
        .and_then(std::ffi::OsStr::to_str)
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("toml") => Ok(toml::to_string_pretty(queue)?),
        _ => Ok(serde_json::to_string_pretty(queue)?),
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn queue_item(
        id: u64,
        status: QueueItemStatus,
        url: &str,
        error: Option<&str>,
        priority: QueuePriority,
    ) -> QueueItem {
        QueueItem {
            id,
            config: DownloadConfig {
                url: url.to_owned(),
                output_path: PathBuf::from(format!("/tmp/{id}.bin")),
                ..DownloadConfig::default()
            },
            category: DownloadCategory::Other,
            priority,
            status,
            added_at_unix_ms: 1,
            last_error: error.map(ToOwned::to_owned),
        }
    }

    #[test]
    fn classifies_video_and_docs() {
        let rules = default_category_rules();
        assert_eq!(
            classify_url("https://example.com/demo.mp4", &rules),
            DownloadCategory::Video
        );
        assert_eq!(
            classify_url("https://example.com/file.pdf", &rules),
            DownloadCategory::Documents
        );
    }

    #[test]
    fn parses_urls_from_text() {
        let input = "\n# comment\nhttps://a\n\nhttps://b\n";
        let urls = urls_from_text(input);
        assert_eq!(urls, vec!["https://a", "https://b"]);
    }

    #[test]
    fn merge_adds_missing_items_and_updates_next_id() {
        let mut local = QueueState {
            next_id: 2,
            items: vec![queue_item(
                1,
                QueueItemStatus::Queued,
                "https://local/1",
                None,
                QueuePriority::Normal,
            )],
            rules: default_category_rules(),
        };
        let on_disk = QueueState {
            next_id: 10,
            items: vec![
                queue_item(
                    1,
                    QueueItemStatus::Scheduled,
                    "https://disk/1",
                    None,
                    QueuePriority::Normal,
                ),
                queue_item(
                    2,
                    QueueItemStatus::Queued,
                    "https://disk/2",
                    None,
                    QueuePriority::Normal,
                ),
            ],
            rules: Vec::new(),
        };

        let report = merge_external_queue_state(&mut local, on_disk);
        assert_eq!(report.new_items_added, 1);
        assert_eq!(report.existing_items_merged, 1);
        assert_eq!(local.next_id, 10);
        assert_eq!(local.items.len(), 2);
        assert_eq!(local.items[0].config.url, "https://disk/1");
    }

    #[test]
    fn merge_preserves_running_and_terminal_states() {
        let statuses = [
            QueueItemStatus::Running,
            QueueItemStatus::Completed,
            QueueItemStatus::Failed,
            QueueItemStatus::Cancelled,
        ];

        for status in statuses {
            let mut local = QueueState {
                next_id: 2,
                items: vec![queue_item(
                    1,
                    status.clone(),
                    "https://local/1",
                    Some("local-error"),
                    QueuePriority::Normal,
                )],
                rules: default_category_rules(),
            };
            let on_disk = QueueState {
                next_id: 2,
                items: vec![queue_item(
                    1,
                    QueueItemStatus::Queued,
                    "https://disk/1",
                    None,
                    QueuePriority::Normal,
                )],
                rules: Vec::new(),
            };

            let _ = merge_external_queue_state(&mut local, on_disk);
            assert_eq!(local.items[0].status, status);
            assert_eq!(local.items[0].last_error.as_deref(), Some("local-error"));
            assert_eq!(local.items[0].config.url, "https://disk/1");
        }
    }

    #[test]
    fn merge_overwrites_non_terminal_state_from_disk() {
        let mut local = QueueState {
            next_id: 2,
            items: vec![queue_item(
                1,
                QueueItemStatus::Queued,
                "https://local/1",
                None,
                QueuePriority::Normal,
            )],
            rules: default_category_rules(),
        };
        let on_disk = QueueState {
            next_id: 2,
            items: vec![queue_item(
                1,
                QueueItemStatus::Scheduled,
                "https://disk/1",
                Some("disk-message"),
                QueuePriority::Normal,
            )],
            rules: Vec::new(),
        };

        let _ = merge_external_queue_state(&mut local, on_disk);
        assert_eq!(local.items[0].status, QueueItemStatus::Scheduled);
        assert_eq!(local.items[0].last_error.as_deref(), Some("disk-message"));
        assert_eq!(local.items[0].config.url, "https://disk/1");
    }

    #[test]
    fn merge_replaces_rules_only_when_non_empty() {
        let mut local = QueueState {
            next_id: 1,
            items: Vec::new(),
            rules: default_category_rules(),
        };
        let keep_rules = local.rules.clone();

        let report = merge_external_queue_state(
            &mut local,
            QueueState {
                next_id: 1,
                items: Vec::new(),
                rules: Vec::new(),
            },
        );
        assert!(!report.rules_replaced);
        assert_eq!(local.rules, keep_rules);

        let replacement = vec![CategoryRule {
            name: "host-video".to_owned(),
            category: DownloadCategory::Video,
            extensions: Vec::new(),
            host_contains: Some("example.com".to_owned()),
        }];
        let report = merge_external_queue_state(
            &mut local,
            QueueState {
                next_id: 1,
                items: Vec::new(),
                rules: replacement.clone(),
            },
        );
        assert!(report.rules_replaced);
        assert_eq!(local.rules, replacement);
    }

    #[test]
    fn pending_items_are_sorted_by_priority_then_age() {
        let queue = QueueState {
            next_id: 5,
            items: vec![
                QueueItem {
                    id: 1,
                    config: DownloadConfig::default(),
                    category: DownloadCategory::Other,
                    priority: QueuePriority::Normal,
                    status: QueueItemStatus::Queued,
                    added_at_unix_ms: 10,
                    last_error: None,
                },
                QueueItem {
                    id: 2,
                    config: DownloadConfig::default(),
                    category: DownloadCategory::Other,
                    priority: QueuePriority::Critical,
                    status: QueueItemStatus::Queued,
                    added_at_unix_ms: 50,
                    last_error: None,
                },
                QueueItem {
                    id: 3,
                    config: DownloadConfig::default(),
                    category: DownloadCategory::Other,
                    priority: QueuePriority::Critical,
                    status: QueueItemStatus::Scheduled,
                    added_at_unix_ms: 5,
                    last_error: None,
                },
                QueueItem {
                    id: 4,
                    config: DownloadConfig::default(),
                    category: DownloadCategory::Other,
                    priority: QueuePriority::Low,
                    status: QueueItemStatus::Completed,
                    added_at_unix_ms: 1,
                    last_error: None,
                },
            ],
            rules: default_category_rules(),
        };

        let ordered = queue.pending_items();
        let ids = ordered.iter().map(|item| item.id).collect::<Vec<_>>();
        assert_eq!(ids, vec![3, 2, 1]);
    }

    #[test]
    fn dedup_skips_non_terminal_duplicate_urls() {
        let mut queue = QueueState {
            next_id: 2,
            items: vec![QueueItem {
                id: 1,
                config: DownloadConfig {
                    url: "https://example.org/path/file.iso".to_owned(),
                    ..DownloadConfig::default()
                },
                category: DownloadCategory::Other,
                priority: QueuePriority::Normal,
                status: QueueItemStatus::Queued,
                added_at_unix_ms: 1,
                last_error: None,
            }],
            rules: default_category_rules(),
        };

        let outcome = queue.add_download_dedup(
            DownloadConfig {
                url: "https://example.org/path/file.iso#fragment".to_owned(),
                ..DownloadConfig::default()
            },
            QueuePriority::High,
            true,
        );
        assert_eq!(outcome, QueueAddOutcome::Duplicate { existing_id: 1 });

        queue.items[0].status = QueueItemStatus::Completed;
        let outcome = queue.add_download_dedup(
            DownloadConfig {
                url: "https://example.org/path/file.iso".to_owned(),
                ..DownloadConfig::default()
            },
            QueuePriority::High,
            true,
        );
        assert!(matches!(outcome, QueueAddOutcome::Added { .. }));
    }
}
