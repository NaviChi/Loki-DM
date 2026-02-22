#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

fn cli_bin() -> &'static str {
    env!("CARGO_BIN_EXE_loki-dm")
}

fn unique_tag() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    format!("{}_{}", std::process::id(), nanos)
}

struct TempDirGuard {
    path: PathBuf,
}

impl TempDirGuard {
    fn new(prefix: &str) -> Self {
        let path = std::env::temp_dir().join(format!("{prefix}_{}", unique_tag()));
        fs::create_dir_all(&path).expect("create temp dir");
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDirGuard {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

#[test]
fn native_manifests_help_mentions_verify_and_uninstall() {
    let output = Command::new(cli_bin())
        .args(["native-manifests", "--help"])
        .output()
        .expect("run native-manifests --help");
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    assert!(stdout.contains("--verify"));
    assert!(stdout.contains("--uninstall"));
    assert!(stdout.contains("--report-json"));
}

#[test]
fn native_manifests_rejects_install_uninstall_conflict() {
    let output = Command::new(cli_bin())
        .args(["native-manifests", "--install", "--uninstall"])
        .output()
        .expect("run conflict command");
    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(stderr.contains("cannot be used with '--uninstall'"));
}

#[test]
fn native_manifests_install_fails_for_missing_binary() {
    let missing_path = format!("/tmp/loki-dm-missing-{}", unique_tag());
    let output = Command::new(cli_bin())
        .args([
            "native-manifests",
            "--install",
            "--host-name",
            &format!("com.loki.dm.test.{}", unique_tag()),
            "--binary-path",
            &missing_path,
            "--chrome-extension-id",
            "testchromiumid",
        ])
        .output()
        .expect("run install command with missing binary");
    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(stderr.contains("binary path does not exist or is not a file"));
}

#[test]
fn native_manifests_verify_reports_incomplete_for_unique_host() {
    let host = format!("com.loki.dm.verify.{}", unique_tag());
    let output = Command::new(cli_bin())
        .args(["native-manifests", "--verify", "--host-name", &host])
        .output()
        .expect("run verify command");
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    assert!(stdout.contains("verify: installation incomplete"));
}

#[test]
fn native_manifests_generate_writes_manifest_pair() {
    let output_dir = TempDirGuard::new("loki_dm_cli_native_manifests");
    let host = format!("com.loki.dm.generate.{}", unique_tag());
    let output = Command::new(cli_bin())
        .args([
            "native-manifests",
            "--host-name",
            &host,
            "--output-dir",
            output_dir.path().to_str().expect("utf8 output dir"),
            "--chrome-extension-id",
            "testchromiumid",
            "--firefox-extension-id",
            "loki-dm@example.org",
        ])
        .output()
        .expect("run generate command");
    assert!(output.status.success());

    let chrome_manifest = output_dir.path().join(format!("{host}.chrome.json"));
    let firefox_manifest = output_dir.path().join(format!("{host}.firefox.json"));
    assert!(
        chrome_manifest.is_file(),
        "missing {}",
        chrome_manifest.display()
    );
    assert!(
        firefox_manifest.is_file(),
        "missing {}",
        firefox_manifest.display()
    );

    let chrome_body = fs::read_to_string(chrome_manifest).expect("read chromium manifest");
    let firefox_body = fs::read_to_string(firefox_manifest).expect("read firefox manifest");
    assert!(chrome_body.contains("testchromiumid"));
    assert!(firefox_body.contains("loki-dm@example.org"));
}

#[test]
fn native_manifests_generate_with_placeholder_emits_warning() {
    let output_dir = TempDirGuard::new("loki_dm_cli_placeholder_warning");
    let host = format!("com.loki.dm.placeholder.{}", unique_tag());
    let output = Command::new(cli_bin())
        .args([
            "native-manifests",
            "--host-name",
            &host,
            "--output-dir",
            output_dir.path().to_str().expect("utf8 output dir"),
        ])
        .output()
        .expect("run generate command with placeholder id");
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    assert!(stdout.contains("warning: set --chrome-extension-id"));
}

#[test]
fn native_manifests_generate_with_explicit_chromium_id_has_no_placeholder_warning() {
    let output_dir = TempDirGuard::new("loki_dm_cli_no_placeholder_warning");
    let host = format!("com.loki.dm.no.placeholder.{}", unique_tag());
    let output = Command::new(cli_bin())
        .args([
            "native-manifests",
            "--host-name",
            &host,
            "--output-dir",
            output_dir.path().to_str().expect("utf8 output dir"),
            "--chrome-extension-id",
            "testchromiumid",
        ])
        .output()
        .expect("run generate command with explicit chromium id");
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    assert!(!stdout.contains("warning: set --chrome-extension-id"));
}

#[test]
fn native_manifests_report_json_contains_diagnostics_keys() {
    let host = format!("com.loki.dm.report.{}", unique_tag());
    let output = Command::new(cli_bin())
        .args([
            "native-manifests",
            "--host-name",
            &host,
            "--chrome-extension-id",
            "testchromiumid",
            "--verify",
            "--report-json",
        ])
        .output()
        .expect("run native-manifests --report-json");
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    assert!(stdout.contains("\"host_name\""));
    assert!(stdout.contains("\"binary_path_valid\""));
    assert!(stdout.contains("\"validation\""));
}
