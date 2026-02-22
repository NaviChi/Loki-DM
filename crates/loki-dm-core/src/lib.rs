#![allow(
    clippy::assigning_clones,
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    clippy::derivable_impls,
    clippy::ignored_unit_patterns,
    clippy::manual_let_else,
    clippy::map_unwrap_or,
    clippy::missing_errors_doc,
    clippy::must_use_candidate,
    clippy::needless_pass_by_value,
    clippy::new_without_default,
    clippy::ref_option,
    clippy::single_match_else,
    clippy::struct_excessive_bools,
    clippy::too_many_arguments,
    clippy::too_many_lines,
    clippy::unnested_or_patterns,
    clippy::unused_async
)]
#![cfg_attr(test, allow(clippy::expect_used, clippy::unwrap_used))]

pub mod av;
pub mod config;
pub mod cookies;
pub mod engine;
pub mod error;
pub mod external_downloader;
pub mod media;
pub mod metadata;
pub mod native_messaging;
pub mod queue;
pub mod rate_limit;
pub mod scheduler;
pub mod settings;
pub mod spider;
pub mod types;
pub mod updater;

pub use config::{AuthConfig, DownloadConfig, EngineSettings, ProxyConfig, ProxyKind};
pub use cookies::{
    load_cookie_file, merge_cookie_sources, parse_cookie_pair, render_cookie_header,
};
pub use engine::{ControlState, DownloadEngine, DownloadHandle};
pub use error::{LokiDmError, Result};
pub use media::{MediaFormat, MediaProbe, MediaSourceKind};
pub use native_messaging::{
    CHROME_EXTENSION_ID_PLACEHOLDER, DEFAULT_FIREFOX_EXTENSION_ID, DEFAULT_NATIVE_HOST_NAME,
    NativeHostDiagnostics, NativeHostInstallReport, NativeHostManifestSpec,
    NativeHostValidationReport, NativeRequest, NativeRequestAction, NativeResponse,
    collect_native_host_diagnostics,
};
pub use queue::{
    CategoryRule, DownloadCategory, QueueAddOutcome, QueueItem, QueueItemStatus, QueueMergeReport,
    QueuePriority, QueueState, classify_url, default_category_rules, merge_external_queue_state,
    urls_from_text,
};
pub use scheduler::{DownloadScheduler, ScheduleKind, ScheduleSpec, ScheduledDownload};
pub use settings::{
    AdvancedSettings, AppSettings, AppearanceSettings, BrowserSettings, ConnectionSettings,
    ProxySettings, SchedulerSettings, ThemeMode, ToolbarSkin,
};
pub use types::{DownloadEvent, DownloadProgress, DownloadStatus};
pub use updater::{ReleaseAsset, UpdateInfo};
