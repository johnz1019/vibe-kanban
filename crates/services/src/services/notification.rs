use std::sync::{Arc, OnceLock};

use serde::Deserialize;
use serde_json::json;
use tokio::sync::RwLock;
use utils;
use uuid::Uuid;

use crate::services::config::{Config, NotificationConfig, SoundFile};

#[derive(Debug, Deserialize)]
struct DevPorts {
    frontend: u16,
}

/// Service for handling cross-platform notifications including sound alerts and push notifications
#[derive(Debug, Clone)]
pub struct NotificationService {
    config: Arc<RwLock<Config>>,
}

/// Cache for WSL root path from PowerShell
static WSL_ROOT_PATH_CACHE: OnceLock<Option<String>> = OnceLock::new();

impl NotificationService {
    pub fn new(config: Arc<RwLock<Config>>) -> Self {
        Self { config }
    }

    pub async fn kanban_task_url(&self, project_id: Uuid, task_id: Uuid) -> Option<String> {
        let base_url = Self::resolve_kanban_base_url().await?;
        Some(format!("{base_url}/projects/{project_id}/tasks/{task_id}"))
    }

    /// Send both sound and push notifications if enabled
    pub async fn notify(&self, title: &str, message: &str) {
        let config = self.config.read().await.notifications.clone();
        Self::send_notification(&config, title, message).await;
    }

    /// Internal method to send notifications with a given config
    async fn send_notification(config: &NotificationConfig, title: &str, message: &str) {
        if config.sound_enabled {
            Self::play_sound_notification(&config.sound_file).await;
        }

        if config.push_enabled {
            Self::send_push_notification(title, message).await;
        }

        if config.slack_enabled {
            if let Some(webhook_url) = config
                .slack_webhook_url
                .as_ref()
                .map(|url| url.trim())
                .filter(|url| !url.is_empty())
            {
                Self::send_slack_notification(webhook_url.to_string(), title, message).await;
            } else {
                tracing::warn!(
                    "Slack notifications enabled but webhook URL is missing"
                );
            }
        }
    }

    /// Play a system sound notification across platforms
    async fn play_sound_notification(sound_file: &SoundFile) {
        let file_path = match sound_file.get_path().await {
            Ok(path) => path,
            Err(e) => {
                tracing::error!("Failed to create cached sound file: {}", e);
                return;
            }
        };

        // Use platform-specific sound notification
        // Note: spawn() calls are intentionally not awaited - sound notifications should be fire-and-forget
        if cfg!(target_os = "macos") {
            let _ = tokio::process::Command::new("afplay")
                .arg(&file_path)
                .spawn();
        } else if cfg!(target_os = "linux") && !utils::is_wsl2() {
            // Try different Linux audio players
            if tokio::process::Command::new("paplay")
                .arg(&file_path)
                .spawn()
                .is_ok()
            {
                // Success with paplay
            } else if tokio::process::Command::new("aplay")
                .arg(&file_path)
                .spawn()
                .is_ok()
            {
                // Success with aplay
            } else {
                // Try system bell as fallback
                let _ = tokio::process::Command::new("echo")
                    .arg("-e")
                    .arg("\\a")
                    .spawn();
            }
        } else if cfg!(target_os = "windows") || (cfg!(target_os = "linux") && utils::is_wsl2()) {
            // Convert WSL path to Windows path if in WSL2
            let file_path = if utils::is_wsl2() {
                if let Some(windows_path) = Self::wsl_to_windows_path(&file_path).await {
                    windows_path
                } else {
                    file_path.to_string_lossy().to_string()
                }
            } else {
                file_path.to_string_lossy().to_string()
            };

            let _ = tokio::process::Command::new("powershell.exe")
                .arg("-c")
                .arg(format!(
                    r#"(New-Object Media.SoundPlayer "{file_path}").PlaySync()"#
                ))
                .spawn();
        }
    }

    /// Send a cross-platform push notification
    async fn send_push_notification(title: &str, message: &str) {
        if cfg!(target_os = "macos") {
            Self::send_macos_notification(title, message).await;
        } else if cfg!(target_os = "linux") && !utils::is_wsl2() {
            Self::send_linux_notification(title, message).await;
        } else if cfg!(target_os = "windows") || (cfg!(target_os = "linux") && utils::is_wsl2()) {
            Self::send_windows_notification(title, message).await;
        }
    }

    /// Send macOS notification using osascript
    async fn send_macos_notification(title: &str, message: &str) {
        let script = format!(
            r#"display notification "{message}" with title "{title}" sound name "Glass""#,
            message = message.replace('"', r#"\""#),
            title = title.replace('"', r#"\""#)
        );

        let _ = tokio::process::Command::new("osascript")
            .arg("-e")
            .arg(script)
            .spawn();
    }

    /// Send Linux notification using notify-rust
    async fn send_linux_notification(title: &str, message: &str) {
        use notify_rust::Notification;

        let title = title.to_string();
        let message = message.to_string();

        let _handle = tokio::task::spawn_blocking(move || {
            if let Err(e) = Notification::new()
                .summary(&title)
                .body(&message)
                .timeout(10000)
                .show()
            {
                tracing::error!("Failed to send Linux notification: {}", e);
            }
        });
        drop(_handle); // Don't await, fire-and-forget
    }

    /// Send Windows/WSL notification using PowerShell toast script
    async fn send_windows_notification(title: &str, message: &str) {
        let script_path = match utils::get_powershell_script().await {
            Ok(path) => path,
            Err(e) => {
                tracing::error!("Failed to get PowerShell script: {}", e);
                return;
            }
        };

        // Convert WSL path to Windows path if in WSL2
        let script_path_str = if utils::is_wsl2() {
            if let Some(windows_path) = Self::wsl_to_windows_path(&script_path).await {
                windows_path
            } else {
                script_path.to_string_lossy().to_string()
            }
        } else {
            script_path.to_string_lossy().to_string()
        };

        let _ = tokio::process::Command::new("powershell.exe")
            .arg("-NoProfile")
            .arg("-ExecutionPolicy")
            .arg("Bypass")
            .arg("-File")
            .arg(script_path_str)
            .arg("-Title")
            .arg(title)
            .arg("-Message")
            .arg(message)
            .spawn();
    }

    /// Send Slack notification using incoming webhook
    async fn send_slack_notification(webhook_url: String, title: &str, message: &str) {
        fn escape_mrkdwn(s: &str) -> String {
            s.replace('\\', r"\\")
                .replace('*', r"\*")
                .replace('_', r"\_")
                .replace('~', r"\~")
                .replace('`', r"\`")
        }

        fn extract_url(s: &str) -> Option<String> {
            let s = s.trim();

            // Slack-style: <url|text> or <url>
            if let Some(stripped) = s.strip_prefix('<')
                && let Some(close) = stripped.find('>')
            {
                let inner = &stripped[..close];
                let url = inner.split('|').next().unwrap_or("").trim();
                if url.starts_with("http://") || url.starts_with("https://") {
                    return Some(url.to_string());
                }
            }

            // Markdown-style: [text](url)
            if let Some(open) = s.find("](")
                && s.starts_with('[')
                && s.ends_with(')')
            {
                let url = &s[open + 2..s.len() - 1];
                let url = url.trim();
                if url.starts_with("http://") || url.starts_with("https://") {
                    return Some(url.to_string());
                }
            }

            // Raw URL token
            for token in s.split_whitespace() {
                if token.starts_with("http://") || token.starts_with("https://") {
                    return Some(token.trim_matches(|c: char| c == ')' || c == ']').to_string());
                }
            }

            None
        }

        fn format_slack_message(message: &str) -> String {
            let mut out = Vec::new();
            for line in message.lines() {
                let trimmed = line.trim();
                if let Some(rest) = trimmed.strip_prefix("点击查看:") {
                    if let Some(url) = extract_url(rest) {
                        out.push(format!("<{url}|点击查看>"));
                        continue;
                    }
                }
                out.push(trimmed.to_string());
            }
            out.join("\n")
        }

        let title = escape_mrkdwn(title);
        let message = format_slack_message(message);
        let text = format!("*{title}*\n{message}");

        tokio::spawn(async move {
            let client = reqwest::Client::new();
            let response = client
                .post(&webhook_url)
                .json(&json!({ "text": text, "mrkdwn": true }))
                .send()
                .await;

            match response {
                Ok(resp) if resp.status().is_success() => {}
                Ok(resp) => {
                    tracing::error!(
                        "Slack notification failed with status: {}",
                        resp.status()
                    );
                }
                Err(err) => {
                    tracing::error!("Failed to send Slack notification: {}", err);
                }
            }
        });
    }

    async fn resolve_kanban_base_url() -> Option<String> {
        fn normalize(s: String) -> Option<String> {
            let trimmed = s.trim().trim_end_matches('/').trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            }
        }

        if let Ok(url) = std::env::var("SERVER_PUBLIC_BASE_URL")
            && let Some(url) = normalize(url)
        {
            return Some(url);
        }

        if let Ok(url) = std::env::var("VITE_APP_BASE_URL")
            && let Some(url) = normalize(url)
        {
            return Some(url);
        }

        // Local dev: prefer the Vite dev server (frontend), if present.
        if let Ok(content) = tokio::fs::read_to_string(".dev-ports.json").await
            && let Ok(ports) = serde_json::from_str::<DevPorts>(&content)
        {
            return Some(format!("http://127.0.0.1:{}", ports.frontend));
        }

        // Fallback: use backend port discovery (used by packaged/server mode).
        if let Ok(port) = utils::port_file::read_port_file("vibe-kanban").await {
            let host = std::env::var("HOST").unwrap_or_else(|_| "127.0.0.1".to_string());
            let host = match host.as_str() {
                "0.0.0.0" | "::" => "127.0.0.1",
                other => other,
            };
            return Some(format!("http://{host}:{port}"));
        }

        None
    }

    /// Get WSL root path via PowerShell (cached)
    async fn get_wsl_root_path() -> Option<String> {
        if let Some(cached) = WSL_ROOT_PATH_CACHE.get() {
            return cached.clone();
        }

        match tokio::process::Command::new("powershell.exe")
            .arg("-c")
            .arg("(Get-Location).Path -replace '^.*::', ''")
            .current_dir("/")
            .output()
            .await
        {
            Ok(output) => {
                match String::from_utf8(output.stdout) {
                    Ok(pwd_str) => {
                        let pwd = pwd_str.trim();
                        tracing::info!("WSL root path detected: {}", pwd);

                        // Cache the result
                        let _ = WSL_ROOT_PATH_CACHE.set(Some(pwd.to_string()));
                        return Some(pwd.to_string());
                    }
                    Err(e) => {
                        tracing::error!("Failed to parse PowerShell pwd output as UTF-8: {}", e);
                    }
                }
            }
            Err(e) => {
                tracing::error!("Failed to execute PowerShell pwd command: {}", e);
            }
        }

        // Cache the failure result
        let _ = WSL_ROOT_PATH_CACHE.set(None);
        None
    }

    /// Convert WSL path to Windows UNC path for PowerShell
    async fn wsl_to_windows_path(wsl_path: &std::path::Path) -> Option<String> {
        let path_str = wsl_path.to_string_lossy();

        // Relative paths work fine as-is in PowerShell
        if !path_str.starts_with('/') {
            tracing::debug!("Using relative path as-is: {}", path_str);
            return Some(path_str.to_string());
        }

        // Get cached WSL root path from PowerShell
        if let Some(wsl_root) = Self::get_wsl_root_path().await {
            // Simply concatenate WSL root with the absolute path - PowerShell doesn't mind /
            let windows_path = format!("{wsl_root}{path_str}");
            tracing::debug!("WSL path converted: {} -> {}", path_str, windows_path);
            Some(windows_path)
        } else {
            tracing::error!(
                "Failed to determine WSL root path for conversion: {}",
                path_str
            );
            None
        }
    }
}
