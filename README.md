# Loki DM

**Loki DM – Loki Download Manager**

Native Rust download manager inspired by IDM, optimized for segmented/resumable downloads, queue automation, browser handoff, and cross-platform desktop operation.

[![Release](https://img.shields.io/github/v/release/USERNAME/Loki-DM?label=latest%20release)](https://github.com/USERNAME/Loki-DM/releases/latest)
[![Download](https://img.shields.io/badge/Download-Latest%20Release-1f6feb)](https://github.com/USERNAME/Loki-DM/releases/latest)
[![CI](https://img.shields.io/github/actions/workflow/status/USERNAME/Loki-DM/ci.yml?branch=main)](https://github.com/USERNAME/Loki-DM/actions/workflows/ci.yml)

License: **MIT**.

## Current Feature Coverage

- Core engine:
  - Segmented byte-range download (`Range`), parallel connections, dynamic segment splitting
  - Adaptive connection target (`min..max`) based on measured throughput
  - Automatic resume with `.lokidm` sidecar + `.part` file
  - Global and per-download speed limiter
  - Hourly traffic quota per download
  - Retry with exponential backoff + jitter
  - Mirror selection (`--mirror`) with latency/range-awareness
  - AV hook execution on completion (`{path}` / `{filename}` placeholders)
  - Optional queue-completion command hook (for shutdown/sleep/script automation)
- Protocols:
  - HTTP/1.1 + HTTP/2 + HTTPS via `reqwest`
  - SOCKS5/HTTP/HTTPS proxy support
  - FTP/FTPS via native Rust `curl` crate backend (libcurl bindings)
  - NTLM/Kerberos auth path via native Rust `curl` crate backend
  - Cookie-aware downloads via `--cookie` and `--cookie-file` (Netscape jar support)
- Queue and categorization:
  - Persistent queue (JSON/TOML)
  - Default categories + extension/site rule matching
  - Category-specific output folders with editable paths in GUI settings
  - Queue priorities (Low/Normal/High/Critical) and priority-ordered execution
  - Duplicate URL prevention for queued items (toggle in Advanced settings / Queue panel)
  - Queue concurrency control (run N queue downloads at once)
  - Batch add from text and queue run controls
- Scheduler:
  - One-shot and recurring scheduled downloads
  - Runtime scheduler service in both CLI and GUI
- Spider / Site grabber:
  - Recursive crawl with depth, extension filters, host constraints
  - robots.txt-aware mode
  - Download-all links flow
- Media workflow:
  - `yt-dlp` probe/download integration (YouTube/Vimeo/Twitch compatible via yt-dlp)
  - Native HLS `.m3u8` probe/download path in Rust
- Desktop GUI:
  - Download list with live progress/speed/ETA
  - Pause/resume/cancel, context actions, category filter, search, sorting
  - Queue panel (run/remove/persist)
  - Scheduler panel (add/enable/disable/remove jobs)
  - Settings dialog tabs: Connection, Proxy, Scheduler, Browser, Appearance, Advanced
  - Per-download and global speed charts
  - Clipboard URL import and dropped file/text ingestion
  - Native tray menu (macOS/Windows): show/pause all/resume all/quit
  - In-app update check and installer launch flow
- Browser integration:
  - Chromium/Firefox extension + native messaging host manifests
  - CLI + GUI native-host setup actions (generate/install/validate/uninstall)
  - Windows HKCU native-host registry automation (Chrome/Chromium/Edge/Brave/Firefox)
  - Context menu + floating video panel + quick-send popup

## UI Comparison Gallery

- Loki DM Dark (release build): `assets/screenshots/loki-dm-dark.png`
- Loki DM Light (release build): `assets/screenshots/loki-dm-light.png`
- Reference captures from IDM are documented in the visual fidelity report section below.

Visual verification process:
- Capture release-binary screenshots (`target/release/loki-dm-gui`) for dark and light themes.
- Compare menu/toolbar ordering, category tree layout, table headers, and status/footer regions against IDM references.
- Iterate styling and spacing until component-level similarity is within acceptable tolerance for this release cycle.

## Workspace Layout

```text
Loki_DownloadManager/
├── Cargo.toml
├── README.md
├── LICENSE
├── rust-toolchain.toml
├── assets/
│   └── screenshots/
│       ├── loki-dm-dark.png
│       └── loki-dm-light.png
├── .github/workflows/
│   ├── ci.yml
│   └── release.yml
├── crates/
│   ├── loki-dm-core/
│   │   ├── src/
│   │   │   ├── av.rs
│   │   │   ├── config.rs
│   │   │   ├── cookies.rs
│   │   │   ├── engine.rs
│   │   │   ├── error.rs
│   │   │   ├── external_downloader.rs
│   │   │   ├── lib.rs
│   │   │   ├── media.rs
│   │   │   ├── metadata.rs
│   │   │   ├── native_messaging.rs
│   │   │   ├── queue.rs
│   │   │   ├── rate_limit.rs
│   │   │   ├── scheduler.rs
│   │   │   ├── settings.rs
│   │   │   ├── spider.rs
│   │   │   ├── types.rs
│   │   │   └── updater.rs
│   │   ├── tests/range_download.rs
│   │   └── benches/local_download.rs
│   ├── loki-dm-cli/
│   │   └── src/main.rs
│   └── loki-dm-gui/
│       ├── src/main.rs
│       ├── web/
│       │   ├── src/App.jsx
│       │   ├── src/index.css
│       │   └── package.json
│       └── frontend/
│           └── (built web assets for Tauri runtime)
├── extensions/
│   ├── chromium/
│   ├── firefox/
│   └── native-host/
├── packaging/
│   ├── README.md
│   ├── linux/
│   │   ├── loki-dm.appdata.xml
│   │   └── loki-dm.desktop
│   ├── macos/Info.plist
│   └── windows/loki-dm.wxs
└── scripts/
    ├── benchmarks.sh
    ├── profile.sh
    └── cross-build.sh
```

## Dependency Rationale

| Crate | Purpose |
|---|---|
| `tokio` | async runtime, process management, fs/timers/channels |
| `reqwest` | HTTP stack, streaming, TLS, proxies |
| `futures-util` | stream handling |
| `serde` + `serde_json` + `toml` | config/queue/metadata persistence |
| `thiserror` | typed error model |
| `url` | URL parsing/joins |
| `rand` | retry jitter |
| `dirs` | OS config/download path discovery |
| `m3u8-rs` | native HLS playlist parsing |
| `curl` | native FTP/NTLM/Kerberos backend (libcurl bindings) |
| `clap` | CLI parsing |
| `tauri` | native Rust desktop shell + command bridge |
| `arboard` | clipboard integration |
| `tracing` + `tracing-subscriber` | structured logging |
| `anyhow` | app-layer error contexts |
| `tempfile` (dev) | integration test fixtures |

## Build and Test

```bash
npm --prefix crates/loki-dm-gui/web ci
npm --prefix crates/loki-dm-gui/web run build
cargo check --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build --release -p loki-dm-cli
(cd crates/loki-dm-gui && cargo tauri build --no-bundle)
```

Binaries:
- `target/release/loki-dm`
- `target/release/loki-dm-gui`

## CLI Command Surface

```bash
loki-dm download <url> [options]
loki-dm batch <urls.txt> [options]
loki-dm queue add|add-text|list|remove|set-priority|run|import|export ...
loki-dm settings show|import|export|reset ...
loki-dm spider <url> [--depth --ext --same-host --respect-robots --download]
loki-dm download-all <page-url> [--output-dir]
loki-dm media probe <url>
loki-dm media download <url> --output <file> [--format-id]
loki-dm schedule once|recurring ...
loki-dm update check|install ...
loki-dm native-host [--output-dir] [--start-immediately]
loki-dm native-manifests [--output-dir] [--binary-path] [--chrome-extension-id] [--install] [--verify] [--uninstall] [--report-json]
```

### Example: mirrored download with proxy + AV hook

```bash
loki-dm download "https://example.com/large.iso" \
  --mirror "https://cdn1.example.com/large.iso" \
  --mirror "https://cdn2.example.com/large.iso" \
  --connections 12 --min-connections 2 --max-connections 24 \
  --proxy-kind socks5 --proxy-host 127.0.0.1 --proxy-port 9050 \
  --av-hook "clamscan --no-summary {path}" \
  --output ~/Downloads/large.iso
```

### Example: cookie-authenticated download

```bash
loki-dm download "https://portal.example.com/report.csv" \
  --cookie-file ~/cookies.txt \
  --cookie "session=override123" \
  --header "Referer: https://portal.example.com/" \
  --output ~/Downloads/report.csv
```

### Example: queue priorities and duplicate control

```bash
loki-dm queue add "https://example.com/nightly.iso" --priority critical
loki-dm queue add-text ./urls.txt --priority high
loki-dm queue set-priority 42 --priority low
```

### Example: NTLM / Kerberos

```bash
loki-dm download \"https://intranet.example.local/file\" --use-ntlm
loki-dm download \"https://intranet.example.local/file\" --use-kerberos
```

### Example: download all assets from a page

```bash
loki-dm download-all "https://target.example/page" --output-dir ./grabbed --connections 8
```

### Example: media probe and download

```bash
loki-dm media probe "https://www.youtube.com/watch?v=..."
loki-dm media download "https://www.youtube.com/watch?v=..." --format-id 137+140 --output ./video.mp4
```

Note: YouTube/Vimeo/Twitch extraction requires `yt-dlp` available in `PATH`.

### Example: updater

```bash
loki-dm update check
loki-dm update install --launch
```

## GUI Usage

```bash
# Dev mode (hot reload)
(cd crates/loki-dm-gui && cargo tauri dev)

# Release binary
(cd crates/loki-dm-gui && cargo tauri build --no-bundle)
./target/release/loki-dm-gui
```

GUI includes:
- Downloads tab with context actions, live speed/progress/ETA, global/per-download charts
- Queue tab for persistent queued jobs
- Queue panel priority editor + duplicate protection + queue concurrency tuning
- Live queue auto-sync from browser/native-host writes (mtime polling, ~750ms cadence)
- Scheduler panel for timed once/recurring jobs (add/enable/disable/remove)
- Grabber panel for recursive site scan + queue/start integration
- Browser panel for native host generate/install/validate/uninstall with detailed reports
- Category-folder editor (Music/Video/Programs/Documents/Compressed/Other)
- Queue-completion command hook in Advanced settings
- Browser setup fields persisted to `settings.toml` (`host_name`, `binary_path`, extension IDs, manifest output dir)

## Browser Extension Setup

1. Build/install `loki-dm` binary.
2. Generate native host manifests from CLI:
   ```bash
   loki-dm native-manifests \
     --binary-path /absolute/path/to/loki-dm \
     --chrome-extension-id <your_chromium_extension_id> \
     --output-dir ./extensions/native-host
   ```
3. Optional: auto-install manifests on Linux/macOS:
   ```bash
   loki-dm native-manifests \
     --binary-path /absolute/path/to/loki-dm \
     --chrome-extension-id <your_chromium_extension_id> \
     --install
   ```
   If `--chrome-extension-id` is omitted, install still succeeds but emits a warning and uses the placeholder ID.
4. Validate installation:
   ```bash
   loki-dm native-manifests \
     --binary-path /absolute/path/to/loki-dm \
     --host-name com.loki.dm \
     --verify
   ```
   Optional machine-readable diagnostics:
   ```bash
   loki-dm native-manifests \
     --binary-path /absolute/path/to/loki-dm \
     --host-name com.loki.dm \
     --verify --report-json
   ```
5. Optional uninstall/cleanup:
   ```bash
   loki-dm native-manifests \
     --binary-path /absolute/path/to/loki-dm \
     --host-name com.loki.dm \
     --uninstall --verify
   ```
6. On Windows, `--install` writes HKCU native-host registry keys (no admin required).
7. You can perform the same setup directly from GUI: `Settings -> Browser`.
8. Load unpacked extension from:
   - Chromium: `extensions/chromium`
   - Firefox: `extensions/firefox`
9. Browser calls to native host queue downloads immediately by default (`action: "queue"`).

## Benchmarks (Executed: February 22, 2026)

Smoke benchmark run command:

```bash
URL_10GB='https://proof.ovh.net/files/10Mb.dat' \
URL_SMALL='https://proof.ovh.net/files/1Mb.dat' \
SMALL_PARALLEL=12 \
OUT_DIR='/tmp/loki-bench-smoke2' \
./scripts/benchmarks.sh
```

| Scenario | Tool | Real time |
|---|---|---:|
| Large file (10MB smoke) | Loki DM (`12` connections) | `2.959s` |
| Large file (10MB smoke) | curl baseline | `1.547s` |
| 100 small files (1MB each, parallel=12) | Loki DM | `25.530s` |
| Local bench (`LOKI_BENCH_URL=https://proof.ovh.net/files/1Mb.dat`) | `cargo bench` | `2459ms` |

Notes:
- Default benchmark URLs were migrated from expired `speed.hetzner.de` certificates to `proof.ovh.net`.
- For full stress workloads (10GB file, unstable-link tests), use env overrides in `scripts/benchmarks.sh`.

## Profiling

```bash
./scripts/profile.sh
```

- `cargo flamegraph`
- `samply`

## Cross-Compilation

```bash
./scripts/cross-build.sh
```

Targets:
- `x86_64-unknown-linux-gnu`
- `x86_64-pc-windows-gnu`
- `x86_64-apple-darwin`
- `aarch64-apple-darwin`

Notes:
- `scripts/cross-build.sh` performs `cargo check` for core/CLI on non-host targets by default.
- Set `CHECK_GUI_CROSS=1` to include GUI cross-checks when your host has the required cross-linkers/sysroots.

## Release Packaging

Local package build:

```bash
cd crates/loki-dm-gui
cargo tauri build --bundles app,appimage,deb,rpm,dmg,msi
```

Generated outputs are placed under:
- `target/release/bundle/macos/` (`.app`, `.dmg`)
- `target/release/bundle/deb/` (`.deb`)
- `target/release/bundle/rpm/` (`.rpm`)
- `target/release/bundle/appimage/` (`.AppImage`)
- `target/release/bundle/msi/` (`.msi`)

Installer metadata templates are in:
- `packaging/windows/loki-dm.wxs`
- `packaging/macos/Info.plist`
- `packaging/linux/loki-dm.desktop`
- `packaging/linux/loki-dm.appdata.xml`

## How Loki DM Improves Throughput

1. **Resume-first segment metadata model**
   - Each segment progress is persisted continuously to avoid restart waste.
2. **Largest-segment dynamic split**
   - New available workers split the largest remaining segment and continue immediately.
3. **Mirror-aware source choice**
   - Candidate URLs are probed and best source is selected automatically.
4. **Single pooled HTTP client**
   - Reuses sockets/TLS sessions and minimizes setup overhead.
5. **Direct random-access writes**
   - Writes straight into pre-sized `.part` offsets; no merge pass.
6. **Adaptive connection tuning**
   - Scales up/down based on measured speed and error pressure.
7. **Backoff + jitter retry strategy**
   - Prevents synchronized retries and improves resilience under unstable links.

## Remaining Gaps / Next Work

- Deeper per-platform SSPI/GSSAPI credential management controls
- Optional ffmpeg-assisted media post-processing presets
