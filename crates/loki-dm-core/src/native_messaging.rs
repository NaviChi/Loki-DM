use std::collections::BTreeSet;
use std::fs;
use std::io::{ErrorKind, Read, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::error::{LokiDmError, Result};

pub const DEFAULT_NATIVE_HOST_NAME: &str = "com.loki.dm";
pub const DEFAULT_FIREFOX_EXTENSION_ID: &str = "loki-dm@example.org";
pub const CHROME_EXTENSION_ID_PLACEHOLDER: &str = "REPLACE_WITH_EXTENSION_ID";

const MAX_NATIVE_MESSAGE_SIZE: usize = 8 * 1024 * 1024;

#[cfg(windows)]
const CHROMIUM_REGISTRY_PREFIXES: [&str; 4] = [
    "Software\\Google\\Chrome\\NativeMessagingHosts",
    "Software\\Chromium\\NativeMessagingHosts",
    "Software\\Microsoft\\Edge\\NativeMessagingHosts",
    "Software\\BraveSoftware\\Brave-Browser\\NativeMessagingHosts",
];

#[cfg(windows)]
const FIREFOX_REGISTRY_PREFIX: &str = "Software\\Mozilla\\NativeMessagingHosts";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeBrowser {
    Chromium,
    Firefox,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum NativeRequestAction {
    #[default]
    Queue,
    Download,
    Ping,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct NativeRequest {
    pub url: Option<String>,
    #[serde(default)]
    pub urls: Vec<String>,
    #[serde(default)]
    pub action: NativeRequestAction,
    pub output: Option<String>,
    pub output_dir: Option<String>,
    pub connections: Option<u16>,
    pub speed_limit_bps: Option<u64>,
    pub priority: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct NativeResponse {
    pub ok: bool,
    pub message: String,
    pub output_path: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub queue_ids: Vec<u64>,
}

#[derive(Debug, Clone)]
pub struct NativeHostManifestSpec {
    pub host_name: String,
    pub binary_path: PathBuf,
    pub chrome_extension_id: String,
    pub firefox_extension_id: String,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct NativeHostInstallReport {
    pub manifest_files_written: Vec<PathBuf>,
    pub registry_entries_written: Vec<String>,
    pub manifest_files_removed: Vec<PathBuf>,
    pub registry_entries_removed: Vec<String>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct NativeHostValidationReport {
    pub manifest_files_present: Vec<PathBuf>,
    pub manifest_files_missing: Vec<PathBuf>,
    pub registry_entries_present: Vec<String>,
    pub registry_entries_missing: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct NativeHostDiagnostics {
    pub host_name: String,
    pub binary_path: PathBuf,
    pub binary_path_valid: bool,
    pub chrome_extension_id: String,
    pub chrome_extension_id_placeholder: bool,
    pub firefox_extension_id: String,
    pub firefox_extension_id_empty: bool,
    pub manifest_output_dir: Option<PathBuf>,
    pub manifest_output_dir_exists: Option<bool>,
    pub validation: NativeHostValidationReport,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone)]
struct ManifestTarget {
    browser: NativeBrowser,
    path: PathBuf,
}

#[cfg(windows)]
#[derive(Debug, Clone)]
struct RegistryTarget {
    key_path: String,
    expected_value: String,
}

impl NativeHostManifestSpec {
    #[must_use]
    pub fn with_binary_path(binary_path: PathBuf) -> Self {
        Self {
            host_name: DEFAULT_NATIVE_HOST_NAME.to_owned(),
            binary_path,
            chrome_extension_id: CHROME_EXTENSION_ID_PLACEHOLDER.to_owned(),
            firefox_extension_id: DEFAULT_FIREFOX_EXTENSION_ID.to_owned(),
        }
    }
}

impl NativeBrowser {
    #[must_use]
    pub fn install_manifest_path(self, host_name: &str) -> Option<PathBuf> {
        let home = dirs::home_dir()?;
        let file_name = format!("{host_name}.json");
        if cfg!(target_os = "linux") {
            return match self {
                Self::Chromium => Some(
                    home.join(".config")
                        .join("google-chrome")
                        .join("NativeMessagingHosts")
                        .join(file_name),
                ),
                Self::Firefox => Some(
                    home.join(".mozilla")
                        .join("native-messaging-hosts")
                        .join(file_name),
                ),
            };
        }

        if cfg!(target_os = "macos") {
            return match self {
                Self::Chromium => Some(
                    home.join("Library")
                        .join("Application Support")
                        .join("Google")
                        .join("Chrome")
                        .join("NativeMessagingHosts")
                        .join(file_name),
                ),
                Self::Firefox => Some(
                    home.join("Library")
                        .join("Application Support")
                        .join("Mozilla")
                        .join("NativeMessagingHosts")
                        .join(file_name),
                ),
            };
        }

        None
    }

    #[must_use]
    pub fn extra_install_paths(self, host_name: &str) -> Vec<PathBuf> {
        let Some(home) = dirs::home_dir() else {
            return Vec::new();
        };
        let file_name = format!("{host_name}.json");

        if cfg!(target_os = "linux") && matches!(self, Self::Chromium) {
            return vec![
                home.join(".config")
                    .join("chromium")
                    .join("NativeMessagingHosts")
                    .join(&file_name),
                home.join(".config")
                    .join("BraveSoftware")
                    .join("Brave-Browser")
                    .join("NativeMessagingHosts")
                    .join(&file_name),
                home.join(".config")
                    .join("microsoft-edge")
                    .join("NativeMessagingHosts")
                    .join(file_name),
            ];
        }

        if cfg!(target_os = "macos") && matches!(self, Self::Chromium) {
            return vec![
                home.join("Library")
                    .join("Application Support")
                    .join("Chromium")
                    .join("NativeMessagingHosts")
                    .join(&file_name),
                home.join("Library")
                    .join("Application Support")
                    .join("BraveSoftware")
                    .join("Brave-Browser")
                    .join("NativeMessagingHosts")
                    .join(&file_name),
                home.join("Library")
                    .join("Application Support")
                    .join("Microsoft Edge")
                    .join("NativeMessagingHosts")
                    .join(file_name),
            ];
        }

        Vec::new()
    }
}

pub fn read_native_message<R: Read>(reader: &mut R) -> Result<Option<NativeRequest>> {
    let mut len_bytes = [0_u8; 4];
    match reader.read_exact(&mut len_bytes) {
        Ok(()) => {}
        Err(err) if err.kind() == ErrorKind::UnexpectedEof => return Ok(None),
        Err(err) => return Err(err.into()),
    }

    let payload_len = u32::from_le_bytes(len_bytes) as usize;
    if payload_len > MAX_NATIVE_MESSAGE_SIZE {
        return Err(LokiDmError::Message(format!(
            "native message too large: {payload_len} bytes"
        )));
    }

    let mut payload = vec![0_u8; payload_len];
    reader.read_exact(&mut payload)?;
    let request = serde_json::from_slice::<NativeRequest>(&payload)?;
    Ok(Some(request))
}

pub fn write_native_message<W: Write>(writer: &mut W, response: &NativeResponse) -> Result<()> {
    let payload = serde_json::to_vec(response)?;
    let payload_len = (payload.len() as u32).to_le_bytes();
    writer.write_all(&payload_len)?;
    writer.write_all(&payload)?;
    writer.flush()?;
    Ok(())
}

#[must_use]
pub fn chromium_manifest(spec: &NativeHostManifestSpec) -> serde_json::Value {
    json!({
        "name": spec.host_name,
        "description": "Loki DM native messaging host",
        "path": spec.binary_path.display().to_string(),
        "type": "stdio",
        "allowed_origins": [
            format!("chrome-extension://{}/", spec.chrome_extension_id)
        ]
    })
}

#[must_use]
pub fn firefox_manifest(spec: &NativeHostManifestSpec) -> serde_json::Value {
    json!({
        "name": spec.host_name,
        "description": "Loki DM native messaging host",
        "path": spec.binary_path.display().to_string(),
        "type": "stdio",
        "allowed_extensions": [
            spec.firefox_extension_id
        ]
    })
}

pub fn write_manifest_pair(
    output_dir: &Path,
    spec: &NativeHostManifestSpec,
) -> Result<(PathBuf, PathBuf)> {
    fs::create_dir_all(output_dir)?;
    let chrome_path = output_dir.join(format!("{}.chrome.json", spec.host_name));
    let firefox_path = output_dir.join(format!("{}.firefox.json", spec.host_name));
    let chrome_encoded = serde_json::to_string_pretty(&chromium_manifest(spec))?;
    let firefox_encoded = serde_json::to_string_pretty(&firefox_manifest(spec))?;

    fs::write(&chrome_path, chrome_encoded)?;
    fs::write(&firefox_path, firefox_encoded)?;
    Ok((chrome_path, firefox_path))
}

pub fn install_manifests(spec: &NativeHostManifestSpec) -> Result<NativeHostInstallReport> {
    let mut report = NativeHostInstallReport::default();
    if spec.chrome_extension_id.trim().is_empty()
        || spec.chrome_extension_id == CHROME_EXTENSION_ID_PLACEHOLDER
    {
        report.warnings.push(
            "set chrome_extension_id to your real unpacked extension id before browser use"
                .to_owned(),
        );
    }
    let chrome_encoded = serde_json::to_string_pretty(&chromium_manifest(spec))?;
    let firefox_encoded = serde_json::to_string_pretty(&firefox_manifest(spec))?;
    let targets = collect_manifest_targets(spec);

    for target in &targets {
        if let Some(parent) = target.path.parent()
            && let Err(err) = fs::create_dir_all(parent)
        {
            report
                .warnings
                .push(format!("failed to create {}: {err}", parent.display()));
            continue;
        }

        let encoded = match target.browser {
            NativeBrowser::Chromium => &chrome_encoded,
            NativeBrowser::Firefox => &firefox_encoded,
        };

        match fs::write(&target.path, encoded) {
            Ok(()) => report.manifest_files_written.push(target.path.clone()),
            Err(err) => report.warnings.push(format!(
                "failed to write manifest {}: {err}",
                target.path.display()
            )),
        }
    }

    #[cfg(windows)]
    {
        for entry in collect_registry_targets(spec, &targets) {
            match set_registry_default_value(&entry.key_path, &entry.expected_value) {
                Ok(()) => report.registry_entries_written.push(entry.key_path),
                Err(err) => report.warnings.push(format!(
                    "failed to write registry key {}: {err}",
                    entry.key_path
                )),
            }
        }
    }

    Ok(report)
}

pub fn uninstall_manifests(spec: &NativeHostManifestSpec) -> Result<NativeHostInstallReport> {
    let mut report = NativeHostInstallReport::default();
    let targets = collect_manifest_targets(spec);

    for target in &targets {
        if !target.path.exists() {
            continue;
        }

        match fs::remove_file(&target.path) {
            Ok(()) => report.manifest_files_removed.push(target.path.clone()),
            Err(err) => report.warnings.push(format!(
                "failed to remove manifest {}: {err}",
                target.path.display()
            )),
        }
    }

    #[cfg(windows)]
    {
        for entry in collect_registry_targets(spec, &targets) {
            match delete_registry_key(&entry.key_path) {
                Ok(()) => report.registry_entries_removed.push(entry.key_path),
                Err(err) if err.kind() == ErrorKind::NotFound => {}
                Err(err) => report.warnings.push(format!(
                    "failed to remove registry key {}: {err}",
                    entry.key_path
                )),
            }
        }
    }

    Ok(report)
}

pub fn validate_installation(spec: &NativeHostManifestSpec) -> Result<NativeHostValidationReport> {
    let mut report = NativeHostValidationReport::default();
    let targets = collect_manifest_targets(spec);
    for target in &targets {
        if target.path.exists() {
            report.manifest_files_present.push(target.path.clone());
        } else {
            report.manifest_files_missing.push(target.path.clone());
        }
    }

    #[cfg(windows)]
    {
        for entry in collect_registry_targets(spec, &targets) {
            match get_registry_default_value(&entry.key_path) {
                Ok(actual)
                    if normalized_windows_path(&actual)
                        == normalized_windows_path(&entry.expected_value) =>
                {
                    report.registry_entries_present.push(entry.key_path);
                }
                Ok(actual) => {
                    report.registry_entries_missing.push(format!(
                        "{} (value mismatch: expected='{}' actual='{}')",
                        entry.key_path, entry.expected_value, actual
                    ));
                }
                Err(err) if err.kind() == ErrorKind::NotFound => {
                    report.registry_entries_missing.push(entry.key_path);
                }
                Err(err) => {
                    report
                        .registry_entries_missing
                        .push(format!("{} ({err})", entry.key_path));
                }
            }
        }
    }

    Ok(report)
}

pub fn collect_native_host_diagnostics(
    spec: &NativeHostManifestSpec,
    manifest_output_dir: Option<&Path>,
) -> Result<NativeHostDiagnostics> {
    let validation = validate_installation(spec)?;
    Ok(NativeHostDiagnostics {
        host_name: spec.host_name.clone(),
        binary_path: spec.binary_path.clone(),
        binary_path_valid: spec.binary_path.is_file(),
        chrome_extension_id: spec.chrome_extension_id.clone(),
        chrome_extension_id_placeholder: spec.chrome_extension_id.trim().is_empty()
            || spec.chrome_extension_id == CHROME_EXTENSION_ID_PLACEHOLDER,
        firefox_extension_id: spec.firefox_extension_id.clone(),
        firefox_extension_id_empty: spec.firefox_extension_id.trim().is_empty(),
        manifest_output_dir: manifest_output_dir.map(Path::to_path_buf),
        manifest_output_dir_exists: manifest_output_dir.map(Path::is_dir),
        validation,
        notes: vec![
            "validation checks browser install locations/registry, not output_dir".to_owned(),
            "queue-first browser handoff is active".to_owned(),
        ],
    })
}

fn collect_manifest_targets(spec: &NativeHostManifestSpec) -> Vec<ManifestTarget> {
    #[cfg(windows)]
    {
        let mut targets = Vec::new();
        let base = dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("loki-dm")
            .join("native-host");
        targets.push(ManifestTarget {
            browser: NativeBrowser::Chromium,
            path: base.join(format!("{}.chrome.json", spec.host_name)),
        });
        targets.push(ManifestTarget {
            browser: NativeBrowser::Firefox,
            path: base.join(format!("{}.firefox.json", spec.host_name)),
        });
        dedupe_manifest_targets(targets)
    }

    #[cfg(not(windows))]
    {
        let mut targets = Vec::new();
        for browser in [NativeBrowser::Chromium, NativeBrowser::Firefox] {
            if let Some(primary) = browser.install_manifest_path(&spec.host_name) {
                targets.push(ManifestTarget {
                    browser,
                    path: primary,
                });
            }
            for extra in browser.extra_install_paths(&spec.host_name) {
                targets.push(ManifestTarget {
                    browser,
                    path: extra,
                });
            }
        }
        dedupe_manifest_targets(targets)
    }
}

fn dedupe_manifest_targets(targets: Vec<ManifestTarget>) -> Vec<ManifestTarget> {
    let mut seen = BTreeSet::new();
    let mut deduped = Vec::new();

    for target in targets {
        let key = format!(
            "{}::{}",
            browser_label(target.browser),
            target.path.display()
        );
        if seen.insert(key) {
            deduped.push(target);
        }
    }

    deduped
}

#[cfg(windows)]
fn collect_registry_targets(
    spec: &NativeHostManifestSpec,
    manifest_targets: &[ManifestTarget],
) -> Vec<RegistryTarget> {
    let chrome_manifest = manifest_targets
        .iter()
        .find(|target| target.browser == NativeBrowser::Chromium)
        .map(|target| target.path.display().to_string());
    let firefox_manifest = manifest_targets
        .iter()
        .find(|target| target.browser == NativeBrowser::Firefox)
        .map(|target| target.path.display().to_string());

    let mut targets = Vec::new();
    if let Some(chrome_manifest) = chrome_manifest {
        for prefix in CHROMIUM_REGISTRY_PREFIXES {
            targets.push(RegistryTarget {
                key_path: format!("{prefix}\\{}", spec.host_name),
                expected_value: chrome_manifest.clone(),
            });
        }
    }

    if let Some(firefox_manifest) = firefox_manifest {
        targets.push(RegistryTarget {
            key_path: format!("{FIREFOX_REGISTRY_PREFIX}\\{}", spec.host_name),
            expected_value: firefox_manifest,
        });
    }

    targets
}

fn browser_label(browser: NativeBrowser) -> &'static str {
    match browser {
        NativeBrowser::Chromium => "chromium",
        NativeBrowser::Firefox => "firefox",
    }
}

#[cfg(windows)]
fn set_registry_default_value(key_path: &str, value: &str) -> std::io::Result<()> {
    use winreg::RegKey;
    use winreg::enums::HKEY_CURRENT_USER;

    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let (key, _) = hkcu.create_subkey(key_path)?;
    key.set_value("", &value)?;
    Ok(())
}

#[cfg(windows)]
fn get_registry_default_value(key_path: &str) -> std::io::Result<String> {
    use winreg::RegKey;
    use winreg::enums::HKEY_CURRENT_USER;

    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let key = hkcu.open_subkey(key_path)?;
    key.get_value("")
}

#[cfg(windows)]
fn delete_registry_key(key_path: &str) -> std::io::Result<()> {
    use winreg::RegKey;
    use winreg::enums::HKEY_CURRENT_USER;

    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    hkcu.delete_subkey(key_path)
}

#[cfg(windows)]
fn normalized_windows_path(path: &str) -> String {
    path.replace('/', "\\").to_ascii_lowercase()
}

#[cfg(test)]
#[allow(
    clippy::cast_possible_truncation,
    clippy::expect_used,
    clippy::unwrap_used
)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};
    use tempfile::tempdir;

    fn unique_host() -> String {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        format!("com.loki.dm.test.{stamp}")
    }

    #[test]
    fn native_message_defaults_to_queue_action() {
        let body = r#"{"url":"https://example.com/file.bin"}"#;
        let parsed: NativeRequest = serde_json::from_str(body).expect("parse request");
        assert_eq!(parsed.action, NativeRequestAction::Queue);
    }

    #[test]
    fn native_message_frame_round_trip() {
        let req = NativeRequest {
            url: Some("https://example.com/file.bin".to_owned()),
            ..NativeRequest::default()
        };
        let payload = serde_json::to_vec(&req).expect("serialize request");
        let mut frame = Vec::with_capacity(payload.len() + 4);
        frame.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        frame.extend_from_slice(&payload);

        let decoded = read_native_message(&mut frame.as_slice())
            .expect("decode frame")
            .expect("message");
        assert_eq!(decoded.url.as_deref(), Some("https://example.com/file.bin"));
        assert_eq!(decoded.action, NativeRequestAction::Queue);
    }

    #[test]
    fn manifests_include_ids_and_binary_path() {
        let spec = NativeHostManifestSpec {
            host_name: "com.loki.dm".to_owned(),
            binary_path: PathBuf::from("/usr/local/bin/loki-dm"),
            chrome_extension_id: "abc123".to_owned(),
            firefox_extension_id: "loki-dm@example.org".to_owned(),
        };

        let chrome = chromium_manifest(&spec);
        let firefox = firefox_manifest(&spec);

        assert_eq!(chrome["name"], "com.loki.dm");
        assert_eq!(chrome["path"], "/usr/local/bin/loki-dm");
        assert_eq!(
            chrome["allowed_origins"][0].as_str(),
            Some("chrome-extension://abc123/")
        );

        assert_eq!(firefox["name"], "com.loki.dm");
        assert_eq!(firefox["path"], "/usr/local/bin/loki-dm");
        assert_eq!(
            firefox["allowed_extensions"][0].as_str(),
            Some("loki-dm@example.org")
        );
    }

    #[test]
    fn install_target_generation_returns_browsers() {
        let spec = NativeHostManifestSpec {
            host_name: unique_host(),
            binary_path: PathBuf::from("/tmp/loki-dm"),
            chrome_extension_id: "abc123".to_owned(),
            firefox_extension_id: DEFAULT_FIREFOX_EXTENSION_ID.to_owned(),
        };
        let targets = collect_manifest_targets(&spec);
        assert!(!targets.is_empty());
        assert!(targets.iter().any(|t| t.browser == NativeBrowser::Chromium));
        assert!(targets.iter().any(|t| t.browser == NativeBrowser::Firefox));
    }

    #[test]
    fn validate_reports_missing_entries_for_fresh_host() {
        let spec = NativeHostManifestSpec {
            host_name: unique_host(),
            binary_path: PathBuf::from("/tmp/loki-dm"),
            chrome_extension_id: "abc123".to_owned(),
            firefox_extension_id: DEFAULT_FIREFOX_EXTENSION_ID.to_owned(),
        };
        let report = validate_installation(&spec).expect("validate installation");
        assert!(!report.manifest_files_missing.is_empty());
    }

    #[test]
    fn uninstall_is_best_effort_for_missing_entries() {
        let spec = NativeHostManifestSpec {
            host_name: unique_host(),
            binary_path: PathBuf::from("/tmp/loki-dm"),
            chrome_extension_id: "abc123".to_owned(),
            firefox_extension_id: DEFAULT_FIREFOX_EXTENSION_ID.to_owned(),
        };
        let report = uninstall_manifests(&spec).expect("uninstall manifests");
        assert!(report.manifest_files_removed.is_empty());
    }

    #[test]
    fn diagnostics_collects_flags_and_validation() {
        let tmp = tempdir().expect("temp dir");
        let spec = NativeHostManifestSpec {
            host_name: unique_host(),
            binary_path: PathBuf::from("/tmp/loki-dm-missing"),
            chrome_extension_id: CHROME_EXTENSION_ID_PLACEHOLDER.to_owned(),
            firefox_extension_id: DEFAULT_FIREFOX_EXTENSION_ID.to_owned(),
        };

        let diagnostics =
            collect_native_host_diagnostics(&spec, Some(tmp.path())).expect("collect diagnostics");
        assert_eq!(diagnostics.host_name, spec.host_name);
        assert!(!diagnostics.binary_path_valid);
        assert!(diagnostics.chrome_extension_id_placeholder);
        assert!(!diagnostics.firefox_extension_id_empty);
        assert_eq!(diagnostics.manifest_output_dir.as_deref(), Some(tmp.path()));
        assert_eq!(diagnostics.manifest_output_dir_exists, Some(true));
        assert!(!diagnostics.validation.manifest_files_missing.is_empty());
    }

    #[cfg(windows)]
    #[test]
    fn windows_registry_targets_include_all_browsers() {
        let spec = NativeHostManifestSpec {
            host_name: "com.loki.dm".to_owned(),
            binary_path: PathBuf::from("C:\\loki-dm\\loki-dm.exe"),
            chrome_extension_id: "abc123".to_owned(),
            firefox_extension_id: DEFAULT_FIREFOX_EXTENSION_ID.to_owned(),
        };
        let manifest_targets = collect_manifest_targets(&spec);
        let reg_targets = collect_registry_targets(&spec, &manifest_targets);
        let keys = reg_targets
            .iter()
            .map(|entry| entry.key_path.clone())
            .collect::<Vec<_>>();

        assert!(
            keys.iter()
                .any(|k| { k == "Software\\Google\\Chrome\\NativeMessagingHosts\\com.loki.dm" })
        );
        assert!(
            keys.iter()
                .any(|k| k == "Software\\Chromium\\NativeMessagingHosts\\com.loki.dm")
        );
        assert!(
            keys.iter()
                .any(|k| { k == "Software\\Microsoft\\Edge\\NativeMessagingHosts\\com.loki.dm" })
        );
        assert!(keys.iter().any(|k| {
            k == "Software\\BraveSoftware\\Brave-Browser\\NativeMessagingHosts\\com.loki.dm"
        }));
        assert!(
            keys.iter()
                .any(|k| k == "Software\\Mozilla\\NativeMessagingHosts\\com.loki.dm")
        );
    }
}
