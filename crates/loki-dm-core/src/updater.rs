use std::path::{Path, PathBuf};

use futures_util::StreamExt;
use semver::Version;
use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;

use crate::error::{LokiDmError, Result};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReleaseAsset {
    pub name: String,
    pub download_url: String,
    pub size: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateInfo {
    pub current_version: String,
    pub latest_version: String,
    pub update_available: bool,
    pub release_name: Option<String>,
    pub release_notes: Option<String>,
    pub published_at: Option<String>,
    pub assets: Vec<ReleaseAsset>,
}

#[derive(Debug, Deserialize)]
struct GitHubRelease {
    tag_name: String,
    name: Option<String>,
    body: Option<String>,
    published_at: Option<String>,
    assets: Vec<GitHubAsset>,
}

#[derive(Debug, Deserialize)]
struct GitHubAsset {
    name: String,
    browser_download_url: String,
    size: Option<u64>,
}

pub async fn check_for_updates(endpoint: &str, current_version: &str) -> Result<UpdateInfo> {
    let client = reqwest::Client::builder()
        .user_agent("Loki-DM-Updater/0.1")
        .build()?;

    let release: GitHubRelease = client
        .get(endpoint)
        .header("Accept", "application/vnd.github+json")
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;

    let latest = normalize_version(&release.tag_name);
    let current = normalize_version(current_version);

    let latest_semver = Version::parse(&latest)
        .map_err(|err| LokiDmError::Message(format!("invalid latest version '{latest}': {err}")))?;
    let current_semver = Version::parse(&current).map_err(|err| {
        LokiDmError::Message(format!("invalid current version '{current}': {err}"))
    })?;

    Ok(UpdateInfo {
        current_version: current,
        latest_version: latest.clone(),
        update_available: latest_semver > current_semver,
        release_name: release.name,
        release_notes: release.body,
        published_at: release.published_at,
        assets: release
            .assets
            .into_iter()
            .map(|asset| ReleaseAsset {
                name: asset.name,
                download_url: asset.browser_download_url,
                size: asset.size,
            })
            .collect(),
    })
}

#[must_use]
pub fn select_asset_for_current_platform(info: &UpdateInfo) -> Option<ReleaseAsset> {
    let os = std::env::consts::OS;
    let arch = std::env::consts::ARCH;

    let preferred_terms = match os {
        "windows" => vec!["windows", "win", arch, ".msi", ".exe", ".zip"],
        "macos" => vec!["macos", "darwin", arch, ".dmg", ".pkg", ".zip"],
        "linux" => vec!["linux", arch, ".deb", ".rpm", ".appimage", ".tar"],
        _ => vec![arch],
    };

    for term in preferred_terms {
        if let Some(asset) = info.assets.iter().find(|asset| {
            asset
                .name
                .to_ascii_lowercase()
                .contains(&term.to_ascii_lowercase())
        }) {
            return Some(asset.clone());
        }
    }

    info.assets.first().cloned()
}

pub async fn download_release_asset(asset: &ReleaseAsset, output_dir: &Path) -> Result<PathBuf> {
    tokio::fs::create_dir_all(output_dir).await?;
    let destination = output_dir.join(&asset.name);

    let client = reqwest::Client::builder()
        .user_agent("Loki-DM-Updater/0.1")
        .build()?;

    let response = client
        .get(&asset.download_url)
        .send()
        .await?
        .error_for_status()?;
    let mut stream = response.bytes_stream();

    let mut file = tokio::fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&destination)
        .await?;

    while let Some(chunk) = stream.next().await {
        let bytes = chunk?;
        file.write_all(&bytes).await?;
    }

    file.flush().await?;
    Ok(destination)
}

pub async fn launch_installer(path: &Path) -> Result<()> {
    #[cfg(target_os = "windows")]
    {
        tokio::process::Command::new("cmd")
            .arg("/C")
            .arg("start")
            .arg(path)
            .spawn()
            .map_err(|err| LokiDmError::Message(format!("failed to launch installer: {err}")))?;
    }

    #[cfg(target_os = "macos")]
    {
        tokio::process::Command::new("open")
            .arg(path)
            .spawn()
            .map_err(|err| LokiDmError::Message(format!("failed to launch installer: {err}")))?;
    }

    #[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
    {
        tokio::process::Command::new("xdg-open")
            .arg(path)
            .spawn()
            .map_err(|err| LokiDmError::Message(format!("failed to launch installer: {err}")))?;
    }

    Ok(())
}

fn normalize_version(raw: &str) -> String {
    raw.trim().trim_start_matches('v').to_owned()
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn normalize_strips_v_prefix() {
        assert_eq!(normalize_version("v1.2.3"), "1.2.3");
        assert_eq!(normalize_version("1.2.3"), "1.2.3");
    }

    #[test]
    fn picks_some_asset_when_available() {
        let info = UpdateInfo {
            current_version: "1.0.0".to_owned(),
            latest_version: "1.1.0".to_owned(),
            update_available: true,
            release_name: None,
            release_notes: None,
            published_at: None,
            assets: vec![
                ReleaseAsset {
                    name: "loki-dm-linux-x86_64.AppImage".to_owned(),
                    download_url: "https://example.invalid/linux".to_owned(),
                    size: Some(1),
                },
                ReleaseAsset {
                    name: "loki-dm-windows-x86_64.msi".to_owned(),
                    download_url: "https://example.invalid/windows".to_owned(),
                    size: Some(2),
                },
            ],
        };

        assert!(select_asset_for_current_platform(&info).is_some());
    }
}
