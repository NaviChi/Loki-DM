#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::path::PathBuf;

use loki_dm_core::{
    CategoryRule, DownloadCategory, DownloadConfig, QueueItem, QueueItemStatus, QueuePriority,
    QueueState, default_category_rules, merge_external_queue_state,
};
use tempfile::tempdir;

fn queue_item(id: u64, status: QueueItemStatus, url: &str, error: Option<&str>) -> QueueItem {
    QueueItem {
        id,
        config: DownloadConfig {
            url: url.to_owned(),
            output_path: PathBuf::from(format!("/tmp/{id}.bin")),
            ..DownloadConfig::default()
        },
        category: DownloadCategory::Other,
        priority: QueuePriority::Normal,
        status,
        added_at_unix_ms: 1,
        last_error: error.map(ToOwned::to_owned),
    }
}

#[test]
fn file_backed_merge_preserves_terminal_local_state_and_imports_new_items() {
    let dir = tempdir().expect("tempdir");
    let queue_path = dir.path().join("queue.json");

    let mut local = QueueState {
        next_id: 2,
        items: vec![queue_item(
            1,
            QueueItemStatus::Running,
            "https://local/1",
            Some("local-error"),
        )],
        rules: default_category_rules(),
    };

    let disk = QueueState {
        next_id: 9,
        items: vec![
            queue_item(1, QueueItemStatus::Queued, "https://disk/1", None),
            queue_item(2, QueueItemStatus::Queued, "https://disk/2", None),
        ],
        rules: Vec::new(),
    };
    disk.save(Some(&queue_path)).expect("save disk queue");

    let loaded = QueueState::load(Some(&queue_path)).expect("load disk queue");
    let report = merge_external_queue_state(&mut local, loaded);
    assert_eq!(report.new_items_added, 1);
    assert_eq!(report.existing_items_merged, 1);
    assert_eq!(local.next_id, 9);

    let item1 = local.items.iter().find(|item| item.id == 1).expect("item1");
    assert_eq!(item1.config.url, "https://disk/1");
    assert_eq!(item1.status, QueueItemStatus::Running);
    assert_eq!(item1.last_error.as_deref(), Some("local-error"));

    let item2 = local.items.iter().find(|item| item.id == 2).expect("item2");
    assert_eq!(item2.config.url, "https://disk/2");
    assert_eq!(item2.status, QueueItemStatus::Queued);
}

#[test]
fn file_backed_merge_replaces_rules_only_when_disk_rules_non_empty() {
    let dir = tempdir().expect("tempdir");
    let queue_path = dir.path().join("queue.json");

    let mut local = QueueState::default();
    let original_rules = local.rules.clone();

    let disk_empty_rules = QueueState {
        next_id: 1,
        items: Vec::new(),
        rules: Vec::new(),
    };
    disk_empty_rules
        .save(Some(&queue_path))
        .expect("save empty-rules queue");

    let loaded_empty = QueueState::load(Some(&queue_path)).expect("load empty-rules queue");
    let report_empty = merge_external_queue_state(&mut local, loaded_empty);
    assert!(!report_empty.rules_replaced);
    assert_eq!(local.rules, original_rules);

    let replacement_rules = vec![CategoryRule {
        name: "host-video".to_owned(),
        category: DownloadCategory::Video,
        extensions: Vec::new(),
        host_contains: Some("example.org".to_owned()),
    }];
    let disk_replacement = QueueState {
        next_id: 1,
        items: Vec::new(),
        rules: replacement_rules.clone(),
    };
    disk_replacement
        .save(Some(&queue_path))
        .expect("save replacement-rules queue");

    let loaded_replacement =
        QueueState::load(Some(&queue_path)).expect("load replacement-rules queue");
    let report_replacement = merge_external_queue_state(&mut local, loaded_replacement);
    assert!(report_replacement.rules_replaced);
    assert_eq!(local.rules, replacement_rules);
}
