//! Local preferences and the exact file locations shown in Settings. There is
//! no account service: these paths are the whole persistence model.

use eframe::egui;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Appearance {
    Dark,
    Light,
}

impl Default for Appearance {
    fn default() -> Self {
        Self::Dark
    }
}

impl Appearance {
    pub fn label(self) -> &'static str {
        match self {
            Self::Dark => "Dark",
            Self::Light => "Light",
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Dark => "dark",
            Self::Light => "light",
        }
    }

    pub fn from_str(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "dark" => Some(Self::Dark),
            "light" => Some(Self::Light),
            _ => None,
        }
    }

    pub fn theme(self) -> temp_ui::Theme {
        match self {
            Self::Dark => temp_ui::Theme::Dark,
            Self::Light => temp_ui::Theme::Light,
        }
    }

    pub fn apply(self, ctx: &egui::Context) {
        temp_ui::apply(ctx, self.theme());
    }
}

fn default_true() -> bool {
    true
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotificationSettings {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_true")]
    pub mail: bool,
    #[serde(default = "default_true")]
    pub sms: bool,
}

impl Default for NotificationSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            mail: true,
            sms: true,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default)]
    pub appearance: Appearance,
    #[serde(default)]
    pub notifications: NotificationSettings,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            appearance: Appearance::Dark,
            notifications: NotificationSettings::default(),
        }
    }
}

static SAVE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

impl Settings {
    pub fn load(path: &Path) -> Self {
        fs::read(path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| format!("cannot create the settings folder: {error}"))?;
        }
        let bytes = serde_json::to_vec_pretty(self)
            .map_err(|error| format!("cannot serialize settings: {error}"))?;
        // The native window and web handlers both write this file. Serialize
        // writers in this process and give each process its own temporary
        // file, then rename so no reader sees a half-written document.
        let _guard = SAVE_LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let temporary = path.with_extension(format!("json.{}.tmp", std::process::id()));
        if let Err(error) = fs::write(&temporary, bytes) {
            let _ = fs::remove_file(&temporary);
            return Err(format!("cannot save settings: {error}"));
        }
        if let Err(error) = fs::rename(&temporary, path) {
            let _ = fs::remove_file(&temporary);
            return Err(format!("cannot save settings: {error}"));
        }
        Ok(())
    }

    pub fn apply_notifications(&self) {
        temp_email::set_notifications_enabled(
            self.notifications.enabled && self.notifications.mail,
        );
        temp_sms::set_notifications_enabled(self.notifications.enabled && self.notifications.sms);
    }
}

#[derive(Clone, Debug)]
pub struct Paths {
    pub install_root: PathBuf,
    pub state_root: PathBuf,
    pub settings: PathBuf,
    pub token: PathBuf,
    pub token_lock: PathBuf,
    pub logs: PathBuf,
    pub mail_state: PathBuf,
    pub sms_state: PathBuf,
    pub otp_vault: PathBuf,
}

impl Paths {
    pub fn new(install_root: PathBuf, state_root: PathBuf, settings: PathBuf) -> Self {
        let run = install_root.join(".run");
        Self {
            mail_state: state_root.join("temp-email").join("state"),
            sms_state: state_root.join("temp-sms").join("state"),
            otp_vault: state_root
                .join("temp-otp")
                .join("state")
                .join("vault.json"),
            token: run.join("auth-token"),
            token_lock: run.join("auth-token.lock"),
            logs: run.join("logs"),
            install_root,
            state_root,
            settings,
        }
    }

    pub fn rows(&self) -> Vec<(&'static str, PathBuf)> {
        vec![
            ("App folder", self.install_root.clone()),
            ("State folder", self.state_root.clone()),
            ("Mail data", self.mail_state.clone()),
            ("SMS data", self.sms_state.clone()),
            ("OTP vault", self.otp_vault.clone()),
            ("Installation key", self.token.clone()),
            ("Settings", self.settings.clone()),
            ("Logs", self.logs.clone()),
        ]
    }

    /// Delete all user data. Only known app-owned paths are touched, so a
    /// development checkout's source folders cannot be removed by accident.
    pub fn clear_all(&self) -> Result<(), String> {
        let mut failures = Vec::new();
        for dir in [&self.mail_state, &self.sms_state] {
            remove_dir(dir, &mut failures);
        }
        // Older builds shipped a Voice tab. Remove its state when the user
        // deliberately clears all local data, without advertising the tool.
        remove_dir(&self.state_root.join("temp-number"), &mut failures);
        if let Some(otp_state) = self.otp_vault.parent() {
            remove_dir(otp_state, &mut failures);
        }
        remove_file(&self.settings, &mut failures);
        remove_file(&self.token, &mut failures);
        remove_file(&self.token_lock, &mut failures);
        remove_dir(&self.logs, &mut failures);
        if failures.is_empty() {
            Ok(())
        } else {
            Err(failures.join("; "))
        }
    }
}

fn remove_file(path: &Path, failures: &mut Vec<String>) {
    if let Err(error) = fs::remove_file(path) {
        if error.kind() != std::io::ErrorKind::NotFound {
            failures.push(format!("{}: {error}", path.display()));
        }
    }
}

fn remove_dir(path: &Path, failures: &mut Vec<String>) {
    if let Err(error) = fs::remove_dir_all(path) {
        if error.kind() != std::io::ErrorKind::NotFound {
            failures.push(format!("{}: {error}", path.display()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "gmo-settings-{name}-{}",
            std::process::id()
        ))
    }

    #[test]
    fn settings_round_trip_and_default() {
        let root = temp_root("round-trip");
        let _ = fs::remove_dir_all(&root);
        let path = root.join("settings.json");
        assert_eq!(Settings::load(&path).appearance, Appearance::Dark);
        assert!(Settings::load(&path).notifications.enabled);
        let settings = Settings {
            appearance: Appearance::Light,
            notifications: NotificationSettings {
                enabled: false,
                mail: true,
                sms: false,
            },
        };
        settings.save(&path).expect("save settings");
        let loaded = Settings::load(&path);
        assert_eq!(loaded.appearance, Appearance::Light);
        assert!(!loaded.notifications.enabled);
        assert!(loaded.notifications.mail);
        assert!(!loaded.notifications.sms);
        // A second save replaces the existing file atomically.
        let updated = Settings {
            appearance: Appearance::Dark,
            notifications: NotificationSettings::default(),
        };
        updated.save(&path).expect("replace settings");
        assert_eq!(Settings::load(&path).appearance, Appearance::Dark);
        assert!(!path.with_extension("json.tmp").exists());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn clear_all_removes_state_but_not_the_source_folder() {
        let root = temp_root("clear");
        let _ = fs::remove_dir_all(&root);
        let paths = Paths::new(
            root.join("app"),
            root.clone(),
            root.join("settings.json"),
        );
        fs::create_dir_all(paths.mail_state.join("nested")).expect("mail state");
        fs::create_dir_all(&paths.sms_state).expect("sms state");
        fs::create_dir_all(root.join("temp-number").join("state")).expect("legacy voice state");
        fs::create_dir_all(paths.otp_vault.parent().expect("vault parent")).expect("vault parent");
        fs::write(&paths.otp_vault, b"vault").expect("vault");
        fs::write(root.join("keep.txt"), b"keep").expect("source file");
        paths.clear_all().expect("clear data");
        assert!(!paths.mail_state.exists());
        assert!(!paths.sms_state.exists());
        assert!(!root.join("temp-number").exists());
        assert!(!paths.otp_vault.exists());
        assert!(!paths.otp_vault.parent().expect("vault parent").exists());
        assert!(root.join("keep.txt").exists());
        let _ = fs::remove_dir_all(&root);
    }
}
