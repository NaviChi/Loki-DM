use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::config::ProxyConfig;
use crate::error::Result;
use crate::native_messaging::{
    CHROME_EXTENSION_ID_PLACEHOLDER, DEFAULT_FIREFOX_EXTENSION_ID, DEFAULT_NATIVE_HOST_NAME,
};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ThemeMode {
    Auto,
    Light,
    Dark,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum ToolbarSkin {
    #[default]
    Classic,
    Windows10Flat,
    NeonGlow,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ConnectionSettings {
    pub initial_connections: u16,
    pub min_connections: u16,
    pub max_connections: u16,
    pub min_segment_size: u64,
    pub global_speed_limit_bps: Option<u64>,
    pub default_download_speed_limit_bps: Option<u64>,
    pub default_hour_quota_mb: Option<u64>,
}

impl Default for ConnectionSettings {
    fn default() -> Self {
        Self {
            initial_connections: 8,
            min_connections: 2,
            max_connections: 16,
            min_segment_size: 512 * 1024,
            global_speed_limit_bps: None,
            default_download_speed_limit_bps: None,
            default_hour_quota_mb: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ProxySettings {
    pub enabled: bool,
    pub proxy: Option<ProxyConfig>,
}

impl Default for ProxySettings {
    fn default() -> Self {
        Self {
            enabled: false,
            proxy: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SchedulerSettings {
    pub enabled: bool,
    pub run_missed_jobs_on_startup: bool,
    pub default_interval_minutes: u64,
    pub wake_from_sleep: bool,
    pub queue_concurrent_downloads: usize,
}

impl Default for SchedulerSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            run_missed_jobs_on_startup: true,
            default_interval_minutes: 60,
            wake_from_sleep: false,
            queue_concurrent_downloads: 3,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct BrowserSettings {
    pub integration_enabled: bool,
    pub native_host_name: String,
    pub intercept_all_downloads: bool,
    pub native_host_binary_path: Option<PathBuf>,
    pub chrome_extension_id: String,
    pub firefox_extension_id: String,
    pub manifest_output_dir: PathBuf,
}

impl Default for BrowserSettings {
    fn default() -> Self {
        Self {
            integration_enabled: true,
            native_host_name: DEFAULT_NATIVE_HOST_NAME.to_owned(),
            intercept_all_downloads: false,
            native_host_binary_path: None,
            chrome_extension_id: CHROME_EXTENSION_ID_PLACEHOLDER.to_owned(),
            firefox_extension_id: DEFAULT_FIREFOX_EXTENSION_ID.to_owned(),
            manifest_output_dir: default_native_host_manifest_output_dir(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AppearanceSettings {
    pub theme: ThemeMode,
    pub toolbar_skin: ToolbarSkin,
    pub language: String,
    pub compact_rows: bool,
}

impl Default for AppearanceSettings {
    fn default() -> Self {
        Self {
            theme: ThemeMode::Dark,
            toolbar_skin: ToolbarSkin::Classic,
            language: "en-US".to_owned(),
            compact_rows: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AdvancedSettings {
    pub retry_count: u8,
    pub check_for_updates: bool,
    pub update_endpoint: String,
    pub av_hook_command: Option<String>,
    pub queue_complete_command: Option<String>,
    pub crash_reporting_enabled: bool,
    pub sound_on_completion: bool,
    pub prevent_duplicate_queue_entries: bool,
}

impl Default for AdvancedSettings {
    fn default() -> Self {
        Self {
            retry_count: 6,
            check_for_updates: true,
            update_endpoint: "https://api.github.com/repos/navi/loki-dm/releases/latest".to_owned(),
            av_hook_command: None,
            queue_complete_command: None,
            crash_reporting_enabled: false,
            sound_on_completion: true,
            prevent_duplicate_queue_entries: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AppSettings {
    pub download_dir: PathBuf,
    pub category_dirs: BTreeMap<String, PathBuf>,
    pub connection: ConnectionSettings,
    pub proxy: ProxySettings,
    pub scheduler: SchedulerSettings,
    pub browser: BrowserSettings,
    pub appearance: AppearanceSettings,
    pub advanced: AdvancedSettings,
}

impl Default for AppSettings {
    fn default() -> Self {
        let download_dir = default_download_dir();
        Self {
            category_dirs: default_category_dirs_for(&download_dir),
            download_dir,
            connection: ConnectionSettings::default(),
            proxy: ProxySettings::default(),
            scheduler: SchedulerSettings::default(),
            browser: BrowserSettings::default(),
            appearance: AppearanceSettings::default(),
            advanced: AdvancedSettings::default(),
        }
    }
}

impl AppSettings {
    pub fn load(path: Option<&Path>) -> Result<Self> {
        let file = path.map_or_else(default_settings_path, Path::to_path_buf);
        if !file.exists() {
            return Ok(Self::default());
        }

        let raw = fs::read_to_string(&file)?;
        let mut decoded = decode_settings(&file, &raw)?;
        decoded.normalize_category_dirs();
        Ok(decoded)
    }

    pub fn save(&self, path: Option<&Path>) -> Result<PathBuf> {
        let file = path.map_or_else(default_settings_path, Path::to_path_buf);
        if let Some(parent) = file.parent() {
            fs::create_dir_all(parent)?;
        }

        let encoded = encode_settings(&file, self)?;
        fs::write(&file, encoded)?;
        Ok(file)
    }

    pub fn normalize_category_dirs(&mut self) {
        let defaults = default_category_dirs_for(&self.download_dir);
        for (name, path) in defaults {
            self.category_dirs.entry(name).or_insert(path);
        }
    }

    #[must_use]
    pub fn category_output_dir(&self, category: Option<&str>) -> PathBuf {
        let Some(category) = category.map(str::trim).filter(|v| !v.is_empty()) else {
            return self.download_dir.clone();
        };

        if let Some((_, path)) = self
            .category_dirs
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case(category))
        {
            return path.clone();
        }

        self.download_dir.clone()
    }
}

#[must_use]
pub fn app_config_dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("loki-dm")
}

#[must_use]
pub fn default_settings_path() -> PathBuf {
    app_config_dir().join("settings.toml")
}

#[must_use]
pub fn default_download_dir() -> PathBuf {
    dirs::download_dir().unwrap_or_else(|| PathBuf::from("."))
}

#[must_use]
pub fn category_names() -> [&'static str; 6] {
    [
        "Music",
        "Video",
        "Programs",
        "Documents",
        "Compressed",
        "Other",
    ]
}

#[must_use]
pub fn default_category_dirs_for(download_root: &Path) -> BTreeMap<String, PathBuf> {
    let mut map = BTreeMap::new();
    for category in category_names() {
        map.insert(category.to_owned(), download_root.join(category));
    }
    map
}

#[must_use]
pub fn default_native_host_manifest_output_dir() -> PathBuf {
    app_config_dir().join("native-host")
}

fn decode_settings(path: &Path, raw: &str) -> Result<AppSettings> {
    match path
        .extension()
        .and_then(std::ffi::OsStr::to_str)
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("json") => Ok(serde_json::from_str(raw)?),
        _ => {
            let toml_try = toml::from_str(raw);
            if let Ok(parsed) = toml_try {
                Ok(parsed)
            } else {
                Ok(serde_json::from_str(raw)?)
            }
        }
    }
}

fn encode_settings(path: &Path, settings: &AppSettings) -> Result<String> {
    match path
        .extension()
        .and_then(std::ffi::OsStr::to_str)
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("json") => Ok(serde_json::to_string_pretty(settings)?),
        _ => Ok(toml::to_string_pretty(settings)?),
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_json() {
        let settings = AppSettings::default();
        let encoded = serde_json::to_string_pretty(&settings).expect("serialize settings");
        let decoded: AppSettings = serde_json::from_str(&encoded).expect("deserialize settings");
        assert_eq!(decoded.appearance.language, "en-US");
    }

    #[test]
    fn legacy_browser_settings_deserialize_with_defaults() {
        let legacy = r#"{
            "download_dir": ".",
            "connection": {
                "initial_connections": 8,
                "min_connections": 2,
                "max_connections": 16,
                "min_segment_size": 524288,
                "global_speed_limit_bps": null,
                "default_download_speed_limit_bps": null,
                "default_hour_quota_mb": null
            },
            "proxy": { "enabled": false, "proxy": null },
            "scheduler": {
                "enabled": false,
                "run_missed_jobs_on_startup": true,
                "default_interval_minutes": 60,
                "wake_from_sleep": false
            },
            "browser": {
                "integration_enabled": true,
                "native_host_name": "com.loki.dm",
                "intercept_all_downloads": false
            },
            "appearance": {
                "theme": "Auto",
                "language": "en-US",
                "compact_rows": false
            },
            "advanced": {
                "retry_count": 6,
                "check_for_updates": true,
                "update_endpoint": "https://example.invalid/releases/latest",
                "av_hook_command": null,
                "crash_reporting_enabled": false
            }
        }"#;

        let parsed: AppSettings =
            serde_json::from_str(legacy).expect("deserialize legacy settings");
        assert_eq!(parsed.appearance.toolbar_skin, ToolbarSkin::Classic);
        assert_eq!(
            parsed.browser.chrome_extension_id,
            CHROME_EXTENSION_ID_PLACEHOLDER
        );
        assert_eq!(
            parsed.browser.firefox_extension_id,
            DEFAULT_FIREFOX_EXTENSION_ID
        );
        assert!(parsed.browser.manifest_output_dir.ends_with("native-host"));
        assert!(parsed.category_dirs.contains_key("Music"));
        assert!(parsed.category_dirs.contains_key("Video"));
        assert!(parsed.advanced.sound_on_completion);
        assert!(parsed.advanced.prevent_duplicate_queue_entries);
        assert!(parsed.advanced.queue_complete_command.is_none());
        assert_eq!(parsed.scheduler.queue_concurrent_downloads, 3);
    }
}
