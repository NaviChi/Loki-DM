#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    clippy::large_enum_variant,
    clippy::map_unwrap_or,
    clippy::missing_errors_doc,
    clippy::must_use_candidate,
    clippy::redundant_closure_for_method_calls,
    clippy::struct_excessive_bools,
    clippy::too_many_lines
)]
#![cfg_attr(test, allow(clippy::expect_used, clippy::unwrap_used))]

use std::collections::BTreeSet;
use std::fs;
use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use anyhow::{Context, Result};
use clap::{Args, CommandFactory, Parser, Subcommand};
use loki_dm_core::media::{download_media, probe_media};
use loki_dm_core::native_messaging::{
    NativeHostManifestSpec, NativeRequestAction, NativeResponse, chromium_manifest,
    collect_native_host_diagnostics, firefox_manifest, install_manifests, read_native_message,
    uninstall_manifests, validate_installation, write_manifest_pair, write_native_message,
};
use loki_dm_core::spider::{SpiderConfig, collect_urls, crawl};
use loki_dm_core::updater::{
    check_for_updates, download_release_asset, launch_installer, select_asset_for_current_platform,
};
use loki_dm_core::{
    AppSettings, AuthConfig, CHROME_EXTENSION_ID_PLACEHOLDER, DEFAULT_FIREFOX_EXTENSION_ID,
    DEFAULT_NATIVE_HOST_NAME, DownloadConfig, DownloadEngine, DownloadEvent, DownloadScheduler,
    EngineSettings, LokiDmError, NativeHostInstallReport, NativeHostValidationReport, ProxyConfig,
    ProxyKind, QueueAddOutcome, QueueItemStatus, QueuePriority, QueueState, ScheduleSpec,
    classify_url, default_category_rules, merge_cookie_sources, render_cookie_header,
};
use tracing::{error, info};
use url::Url;

#[derive(Debug, Parser)]
#[command(name = "loki-dm", version, about = "Loki DM CLI")]
struct Cli {
    #[arg(long)]
    global_speed_limit_bps: Option<u64>,
    #[arg(long, default_value_t = 30)]
    timeout_secs: u64,
    #[arg(long)]
    settings_path: Option<PathBuf>,
    #[arg(long)]
    queue_path: Option<PathBuf>,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    Download(DownloadCommand),
    Batch {
        input: PathBuf,
        #[command(flatten)]
        options: DownloadOptions,
    },
    Queue {
        #[command(subcommand)]
        command: QueueCommand,
    },
    Settings {
        #[command(subcommand)]
        command: SettingsCommand,
    },
    Spider {
        url: String,
        #[arg(long, default_value_t = 2)]
        depth: usize,
        #[arg(long)]
        ext: Vec<String>,
        #[arg(long, default_value_t = true)]
        same_host: bool,
        #[arg(long, default_value_t = true)]
        respect_robots: bool,
        #[arg(long)]
        download: bool,
        #[arg(long)]
        output_dir: Option<PathBuf>,
        #[arg(long, default_value_t = 8)]
        connections: u16,
    },
    DownloadAll {
        page_url: String,
        #[arg(long)]
        output_dir: Option<PathBuf>,
        #[arg(long, default_value_t = 8)]
        connections: u16,
    },
    Media {
        #[command(subcommand)]
        command: MediaCommand,
    },
    Schedule {
        #[command(subcommand)]
        command: ScheduleCommand,
    },
    Update {
        #[command(subcommand)]
        command: UpdateCommand,
    },
    NativeHost(NativeHostCommand),
    NativeManifests(NativeManifestsCommand),
}

#[derive(Debug, Args)]
struct DownloadCommand {
    url: String,
    #[command(flatten)]
    options: DownloadOptions,
}

#[derive(Debug, Clone, Args)]
struct DownloadOptions {
    #[arg(short, long)]
    output: Option<PathBuf>,
    #[arg(long, default_value_t = 8)]
    connections: u16,
    #[arg(long, default_value_t = 16)]
    max_connections: u16,
    #[arg(long, default_value_t = 2)]
    min_connections: u16,
    #[arg(long)]
    speed_limit_bps: Option<u64>,
    #[arg(long)]
    overwrite: bool,
    #[arg(long)]
    hour_quota_mb: Option<u64>,
    #[arg(long)]
    user_agent: Option<String>,
    #[arg(long)]
    header: Vec<String>,
    #[arg(long)]
    cookie: Vec<String>,
    #[arg(long)]
    cookie_file: Option<PathBuf>,
    #[arg(long)]
    mirror: Vec<String>,
    #[arg(long)]
    av_hook: Option<String>,
    #[arg(long)]
    category: Option<String>,
    #[arg(long)]
    proxy_kind: Option<String>,
    #[arg(long)]
    proxy_host: Option<String>,
    #[arg(long)]
    proxy_port: Option<u16>,
    #[arg(long)]
    proxy_username: Option<String>,
    #[arg(long)]
    proxy_password: Option<String>,
    #[arg(long)]
    basic_user: Option<String>,
    #[arg(long)]
    basic_password: Option<String>,
    #[arg(long)]
    bearer_token: Option<String>,
    #[arg(long)]
    use_ntlm: bool,
    #[arg(long)]
    use_kerberos: bool,
}

#[derive(Debug, Subcommand)]
enum QueueCommand {
    Add {
        url: String,
        #[arg(long, default_value = "normal")]
        priority: String,
        #[arg(long)]
        allow_duplicates: bool,
        #[command(flatten)]
        options: DownloadOptions,
    },
    AddText {
        input: PathBuf,
        #[arg(long)]
        output_dir: Option<PathBuf>,
        #[arg(long, default_value_t = 8)]
        connections: u16,
        #[arg(long, default_value = "normal")]
        priority: String,
        #[arg(long)]
        allow_duplicates: bool,
    },
    List,
    Remove {
        id: u64,
    },
    SetPriority {
        id: u64,
        #[arg(long)]
        priority: String,
    },
    Run {
        #[arg(long)]
        id: Option<u64>,
    },
    Import {
        path: PathBuf,
    },
    Export {
        path: PathBuf,
    },
}

#[derive(Debug, Subcommand)]
enum SettingsCommand {
    Show,
    Export { path: PathBuf },
    Import { path: PathBuf },
    Reset,
}

#[derive(Debug, Subcommand)]
enum MediaCommand {
    Probe {
        url: String,
    },
    Download {
        url: String,
        #[arg(long)]
        format_id: Option<String>,
        #[arg(short, long)]
        output: PathBuf,
    },
}

#[derive(Debug, Subcommand)]
enum ScheduleCommand {
    Once {
        url: String,
        #[arg(long)]
        output: Option<PathBuf>,
        #[arg(long)]
        delay_secs: u64,
        #[arg(long, default_value_t = 8)]
        connections: u16,
    },
    Recurring {
        url: String,
        #[arg(long)]
        output: Option<PathBuf>,
        #[arg(long)]
        start_in_secs: u64,
        #[arg(long)]
        every_secs: u64,
        #[arg(long, default_value_t = 8)]
        connections: u16,
    },
}

#[derive(Debug, Subcommand)]
enum UpdateCommand {
    Check {
        #[arg(long)]
        endpoint: Option<String>,
    },
    Install {
        #[arg(long)]
        endpoint: Option<String>,
        #[arg(long)]
        asset_name: Option<String>,
        #[arg(long)]
        output_dir: Option<PathBuf>,
        #[arg(long, default_value_t = true)]
        launch: bool,
    },
}

#[derive(Debug, Args)]
struct NativeHostCommand {
    #[arg(short, long)]
    output_dir: Option<PathBuf>,
    #[arg(long)]
    start_immediately: bool,
}

#[derive(Debug, Args)]
struct NativeManifestsCommand {
    #[arg(short, long)]
    output_dir: Option<PathBuf>,
    #[arg(long)]
    binary_path: Option<PathBuf>,
    #[arg(long, default_value = "com.loki.dm")]
    host_name: String,
    #[arg(long)]
    chrome_extension_id: Option<String>,
    #[arg(long, default_value = "loki-dm@example.org")]
    firefox_extension_id: String,
    #[arg(long)]
    install: bool,
    #[arg(long)]
    verify: bool,
    #[arg(long, conflicts_with = "install")]
    uninstall: bool,
    #[arg(long)]
    report_json: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter("loki_dm=info,loki_dm_core=info")
        .init();

    let Cli {
        global_speed_limit_bps,
        timeout_secs,
        settings_path,
        queue_path,
        command,
    } = Cli::parse();

    let command = match command {
        Some(command) => command,
        None if !std::io::stdin().is_terminal() => Command::NativeHost(NativeHostCommand {
            output_dir: None,
            start_immediately: false,
        }),
        None => {
            Cli::command().print_help()?;
            println!();
            return Ok(());
        }
    };

    let mut settings = AppSettings::load(settings_path.as_deref())?;
    let mut queue = QueueState::load(queue_path.as_deref())?;

    if let Some(limit) = global_speed_limit_bps {
        settings.connection.global_speed_limit_bps = Some(limit);
    }

    let engine = DownloadEngine::new(EngineSettings {
        global_speed_limit_bps: settings.connection.global_speed_limit_bps,
        request_timeout_secs: timeout_secs,
        ..EngineSettings::default()
    })
    .context("failed to initialize download engine")?;

    match command {
        Command::Download(command) => {
            let cfg = build_download_config(&command.url, &command.options, &settings)?;
            run_download(&engine, cfg).await?;
        }
        Command::Batch { input, options } => {
            let body = fs::read_to_string(&input)
                .with_context(|| format!("failed to read batch file: {}", input.display()))?;
            for line in loki_dm_core::urls_from_text(&body) {
                let cfg = build_download_config(&line, &options, &settings)?;
                run_download(&engine, cfg).await?;
            }
        }
        Command::Queue { command } => {
            handle_queue_command(command, &engine, &settings, &mut queue).await?;
            queue.save(queue_path.as_deref())?;
        }
        Command::Settings { command } => {
            handle_settings_command(command, &mut settings, settings_path.as_deref())?;
        }
        Command::Spider {
            url,
            depth,
            ext,
            same_host,
            respect_robots,
            download,
            output_dir,
            connections,
        } => {
            run_spider(
                &engine,
                &settings,
                SpiderInput {
                    url,
                    depth,
                    ext,
                    same_host,
                    respect_robots,
                    download,
                    output_dir,
                    connections,
                },
            )
            .await?;
        }
        Command::DownloadAll {
            page_url,
            output_dir,
            connections,
        } => {
            run_spider(
                &engine,
                &settings,
                SpiderInput {
                    url: page_url,
                    depth: 1,
                    ext: Vec::new(),
                    same_host: true,
                    respect_robots: true,
                    download: true,
                    output_dir,
                    connections,
                },
            )
            .await?;
        }
        Command::Media { command } => match command {
            MediaCommand::Probe { url } => {
                let probe = probe_media(&url).await?;
                println!(
                    "title={:?} source={:?} extractor={:?} formats={}",
                    probe.title,
                    probe.source,
                    probe.extractor,
                    probe.formats.len()
                );
                for format in probe.formats {
                    println!(
                        "- id={} quality={:?} resolution={:?} ext={:?} audio_only={} size={:?}",
                        format.id,
                        format.quality,
                        format.resolution,
                        format.ext,
                        format.audio_only,
                        format.filesize
                    );
                }
            }
            MediaCommand::Download {
                url,
                format_id,
                output,
            } => {
                download_media(&url, format_id.as_deref(), &output).await?;
                println!("media saved: {}", output.display());
            }
        },
        Command::Schedule { command } => {
            run_schedule_command(&engine, &settings, command).await?;
        }
        Command::Update { command } => {
            run_update_command(&settings, command).await?;
        }
        Command::NativeHost(command) => {
            native_host_loop(&engine, &settings, queue_path.as_deref(), command).await?;
        }
        Command::NativeManifests(command) => {
            run_native_manifest_command(command)?;
        }
    }

    Ok(())
}

async fn handle_queue_command(
    command: QueueCommand,
    engine: &DownloadEngine,
    settings: &AppSettings,
    queue: &mut QueueState,
) -> Result<()> {
    match command {
        QueueCommand::Add {
            url,
            priority,
            allow_duplicates,
            options,
        } => {
            let cfg = build_download_config(&url, &options, settings)?;
            let priority = parse_queue_priority(&priority)?;
            let prevent_duplicates =
                settings.advanced.prevent_duplicate_queue_entries && !allow_duplicates;
            match queue.add_download_dedup(cfg, priority, prevent_duplicates) {
                QueueAddOutcome::Added { id } => {
                    println!("queued #{id}: {url} (priority={})", priority.as_str());
                }
                QueueAddOutcome::Duplicate { existing_id } => {
                    println!("skipped duplicate URL (already queued as #{existing_id})");
                }
            }
        }
        QueueCommand::AddText {
            input,
            output_dir,
            connections,
            priority,
            allow_duplicates,
        } => {
            let body = fs::read_to_string(&input)
                .with_context(|| format!("failed to read {}", input.display()))?;
            let priority = parse_queue_priority(&priority)?;
            let prevent_duplicates =
                settings.advanced.prevent_duplicate_queue_entries && !allow_duplicates;
            let mut ids = Vec::new();
            let mut duplicates = 0_usize;
            for url in loki_dm_core::urls_from_text(&body) {
                let category = classify_url(&url, &queue.rules);
                let category_name = category.as_str().to_owned();
                let output_root = output_dir
                    .clone()
                    .unwrap_or_else(|| settings.category_output_dir(Some(&category_name)));
                let cfg = DownloadConfig {
                    url: url.clone(),
                    mirror_urls: Vec::new(),
                    output_path: output_root.join(default_filename(&url)),
                    initial_connections: connections,
                    min_connections: settings.connection.min_connections,
                    max_connections: settings.connection.max_connections,
                    min_segment_size: settings.connection.min_segment_size,
                    max_retries: settings.advanced.retry_count,
                    speed_limit_bps: settings.connection.default_download_speed_limit_bps,
                    hour_quota_mb: settings.connection.default_hour_quota_mb,
                    overwrite: false,
                    headers: std::collections::BTreeMap::new(),
                    proxy: if settings.proxy.enabled {
                        settings.proxy.proxy.clone()
                    } else {
                        None
                    },
                    auth: None,
                    user_agent: None,
                    category: Some(category_name),
                    av_hook_command: settings.advanced.av_hook_command.clone(),
                };
                match queue.add_download_dedup(cfg, priority, prevent_duplicates) {
                    QueueAddOutcome::Added { id } => ids.push(id),
                    QueueAddOutcome::Duplicate { .. } => duplicates = duplicates.saturating_add(1),
                }
            }
            println!(
                "queued {} URLs (duplicates skipped: {duplicates}, priority={})",
                ids.len(),
                priority.as_str()
            );
        }
        QueueCommand::List => {
            for item in &queue.items {
                println!(
                    "#{:>4} {:<10} {:<9} {:<11} {} -> {}",
                    item.id,
                    item.category.as_str(),
                    item.priority.as_str(),
                    format!("{:?}", item.status),
                    item.config.url,
                    item.config.output_path.display()
                );
            }
        }
        QueueCommand::Remove { id } => {
            if queue.remove(id) {
                println!("removed queue item #{id}");
            } else {
                println!("queue item #{id} not found");
            }
        }
        QueueCommand::SetPriority { id, priority } => {
            let parsed = parse_queue_priority(&priority)?;
            if queue.set_priority(id, parsed) {
                println!("queue item #{id} priority set to {}", parsed.as_str());
            } else {
                println!("queue item #{id} not found");
            }
        }
        QueueCommand::Run { id } => {
            let candidates: Vec<u64> = if let Some(id) = id {
                vec![id]
            } else {
                queue.pending_items().iter().map(|item| item.id).collect()
            };

            for item_id in candidates {
                let maybe_cfg = queue
                    .items
                    .iter()
                    .find(|item| item.id == item_id)
                    .map(|item| item.config.clone());
                let Some(cfg) = maybe_cfg else {
                    continue;
                };

                queue.set_status(item_id, QueueItemStatus::Running, None);
                match run_download(engine, cfg).await {
                    Ok(()) => queue.set_status(item_id, QueueItemStatus::Completed, None),
                    Err(err) => {
                        queue.set_status(item_id, QueueItemStatus::Failed, Some(err.to_string()));
                    }
                }
            }
        }
        QueueCommand::Import { path } => {
            let imported = QueueState::load(Some(&path))?;
            *queue = imported;
            println!("imported queue from {}", path.display());
        }
        QueueCommand::Export { path } => {
            queue.save(Some(&path))?;
            println!("exported queue to {}", path.display());
        }
    }

    Ok(())
}

fn handle_settings_command(
    command: SettingsCommand,
    settings: &mut AppSettings,
    settings_path: Option<&Path>,
) -> Result<()> {
    match command {
        SettingsCommand::Show => {
            let rendered = toml::to_string_pretty(settings)?;
            println!("{rendered}");
        }
        SettingsCommand::Export { path } => {
            settings.save(Some(&path))?;
            println!("exported settings to {}", path.display());
        }
        SettingsCommand::Import { path } => {
            *settings = AppSettings::load(Some(&path))?;
            settings.save(settings_path)?;
            println!("imported settings from {}", path.display());
        }
        SettingsCommand::Reset => {
            *settings = AppSettings::default();
            let out = settings.save(settings_path)?;
            println!("settings reset: {}", out.display());
        }
    }

    Ok(())
}

struct SpiderInput {
    url: String,
    depth: usize,
    ext: Vec<String>,
    same_host: bool,
    respect_robots: bool,
    download: bool,
    output_dir: Option<PathBuf>,
    connections: u16,
}

async fn run_spider(
    engine: &DownloadEngine,
    settings: &AppSettings,
    input: SpiderInput,
) -> Result<()> {
    let client = reqwest::Client::builder().tcp_nodelay(true).build()?;
    let cfg = SpiderConfig {
        root: Url::parse(&input.url)?,
        max_depth: input.depth,
        allowed_extensions: input
            .ext
            .into_iter()
            .map(|ext| ext.trim().trim_start_matches('.').to_ascii_lowercase())
            .filter(|ext| !ext.is_empty())
            .collect::<BTreeSet<_>>(),
        same_host_only: input.same_host,
        respect_robots: input.respect_robots,
        allowed_schemes: BTreeSet::new(),
    };

    let hits = crawl(&client, &cfg).await?;
    let urls = collect_urls(&hits);
    println!("found {} links", urls.len());
    for hit in &hits {
        println!("depth={} {}", hit.depth, hit.url);
    }

    if input.download {
        let options = DownloadOptions {
            output: None,
            connections: input.connections,
            max_connections: settings.connection.max_connections,
            min_connections: settings.connection.min_connections,
            speed_limit_bps: settings.connection.default_download_speed_limit_bps,
            overwrite: false,
            hour_quota_mb: settings.connection.default_hour_quota_mb,
            user_agent: None,
            header: Vec::new(),
            cookie: Vec::new(),
            cookie_file: None,
            mirror: Vec::new(),
            av_hook: settings.advanced.av_hook_command.clone(),
            category: None,
            proxy_kind: None,
            proxy_host: None,
            proxy_port: None,
            proxy_username: None,
            proxy_password: None,
            basic_user: None,
            basic_password: None,
            bearer_token: None,
            use_ntlm: false,
            use_kerberos: false,
        };

        for url in urls {
            let mut per_download = options.clone();
            if let Some(base) = &input.output_dir {
                per_download.output = Some(base.join(default_filename(url.as_str())));
            }

            let cfg = build_download_config(url.as_str(), &per_download, settings)?;
            let _ = run_download(engine, cfg).await;
        }
    }

    Ok(())
}

async fn run_schedule_command(
    engine: &DownloadEngine,
    settings: &AppSettings,
    command: ScheduleCommand,
) -> Result<()> {
    let scheduler = DownloadScheduler::new();

    match command {
        ScheduleCommand::Once {
            url,
            output,
            delay_secs,
            connections,
        } => {
            let options = DownloadOptions {
                output,
                connections,
                max_connections: settings.connection.max_connections,
                min_connections: settings.connection.min_connections,
                speed_limit_bps: settings.connection.default_download_speed_limit_bps,
                overwrite: false,
                hour_quota_mb: settings.connection.default_hour_quota_mb,
                user_agent: None,
                header: Vec::new(),
                cookie: Vec::new(),
                cookie_file: None,
                mirror: Vec::new(),
                av_hook: settings.advanced.av_hook_command.clone(),
                category: None,
                proxy_kind: None,
                proxy_host: None,
                proxy_port: None,
                proxy_username: None,
                proxy_password: None,
                basic_user: None,
                basic_password: None,
                bearer_token: None,
                use_ntlm: false,
                use_kerberos: false,
            };
            let cfg = build_download_config(&url, &options, settings)?;
            let spec = ScheduleSpec {
                start_at: SystemTime::now() + Duration::from_secs(delay_secs),
                interval: None,
            };
            let id = scheduler.add_job("once", spec, cfg).await;
            println!("scheduled one-time job #{id}, waiting for execution");
        }
        ScheduleCommand::Recurring {
            url,
            output,
            start_in_secs,
            every_secs,
            connections,
        } => {
            let options = DownloadOptions {
                output,
                connections,
                max_connections: settings.connection.max_connections,
                min_connections: settings.connection.min_connections,
                speed_limit_bps: settings.connection.default_download_speed_limit_bps,
                overwrite: true,
                hour_quota_mb: settings.connection.default_hour_quota_mb,
                user_agent: None,
                header: Vec::new(),
                cookie: Vec::new(),
                cookie_file: None,
                mirror: Vec::new(),
                av_hook: settings.advanced.av_hook_command.clone(),
                category: None,
                proxy_kind: None,
                proxy_host: None,
                proxy_port: None,
                proxy_username: None,
                proxy_password: None,
                basic_user: None,
                basic_password: None,
                bearer_token: None,
                use_ntlm: false,
                use_kerberos: false,
            };
            let cfg = build_download_config(&url, &options, settings)?;
            let spec = ScheduleSpec {
                start_at: SystemTime::now() + Duration::from_secs(start_in_secs),
                interval: Some(Duration::from_secs(every_secs.max(1))),
            };
            let id = scheduler.add_job("recurring", spec, cfg).await;
            println!("scheduled recurring job #{id}, Ctrl+C to stop");
        }
    }

    let runner = scheduler.start(engine.clone());
    tokio::signal::ctrl_c().await.ok();
    scheduler.stop();
    let _ = runner.await;
    Ok(())
}

async fn run_update_command(settings: &AppSettings, command: UpdateCommand) -> Result<()> {
    match command {
        UpdateCommand::Check { endpoint } => {
            let endpoint = endpoint.unwrap_or_else(|| settings.advanced.update_endpoint.clone());
            let Some(info) = fetch_update_info(&endpoint).await? else {
                return Ok(());
            };
            println!(
                "current={} latest={} update_available={}",
                info.current_version, info.latest_version, info.update_available
            );
            println!(
                "release={:?} published_at={:?}",
                info.release_name, info.published_at
            );
            for asset in info.assets {
                println!(
                    "- asset={} size={:?} url={}",
                    asset.name, asset.size, asset.download_url
                );
            }
        }
        UpdateCommand::Install {
            endpoint,
            asset_name,
            output_dir,
            launch,
        } => {
            let endpoint = endpoint.unwrap_or_else(|| settings.advanced.update_endpoint.clone());
            let Some(info) = fetch_update_info(&endpoint).await? else {
                return Ok(());
            };
            if !info.update_available {
                println!("already up-to-date ({})", info.current_version);
                return Ok(());
            }

            let asset = if let Some(name) = asset_name {
                info.assets
                    .iter()
                    .find(|asset| asset.name == name)
                    .cloned()
                    .ok_or_else(|| anyhow::anyhow!("asset not found: {name}"))?
            } else {
                select_asset_for_current_platform(&info).ok_or_else(|| {
                    anyhow::anyhow!("no release asset available for current platform")
                })?
            };

            let out_dir = output_dir.unwrap_or_else(|| {
                loki_dm_core::settings::app_config_dir()
                    .join("updates")
                    .join(&info.latest_version)
            });
            let downloaded = download_release_asset(&asset, &out_dir).await?;
            println!("downloaded update asset: {}", downloaded.display());

            if launch {
                launch_installer(&downloaded).await?;
                println!("installer launched");
            } else {
                println!("launch skipped (--launch=false)");
            }
        }
    }

    Ok(())
}

async fn fetch_update_info(endpoint: &str) -> Result<Option<loki_dm_core::UpdateInfo>> {
    match check_for_updates(endpoint, env!("CARGO_PKG_VERSION")).await {
        Ok(info) => Ok(Some(info)),
        Err(err) => {
            if is_release_endpoint_missing(&err) {
                println!(
                    "update endpoint has no releases yet: {endpoint}\nconfigure a valid releases URL with `settings set --advanced.update_endpoint ...`"
                );
                Ok(None)
            } else {
                Err(err.into())
            }
        }
    }
}

fn is_release_endpoint_missing(err: &LokiDmError) -> bool {
    match err {
        LokiDmError::Http(http) => http.status() == Some(reqwest::StatusCode::NOT_FOUND),
        LokiDmError::HttpStatus(code) => *code == reqwest::StatusCode::NOT_FOUND,
        _ => false,
    }
}

fn build_download_config(
    url: &str,
    options: &DownloadOptions,
    settings: &AppSettings,
) -> Result<DownloadConfig> {
    let mut headers = std::collections::BTreeMap::new();
    for raw in &options.header {
        let (key, value) = raw
            .split_once(':')
            .ok_or_else(|| anyhow::anyhow!("invalid header format: {raw}"))?;
        headers.insert(key.trim().to_owned(), value.trim().to_owned());
    }

    if !headers.keys().any(|key| key.eq_ignore_ascii_case("cookie")) {
        let cookies = merge_cookie_sources(&options.cookie, options.cookie_file.as_deref())
            .map_err(|err| anyhow::anyhow!(err.to_string()))?;
        if let Some(cookie_header) = render_cookie_header(&cookies) {
            headers.insert("Cookie".to_owned(), cookie_header);
        }
    }

    let inferred_category = options
        .category
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| {
            classify_url(url, &default_category_rules())
                .as_str()
                .to_owned()
        });

    let output = options.output.clone().unwrap_or_else(|| {
        settings
            .category_output_dir(Some(&inferred_category))
            .join(default_filename(url))
    });

    let proxy = match (
        options.proxy_kind.as_deref(),
        options.proxy_host.as_deref(),
        options.proxy_port,
    ) {
        (Some(kind), Some(host), Some(port)) => {
            let kind = match kind.to_ascii_lowercase().as_str() {
                "http" => ProxyKind::Http,
                "https" => ProxyKind::Https,
                "socks5" | "socks" => ProxyKind::Socks5,
                other => return Err(anyhow::anyhow!("unsupported proxy kind: {other}")),
            };
            Some(ProxyConfig {
                kind,
                host: host.to_owned(),
                port,
                username: options.proxy_username.clone(),
                password: options.proxy_password.clone(),
            })
        }
        _ if settings.proxy.enabled => settings.proxy.proxy.clone(),
        _ => None,
    };

    let auth = if options.use_ntlm {
        Some(AuthConfig::Ntlm)
    } else if options.use_kerberos {
        Some(AuthConfig::Kerberos)
    } else if let (Some(user), Some(pass)) = (&options.basic_user, &options.basic_password) {
        Some(AuthConfig::Basic {
            username: user.clone(),
            password: pass.clone(),
        })
    } else {
        options
            .bearer_token
            .as_ref()
            .map(|token| AuthConfig::Bearer {
                token: token.clone(),
            })
    };

    Ok(DownloadConfig {
        url: url.to_owned(),
        mirror_urls: options.mirror.clone(),
        output_path: output,
        initial_connections: options.connections,
        min_connections: options.min_connections,
        max_connections: options.max_connections,
        min_segment_size: settings.connection.min_segment_size,
        max_retries: settings.advanced.retry_count,
        speed_limit_bps: options
            .speed_limit_bps
            .or(settings.connection.default_download_speed_limit_bps),
        hour_quota_mb: options
            .hour_quota_mb
            .or(settings.connection.default_hour_quota_mb),
        overwrite: options.overwrite,
        headers,
        proxy,
        auth,
        user_agent: options.user_agent.clone(),
        category: Some(inferred_category),
        av_hook_command: options
            .av_hook
            .clone()
            .or(settings.advanced.av_hook_command.clone()),
    })
}

async fn run_download(engine: &DownloadEngine, cfg: DownloadConfig) -> Result<()> {
    info!(
        "starting download url={} output={} connections={}..{}",
        cfg.url,
        cfg.output_path.display(),
        cfg.min_connections,
        cfg.max_connections
    );

    let mut handle = engine.start(cfg).context("failed to start download")?;

    loop {
        tokio::select! {
            maybe_event = handle.recv_event() => {
                let Some(event) = maybe_event else {
                    break;
                };

                match event {
                    DownloadEvent::Started { id, total_bytes, resumed, .. } => {
                        println!("[{id}] started, total={total_bytes} bytes, resumed={resumed}");
                    }
                    DownloadEvent::MirrorSelected { id, url, source_count } => {
                        println!("[{id}] mirror selected ({source_count} candidates): {url}");
                    }
                    DownloadEvent::Progress(progress) => {
                        let pct = if progress.total_bytes == 0 {
                            0.0
                        } else {
                            (progress.downloaded_bytes as f64 / progress.total_bytes as f64) * 100.0
                        };
                        let eta = progress
                            .eta_seconds
                            .map_or_else(|| "--".to_owned(), |v| format!("{v}s"));

                        print!(
                            "\r[{}] {:>6.2}% {:>10}/{} bytes | {:>10.0} B/s | ETA {} | conn {}/{}   ",
                            progress.id,
                            pct,
                            progress.downloaded_bytes,
                            progress.total_bytes,
                            progress.speed_bps,
                            eta,
                            progress.active_connections,
                            progress.target_connections,
                        );
                        std::io::stdout().flush().ok();
                    }
                    DownloadEvent::Retrying { segment_id, attempt, wait_ms, reason, .. } => {
                        println!("\nretry segment={segment_id} attempt={attempt} wait={wait_ms}ms reason={reason}");
                    }
                    DownloadEvent::ConnectionsAdjusted { from, to, .. } => {
                        println!("\nconnections adjusted: {from} -> {to}");
                    }
                    DownloadEvent::HookExecuted { command, success, code, stderr, .. } => {
                        println!("\nAV hook command={command} success={success} code={code:?} stderr={stderr}");
                    }
                    DownloadEvent::Completed { output_path, duration_ms, .. } => {
                        println!("\ncompleted: {} in {}ms", output_path.display(), duration_ms);
                        break;
                    }
                    DownloadEvent::Failed { error, .. } => {
                        println!("\nfailed: {error}");
                        break;
                    }
                    DownloadEvent::Cancelled { .. } => {
                        println!("\ncancelled");
                        break;
                    }
                    DownloadEvent::Paused { .. } | DownloadEvent::Resumed { .. } => {}
                }
            }
            sig = tokio::signal::ctrl_c() => {
                if sig.is_ok() {
                    println!("\nreceived Ctrl+C, cancelling");
                    let _ = handle.cancel();
                }
            }
        }
    }

    match handle.wait().await {
        Ok(path) => {
            println!("saved: {}", path.display());
            Ok(())
        }
        Err(LokiDmError::Cancelled) => {
            println!("download cancelled");
            Ok(())
        }
        Err(err) => {
            error!("download failed: {err}");
            Err(anyhow::anyhow!(err.to_string()))
        }
    }
}

async fn native_host_loop(
    engine: &DownloadEngine,
    settings: &AppSettings,
    queue_path: Option<&Path>,
    command: NativeHostCommand,
) -> Result<()> {
    let mut stdin = std::io::stdin();
    let mut stdout = std::io::stdout();

    loop {
        let maybe_req =
            read_native_message(&mut stdin).context("failed to read native messaging request")?;
        let Some(req) = maybe_req else {
            break;
        };

        let response =
            match handle_native_request(engine, settings, queue_path, &command, req).await {
                Ok(response) => response,
                Err(err) => NativeResponse {
                    ok: false,
                    message: err.to_string(),
                    output_path: None,
                    queue_ids: Vec::new(),
                },
            };

        write_native_message(&mut stdout, &response)
            .context("failed to write native messaging response")?;
    }

    Ok(())
}

async fn handle_native_request(
    engine: &DownloadEngine,
    settings: &AppSettings,
    queue_path: Option<&Path>,
    command: &NativeHostCommand,
    request: loki_dm_core::NativeRequest,
) -> Result<NativeResponse> {
    if request.action == NativeRequestAction::Ping {
        return Ok(NativeResponse {
            ok: true,
            message: "pong".to_owned(),
            output_path: None,
            queue_ids: Vec::new(),
        });
    }

    let mut urls = request.urls;
    if let Some(url) = request
        .url
        .as_deref()
        .map(str::trim)
        .filter(|url| !url.is_empty())
    {
        urls.push(url.to_owned());
    }
    urls.sort_unstable();
    urls.dedup();

    if urls.is_empty() {
        return Ok(NativeResponse {
            ok: false,
            message: "native request must include at least one URL".to_owned(),
            output_path: None,
            queue_ids: Vec::new(),
        });
    }

    let output_root_override = request
        .output_dir
        .as_deref()
        .map(PathBuf::from)
        .or_else(|| command.output_dir.clone());
    let custom_output = request.output.as_deref().map(PathBuf::from);
    if custom_output.is_some() && urls.len() > 1 {
        return Ok(NativeResponse {
            ok: false,
            message: "native request cannot combine `output` with multiple URLs".to_owned(),
            output_path: None,
            queue_ids: Vec::new(),
        });
    }

    let initial_connections = request
        .connections
        .unwrap_or(settings.connection.initial_connections)
        .max(1);
    let min_connections = settings
        .connection
        .min_connections
        .min(initial_connections)
        .max(1);
    let max_connections = settings.connection.max_connections.max(initial_connections);
    let requested_priority = request
        .priority
        .as_deref()
        .and_then(QueuePriority::parse)
        .unwrap_or(QueuePriority::Normal);

    let immediate = command.start_immediately || request.action == NativeRequestAction::Download;

    if immediate {
        let mut first_output = None;
        for url in urls {
            let category = classify_url(&url, &default_category_rules());
            let category_name = category.as_str().to_owned();
            let output_root = output_root_override
                .clone()
                .unwrap_or_else(|| settings.category_output_dir(Some(&category_name)));
            let output_path = custom_output
                .clone()
                .unwrap_or_else(|| output_root.join(default_filename(&url)));
            let cfg = DownloadConfig {
                url,
                mirror_urls: Vec::new(),
                output_path: output_path.clone(),
                initial_connections,
                min_connections,
                max_connections,
                min_segment_size: settings.connection.min_segment_size,
                max_retries: settings.advanced.retry_count,
                speed_limit_bps: request
                    .speed_limit_bps
                    .or(settings.connection.default_download_speed_limit_bps),
                hour_quota_mb: settings.connection.default_hour_quota_mb,
                overwrite: false,
                headers: std::collections::BTreeMap::new(),
                proxy: if settings.proxy.enabled {
                    settings.proxy.proxy.clone()
                } else {
                    None
                },
                auth: None,
                user_agent: None,
                category: Some(category_name),
                av_hook_command: settings.advanced.av_hook_command.clone(),
            };
            run_download(engine, cfg).await?;
            if first_output.is_none() {
                first_output = Some(output_path.display().to_string());
            }
        }

        Ok(NativeResponse {
            ok: true,
            message: "download complete".to_owned(),
            output_path: first_output,
            queue_ids: Vec::new(),
        })
    } else {
        let mut queue = QueueState::load(queue_path)?;
        let mut queue_ids = Vec::new();
        let mut first_output = None;
        let mut added_count = 0_usize;
        let mut duplicate_count = 0_usize;

        for url in urls {
            let category = classify_url(&url, &default_category_rules());
            let category_name = category.as_str().to_owned();
            let output_root = output_root_override
                .clone()
                .unwrap_or_else(|| settings.category_output_dir(Some(&category_name)));
            let output_path = custom_output
                .clone()
                .unwrap_or_else(|| output_root.join(default_filename(&url)));

            let cfg = DownloadConfig {
                url,
                mirror_urls: Vec::new(),
                output_path: output_path.clone(),
                initial_connections,
                min_connections,
                max_connections,
                min_segment_size: settings.connection.min_segment_size,
                max_retries: settings.advanced.retry_count,
                speed_limit_bps: request
                    .speed_limit_bps
                    .or(settings.connection.default_download_speed_limit_bps),
                hour_quota_mb: settings.connection.default_hour_quota_mb,
                overwrite: false,
                headers: std::collections::BTreeMap::new(),
                proxy: if settings.proxy.enabled {
                    settings.proxy.proxy.clone()
                } else {
                    None
                },
                auth: None,
                user_agent: None,
                category: Some(category_name),
                av_hook_command: settings.advanced.av_hook_command.clone(),
            };

            if first_output.is_none() {
                first_output = Some(output_path.display().to_string());
            }
            match queue.add_download_dedup(
                cfg,
                requested_priority,
                settings.advanced.prevent_duplicate_queue_entries,
            ) {
                QueueAddOutcome::Added { id } => {
                    queue_ids.push(id);
                    added_count = added_count.saturating_add(1);
                }
                QueueAddOutcome::Duplicate { existing_id } => {
                    queue_ids.push(existing_id);
                    duplicate_count = duplicate_count.saturating_add(1);
                }
            }
        }

        let written = queue.save(queue_path)?;
        Ok(NativeResponse {
            ok: true,
            message: format!(
                "queued {} download(s) in {} (duplicates: {}, priority={})",
                added_count,
                written.display(),
                duplicate_count,
                requested_priority.as_str()
            ),
            output_path: first_output,
            queue_ids,
        })
    }
}

fn run_native_manifest_command(command: NativeManifestsCommand) -> Result<()> {
    let binary_path = if let Some(path) = command.binary_path {
        if path.is_absolute() {
            path
        } else {
            std::env::current_dir()?.join(path)
        }
    } else {
        std::env::current_exe().context("failed to resolve current executable path")?
    };

    let mut spec = NativeHostManifestSpec::with_binary_path(binary_path);
    spec.host_name = if command.host_name.trim().is_empty() {
        DEFAULT_NATIVE_HOST_NAME.to_owned()
    } else {
        command.host_name.trim().to_owned()
    };
    spec.chrome_extension_id = command
        .chrome_extension_id
        .unwrap_or_else(|| CHROME_EXTENSION_ID_PLACEHOLDER.to_owned());
    spec.firefox_extension_id = if command.firefox_extension_id.trim().is_empty() {
        DEFAULT_FIREFOX_EXTENSION_ID.to_owned()
    } else {
        command.firefox_extension_id.trim().to_owned()
    };

    let should_generate = command.install || (!command.verify && !command.uninstall);
    if command.install && !spec.binary_path.is_file() {
        return Err(anyhow::anyhow!(
            "binary path does not exist or is not a file: {}",
            spec.binary_path.display()
        ));
    }

    if should_generate {
        let output_dir = command
            .output_dir
            .clone()
            .unwrap_or_else(|| PathBuf::from("extensions/native-host"));
        let (chrome_path, firefox_path) = write_manifest_pair(&output_dir, &spec)?;
        println!("generated chromium manifest: {}", chrome_path.display());
        println!("generated firefox manifest: {}", firefox_path.display());

        let chrome = chromium_manifest(&spec);
        if let Some(origin) = chrome
            .get("allowed_origins")
            .and_then(|v| v.as_array())
            .and_then(|list| list.first())
            .and_then(serde_json::Value::as_str)
        {
            println!("chromium allowed origin: {origin}");
        }

        let firefox = firefox_manifest(&spec);
        if let Some(ext) = firefox
            .get("allowed_extensions")
            .and_then(|v| v.as_array())
            .and_then(|list| list.first())
            .and_then(serde_json::Value::as_str)
        {
            println!("firefox allowed extension: {ext}");
        }
    }

    if command.install {
        let report = install_manifests(&spec)?;
        print_install_report("install", &report);
    }

    if command.uninstall {
        let report = uninstall_manifests(&spec)?;
        print_install_report("uninstall", &report);
    }

    if command.verify {
        let report = validate_installation(&spec)?;
        print_validation_report(&report);
    }

    if !command.install && spec.chrome_extension_id == CHROME_EXTENSION_ID_PLACEHOLDER {
        println!(
            "warning: set --chrome-extension-id to your real unpacked extension id before use"
        );
    }

    if command.report_json {
        let diagnostics = collect_native_host_diagnostics(&spec, command.output_dir.as_deref())?;
        let rendered = serde_json::to_string_pretty(&diagnostics)?;
        println!("{rendered}");
    }

    Ok(())
}

fn print_install_report(action: &str, report: &NativeHostInstallReport) {
    if !report.manifest_files_written.is_empty() {
        for path in &report.manifest_files_written {
            println!("{action}: wrote manifest {}", path.display());
        }
    }
    if !report.registry_entries_written.is_empty() {
        for key in &report.registry_entries_written {
            println!("{action}: wrote registry key {key}");
        }
    }
    if !report.manifest_files_removed.is_empty() {
        for path in &report.manifest_files_removed {
            println!("{action}: removed manifest {}", path.display());
        }
    }
    if !report.registry_entries_removed.is_empty() {
        for key in &report.registry_entries_removed {
            println!("{action}: removed registry key {key}");
        }
    }

    if report.manifest_files_written.is_empty()
        && report.registry_entries_written.is_empty()
        && report.manifest_files_removed.is_empty()
        && report.registry_entries_removed.is_empty()
    {
        println!("{action}: no changes");
    }

    for warning in &report.warnings {
        println!("{action}: warning: {warning}");
    }
}

fn print_validation_report(report: &NativeHostValidationReport) {
    for path in &report.manifest_files_present {
        println!("verify: manifest present {}", path.display());
    }
    for path in &report.manifest_files_missing {
        println!("verify: manifest missing {}", path.display());
    }
    for key in &report.registry_entries_present {
        println!("verify: registry key present {key}");
    }
    for key in &report.registry_entries_missing {
        println!("verify: registry key missing {key}");
    }

    if report.manifest_files_missing.is_empty() && report.registry_entries_missing.is_empty() {
        println!("verify: native host installation looks complete");
    } else {
        println!("verify: installation incomplete");
    }
}

fn default_filename(url: &str) -> String {
    let parsed = Url::parse(url).ok();
    parsed
        .and_then(|u| {
            u.path_segments()
                .and_then(|mut segs| segs.next_back())
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .map(ToOwned::to_owned)
        })
        .unwrap_or_else(|| "download.bin".to_owned())
}

fn parse_queue_priority(raw: &str) -> Result<QueuePriority> {
    QueuePriority::parse(raw).ok_or_else(|| {
        anyhow::anyhow!("invalid priority `{raw}` (expected one of: low, normal, high, critical)")
    })
}
