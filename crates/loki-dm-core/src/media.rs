use std::path::Path;

use m3u8_rs::Playlist;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use url::Url;

use crate::error::{LokiDmError, Result};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum MediaSourceKind {
    Hls,
    YtDlp,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaFormat {
    pub id: String,
    pub ext: Option<String>,
    pub quality: Option<String>,
    pub resolution: Option<String>,
    pub audio_only: bool,
    pub filesize: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaProbe {
    pub title: Option<String>,
    pub source: MediaSourceKind,
    pub extractor: Option<String>,
    pub formats: Vec<MediaFormat>,
}

pub async fn probe_media(url: &str) -> Result<MediaProbe> {
    if is_hls_url(url) {
        return probe_hls(url).await;
    }

    probe_with_ytdlp(url).await
}

pub async fn download_media(url: &str, format_id: Option<&str>, output: &Path) -> Result<()> {
    if is_hls_url(url) {
        return download_hls(url, output).await;
    }

    let mut cmd = Command::new("yt-dlp");
    cmd.arg("--no-playlist").arg("-o").arg(output.as_os_str());

    if let Some(format_id) = format_id {
        cmd.arg("-f").arg(format_id);
    }

    cmd.arg(url);
    let output = cmd.output().await.map_err(|err| {
        LokiDmError::Message(format!(
            "failed to execute yt-dlp (is it installed?): {err}"
        ))
    })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        return Err(LokiDmError::Message(format!(
            "yt-dlp failed with status {:?}: {stderr}",
            output.status.code()
        )));
    }

    Ok(())
}

#[must_use]
pub fn is_hls_url(url: &str) -> bool {
    url.to_ascii_lowercase().contains(".m3u8")
}

async fn probe_with_ytdlp(url: &str) -> Result<MediaProbe> {
    let output = Command::new("yt-dlp")
        .arg("-J")
        .arg("--no-playlist")
        .arg("--skip-download")
        .arg(url)
        .output()
        .await
        .map_err(|err| {
            LokiDmError::Message(format!(
                "failed to execute yt-dlp (is it installed?): {err}"
            ))
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        return Err(LokiDmError::Message(format!(
            "yt-dlp probe failed with status {:?}: {stderr}",
            output.status.code()
        )));
    }

    parse_ytdlp_probe(&output.stdout)
}

fn parse_ytdlp_probe(raw: &[u8]) -> Result<MediaProbe> {
    let value: Value = serde_json::from_slice(raw)?;

    let title = value
        .get("title")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    let extractor = value
        .get("extractor")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);

    let mut formats = Vec::new();
    if let Some(entries) = value.get("formats").and_then(Value::as_array) {
        for fmt in entries {
            let id = fmt
                .get("format_id")
                .and_then(Value::as_str)
                .unwrap_or("unknown")
                .to_owned();
            let vcodec = fmt
                .get("vcodec")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let acodec = fmt
                .get("acodec")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let quality = fmt
                .get("format_note")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned);

            let resolution = fmt
                .get("resolution")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned)
                .or_else(|| {
                    fmt.get("height")
                        .and_then(Value::as_u64)
                        .map(|h| format!("{h}p"))
                });

            formats.push(MediaFormat {
                id,
                ext: fmt
                    .get("ext")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned),
                quality,
                resolution,
                audio_only: vcodec == "none" && acodec != "none",
                filesize: fmt
                    .get("filesize")
                    .and_then(Value::as_u64)
                    .or_else(|| fmt.get("filesize_approx").and_then(Value::as_u64)),
            });
        }
    }

    Ok(MediaProbe {
        title,
        source: MediaSourceKind::YtDlp,
        extractor,
        formats,
    })
}

async fn probe_hls(url: &str) -> Result<MediaProbe> {
    let text = reqwest::get(url).await?.text().await?;
    let parsed = m3u8_rs::parse_playlist_res(text.as_bytes())
        .map_err(|_| LokiDmError::Message("invalid m3u8 playlist".to_owned()))?;

    let mut formats = Vec::new();
    match parsed {
        Playlist::MasterPlaylist(master) => {
            for variant in master.variants {
                formats.push(MediaFormat {
                    id: variant.uri,
                    ext: Some("ts".to_owned()),
                    quality: Some(format!("{} bps", variant.bandwidth)),
                    resolution: variant
                        .resolution
                        .map(|res| format!("{}x{}", res.width, res.height)),
                    audio_only: false,
                    filesize: None,
                });
            }
        }
        Playlist::MediaPlaylist(_) => {
            formats.push(MediaFormat {
                id: "default".to_owned(),
                ext: Some("ts".to_owned()),
                quality: Some("default".to_owned()),
                resolution: None,
                audio_only: false,
                filesize: None,
            });
        }
    }

    Ok(MediaProbe {
        title: None,
        source: MediaSourceKind::Hls,
        extractor: Some("hls".to_owned()),
        formats,
    })
}

async fn download_hls(url: &str, output_path: &Path) -> Result<()> {
    let client = Client::builder().tcp_nodelay(true).build()?;
    let url = Url::parse(url)?;

    let media_url = resolve_hls_media_url(&client, &url).await?;
    let body = client.get(media_url.clone()).send().await?.text().await?;
    let parsed = m3u8_rs::parse_playlist_res(body.as_bytes())
        .map_err(|_| LokiDmError::Message("invalid media m3u8 playlist".to_owned()))?;

    let Playlist::MediaPlaylist(media) = parsed else {
        return Err(LokiDmError::Message(
            "expected media playlist after master selection".to_owned(),
        ));
    };

    if media.segments.is_empty() {
        return Err(LokiDmError::Message(
            "media playlist has no segments".to_owned(),
        ));
    }

    if let Some(parent) = output_path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }

    let mut out = tokio::fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(output_path)
        .await?;

    for segment in media.segments {
        let segment_url = media_url.join(&segment.uri)?;
        let bytes = client.get(segment_url).send().await?.bytes().await?;
        out.write_all(&bytes).await?;
    }

    out.flush().await?;
    Ok(())
}

async fn resolve_hls_media_url(client: &Client, root_url: &Url) -> Result<Url> {
    let body = client.get(root_url.clone()).send().await?.text().await?;
    let parsed = m3u8_rs::parse_playlist_res(body.as_bytes())
        .map_err(|_| LokiDmError::Message("invalid m3u8 playlist".to_owned()))?;

    match parsed {
        Playlist::MediaPlaylist(_) => Ok(root_url.clone()),
        Playlist::MasterPlaylist(master) => {
            let Some(best) = master.variants.iter().max_by_key(|v| v.bandwidth) else {
                return Err(LokiDmError::Message(
                    "master playlist does not contain variants".to_owned(),
                ));
            };
            Ok(root_url.join(&best.uri)?)
        }
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::is_hls_url;

    #[test]
    fn detects_hls_urls() {
        assert!(is_hls_url("https://a/b/index.m3u8"));
        assert!(!is_hls_url("https://a/b/file.mp4"));
    }
}
