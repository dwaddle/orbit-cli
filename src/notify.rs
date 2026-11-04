use colored::Colorize;
use serde::{Deserialize, Serialize};
use std::io::{self, Write};
use std::process::Command;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotificationConfig {
    pub enabled: bool,
    pub bell_enabled: bool,
    pub desktop_enabled: bool,
    pub threshold_secs: u64,
}

impl Default for NotificationConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            bell_enabled: true,
            desktop_enabled: true,
            threshold_secs: 5,
        }
    }
}

#[derive(Debug, Clone)]
pub struct TaskNotificationManager {
    config: NotificationConfig,
}

impl TaskNotificationManager {
    pub fn new(config: NotificationConfig) -> Self {
        Self { config }
    }

    pub fn config(&self) -> &NotificationConfig {
        &self.config
    }

    pub fn set_enabled(&mut self, enabled: bool) {
        self.config.enabled = enabled;
    }

    pub fn set_bell(&mut self, enabled: bool) {
        self.config.bell_enabled = enabled;
    }

    pub fn set_desktop(&mut self, enabled: bool) {
        self.config.desktop_enabled = enabled;
    }

    pub fn set_threshold(&mut self, threshold_secs: u64) {
        self.config.threshold_secs = threshold_secs;
    }

    pub fn notify_if_exceeded(
        &self,
        task_name: &str,
        duration_secs: u64,
        success: bool,
        details: Option<&str>,
    ) {
        if !self.config.enabled || duration_secs < self.config.threshold_secs {
            return;
        }

        // 1. Terminal audio bell
        if self.config.bell_enabled {
            print!("\x07");
            let _ = io::stdout().flush();
        }

        // 2. Desktop notification
        if self.config.desktop_enabled {
            let title = format!(
                "Orbit CLI - {} ({})",
                task_name,
                if success { "Success" } else { "Failed" }
            );
            let message = format!(
                "Task completed in {}s. {}",
                duration_secs,
                details.unwrap_or("")
            );
            Self::send_desktop_notification(&title, &message, success);
        }
    }

    fn send_desktop_notification(title: &str, message: &str, success: bool) {
        // Linux: notify-send
        #[cfg(target_os = "linux")]
        {
            let icon = if success {
                "dialog-information"
            } else {
                "dialog-error"
            };
            let _ = Command::new("notify-send")
                .args(["-a", "Orbit CLI", "-i", icon, title, message])
                .spawn();
        }

        // macOS: osascript
        #[cfg(target_os = "macos")]
        {
            let script = format!(
                "display notification \"{}\" with title \"{}\"",
                message.replace('"', "\\\""),
                title.replace('"', "\\\"")
            );
            let _ = Command::new("osascript").args(["-e", &script]).spawn();
        }
    }

    pub fn format_status(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!(
            "{}\n",
            "=== TASK NOTIFICATION SETTINGS ===".bold().cyan()
        ));
        out.push_str(&format!(
            "  Global Notifications: {}\n",
            if self.config.enabled {
                "ENABLED".green()
            } else {
                "DISABLED".red()
            }
        ));
        out.push_str(&format!(
            "  Terminal Bell (\\x07): {}\n",
            if self.config.bell_enabled {
                "ENABLED".green()
            } else {
                "DISABLED".yellow()
            }
        ));
        out.push_str(&format!(
            "  Desktop Notification: {}\n",
            if self.config.desktop_enabled {
                "ENABLED".green()
            } else {
                "DISABLED".yellow()
            }
        ));
        out.push_str(&format!(
            "  Threshold Trigger:    {} seconds (only fires for tasks >= threshold)\n",
            self.config.threshold_secs
        ));
        out.push_str(
            "  Commands:             /notify [on|off|bell|desktop], /notify threshold <sec>\n",
        );
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_notification_threshold_filtering() {
        let manager = TaskNotificationManager::new(NotificationConfig {
            enabled: true,
            bell_enabled: true,
            desktop_enabled: false,
            threshold_secs: 5,
        });

        // Does not fire under threshold (2s < 5s)
        manager.notify_if_exceeded("Quick check", 2, true, None);

        // Fires when exceeding threshold
        manager.notify_if_exceeded("Long build", 10, true, Some("Cargo test passed"));
    }

    #[test]
    fn test_notification_status_formatting() {
        let manager = TaskNotificationManager::new(NotificationConfig::default());
        let status = manager.format_status();
        assert!(status.contains("TASK NOTIFICATION SETTINGS"));
        assert!(status.contains("5 seconds"));
    }
}
