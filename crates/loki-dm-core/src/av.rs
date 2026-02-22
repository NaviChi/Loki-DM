use std::path::Path;

use tokio::process::Command;

use crate::error::{LokiDmError, Result};

#[derive(Debug, Clone)]
pub struct AvHookResult {
    pub command: String,
    pub success: bool,
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

pub fn render_hook_command(template: &str, output_path: &Path) -> String {
    let full = output_path.display().to_string();
    let file_name = output_path
        .file_name()
        .and_then(std::ffi::OsStr::to_str)
        .unwrap_or_default();

    template
        .replace("{path}", &full)
        .replace("{file}", &full)
        .replace("{filename}", file_name)
}

pub async fn run_av_hook(template: &str, output_path: &Path) -> Result<AvHookResult> {
    let rendered = render_hook_command(template, output_path);

    #[cfg(windows)]
    let output = Command::new("cmd")
        .arg("/C")
        .arg(&rendered)
        .output()
        .await
        .map_err(|err| LokiDmError::Message(format!("failed to run AV hook: {err}")))?;

    #[cfg(not(windows))]
    let output = Command::new("sh")
        .arg("-c")
        .arg(&rendered)
        .output()
        .await
        .map_err(|err| LokiDmError::Message(format!("failed to run AV hook: {err}")))?;

    Ok(AvHookResult {
        command: rendered,
        success: output.status.success(),
        code: output.status.code(),
        stdout: String::from_utf8_lossy(&output.stdout).trim().to_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
    })
}
