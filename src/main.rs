//! GMO TempTools — one Windows desktop window for temporary mail, SMS and
//! OTP verification. The two HTTP service listeners run on background threads
//! in this same process, and a matching web app is served locally.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app_info;
mod security;
mod settings;
#[cfg(windows)]
mod tray;
mod webapp;

#[cfg(not(target_os = "windows"))]
compile_error!("GMO TempTools is a Windows desktop application.");

use app_info::{OWNER, TOOL_RELEASES};
use eframe::egui;
use eframe::App;
use settings::{Appearance, Paths, Settings};
use std::path::PathBuf;
use std::time::{Duration, Instant};

const SETTINGS_SYNC_INTERVAL: Duration = Duration::from_secs(1);

#[derive(Clone, Copy, PartialEq, Eq)]
enum Screen {
    Mail,
    Sms,
    Otp,
    Settings,
}

struct TempTools {
    screen: Screen,
    last_screen: Screen,
    email: temp_email::gui::EmailApp,
    sms: temp_sms::gui::SmsApp,
    otp: temp_otp::gui::OtpApp,
    settings: Settings,
    settings_path: PathBuf,
    last_settings_sync: Instant,
    paths: Paths,
    auth_token: String,
    confirm_clear: bool,
    status: Option<(String, bool)>,
    #[cfg(windows)]
    tray: Option<tray::Tray>,
}

impl TempTools {
    fn new(
        email: temp_email::gui::EmailApp,
        sms: temp_sms::gui::SmsApp,
        otp: temp_otp::gui::OtpApp,
        settings: Settings,
        settings_path: PathBuf,
        paths: Paths,
        auth_token: String,
        #[cfg(windows)] tray: Option<tray::Tray>,
    ) -> Self {
        Self {
            screen: Screen::Mail,
            last_screen: Screen::Mail,
            email,
            sms,
            otp,
            settings,
            settings_path,
            last_settings_sync: Instant::now(),
            paths,
            auth_token,
            confirm_clear: false,
            status: None,
            #[cfg(windows)]
            tray,
        }
    }

    /// The web app writes the same settings file. Sync it from every screen so
    /// a web theme or notification change is never waiting on the Settings tab.
    fn sync_settings(&mut self, ctx: &egui::Context) {
        let entered_settings =
            self.screen == Screen::Settings && self.last_screen != Screen::Settings;
        if !entered_settings && self.last_settings_sync.elapsed() < SETTINGS_SYNC_INTERVAL {
            return;
        }

        let disk = Settings::load(&self.settings_path);
        if disk.appearance != self.settings.appearance
            || disk.notifications != self.settings.notifications
        {
            self.settings = disk;
            self.settings.appearance.apply(ctx);
            self.settings.apply_notifications();
        }
        self.last_settings_sync = Instant::now();
    }

    fn apply_appearance(&mut self, appearance: Appearance, ctx: &egui::Context) {
        if appearance == self.settings.appearance {
            return;
        }
        self.settings.appearance = appearance;
        appearance.apply(ctx);
        if let Err(error) = self.settings.save(&self.settings_path) {
            // Keep the UI on the values actually stored on disk.
            self.settings = Settings::load(&self.settings_path);
            self.settings.appearance.apply(ctx);
            self.settings.apply_notifications();
            self.status = Some((error, true));
        } else {
            self.status = Some((
                format!("Appearance set to {}.", appearance.label().to_ascii_lowercase()),
                false,
            ));
        }
    }

    fn shell(&mut self, ctx: &egui::Context) {
        let p = temp_ui::palette(ctx);
        egui::SidePanel::left("navigation_rail")
            .resizable(false)
            .exact_width(236.0)
            .frame(
                egui::Frame::new()
                    .fill(p.rail)
                    .inner_margin(egui::Margin {
                        left: 14,
                        right: 14,
                        top: 20,
                        bottom: 16,
                    }),
            )
            .show(ctx, |ui| {
                temp_ui::brand(ui, "GMO TempTools", "local services");
                ui.add_space(26.0);
                temp_ui::eyebrow(ui, "TOOLS");
                ui.add_space(6.0);
                if temp_ui::nav_item(ui, "Mail", "Inboxes & codes", self.screen == Screen::Mail)
                    .clicked()
                {
                    self.screen = Screen::Mail;
                }
                if temp_ui::nav_item(
                    ui,
                    "SMS",
                    "Public numbers · Beta",
                    self.screen == Screen::Sms,
                )
                .clicked()
                {
                    self.screen = Screen::Sms;
                }
                if temp_ui::nav_item(
                    ui,
                    "OTP",
                    "Codes & authenticator",
                    self.screen == Screen::Otp,
                )
                .clicked()
                {
                    self.screen = Screen::Otp;
                }

                ui.add_space(18.0);
                temp_ui::eyebrow(ui, "APP");
                ui.add_space(6.0);
                if temp_ui::nav_item(
                    ui,
                    "Settings",
                    "Theme, data & paths",
                    self.screen == Screen::Settings,
                )
                .clicked()
                {
                    self.screen = Screen::Settings;
                }

                ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| {
                    egui::Frame::new()
                        .fill(p.surface)
                        .stroke(egui::Stroke::new(1.0_f32, p.line))
                        .corner_radius(12)
                        .inner_margin(egui::Margin::same(12))
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            // The rail lays its footer out bottom-up, so add the
                            // lines from the bottom of the card upward.
                            ui.label(
                                egui::RichText::new(format!(
                                    "v{} · {}",
                                    app_info::APP_VERSION,
                                    OWNER
                                ))
                                .font(egui::FontId::monospace(9.5))
                                .color(p.muted),
                            );
                            ui.label(
                                egui::RichText::new("mail 8781 · texts 8782")
                                    .font(egui::FontId::monospace(9.5))
                                    .color(p.muted),
                            );
                            ui.add_space(7.0);
                            ui.label(
                                egui::RichText::new("Everything stays on this PC.")
                                    .size(10.0)
                                    .color(p.muted),
                            );
                            ui.add_space(3.0);
                            ui.horizontal(|ui| {
                                let (rect, _) = ui.allocate_exact_size(
                                    egui::Vec2::splat(7.0),
                                    egui::Sense::hover(),
                                );
                                ui.painter().circle_filled(rect.center(), 3.5, p.accent);
                                ui.label(
                                    egui::RichText::new("Local only")
                                        .size(11.5)
                                        .strong()
                                        .color(p.text),
                                );
                            });
                        });
                });
            });
    }

    fn settings_screen(&mut self, ctx: &egui::Context) {
        let p = temp_ui::palette(ctx);
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(p.canvas)
                    .inner_margin(egui::Margin {
                        left: 26,
                        right: 26,
                        top: 18,
                        bottom: 20,
                    }),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new("Settings")
                                    .size(25.0)
                                    .strong()
                                    .color(p.text),
                            );
                            temp_ui::chip(
                                ui,
                                &format!("v{}", app_info::APP_VERSION),
                                temp_ui::Tone::Neutral,
                            );
                        });
                        ui.add_space(1.0);
                        ui.label(
                            egui::RichText::new(
                                "Appearance, notifications, local paths and data cleanup.",
                            )
                            .size(12.5)
                            .color(p.muted),
                        );
                    });
                });
                ui.add_space(12.0);

                if let Some((message, is_error)) = &self.status {
                    temp_ui::banner(ui, message, *is_error);
                    ui.add_space(10.0);
                }

                egui::ScrollArea::vertical()
                    .id_salt("settings_scroll")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.set_max_width(880.0);

                        temp_ui::card(ui, |ui| {
                            ui.label(
                                egui::RichText::new("Appearance")
                                    .size(14.5)
                                    .strong()
                                    .color(p.text),
                            );
                            ui.label(
                                egui::RichText::new(
                                    "Saved on this PC and shared with the local web app.",
                                )
                                .size(11.5)
                                .color(p.muted),
                            );
                            ui.add_space(10.0);
                            let mut chosen = None;
                            ui.horizontal(|ui| {
                                if temp_ui::theme_tile(
                                    ui,
                                    temp_ui::Theme::Dark,
                                    self.settings.appearance == Appearance::Dark,
                                )
                                .clicked()
                                {
                                    chosen = Some(Appearance::Dark);
                                }
                                ui.add_space(2.0);
                                if temp_ui::theme_tile(
                                    ui,
                                    temp_ui::Theme::Light,
                                    self.settings.appearance == Appearance::Light,
                                )
                                .clicked()
                                {
                                    chosen = Some(Appearance::Light);
                                }
                            });
                            if let Some(appearance) = chosen {
                                self.apply_appearance(appearance, ctx);
                            }
                        });

                        ui.add_space(12.0);
                        temp_ui::card(ui, |ui| {
                            ui.label(
                                egui::RichText::new("Desktop notifications")
                                    .size(14.5)
                                    .strong()
                                    .color(p.text),
                            );
                            ui.label(
                                egui::RichText::new(
                                    "Native toasts run while the window is hidden. OTP codes are never included.",
                                )
                                .size(11.5)
                                .color(p.muted),
                            );
                            ui.add_space(8.0);

                            let mut notifications = self.settings.notifications.clone();
                            let mut changed = false;
                            changed |= toggle_row(
                                ui,
                                "Show desktop notifications",
                                "Turn every native toast on or off.",
                                &mut notifications.enabled,
                                true,
                            );
                            changed |= toggle_row(
                                ui,
                                "New mail",
                                "A toast when an inbox receives a message.",
                                &mut notifications.mail,
                                notifications.enabled,
                            );
                            changed |= toggle_row(
                                ui,
                                "New texts",
                                "A toast when a watched number receives a text.",
                                &mut notifications.sms,
                                notifications.enabled,
                            );

                            ui.add_space(8.0);
                            if temp_ui::secondary_button(ui, "Send test notification").clicked() {
                                desktop_notifications::notify(
                                    "GMO TempTools · Test notification",
                                    "Desktop notifications are working.",
                                );
                                self.status =
                                    Some(("Test notification sent.".to_string(), false));
                            }
                            if changed {
                                self.settings.notifications = notifications;
                                self.settings.apply_notifications();
                                if let Err(error) = self.settings.save(&self.settings_path) {
                                    self.settings = Settings::load(&self.settings_path);
                                    self.settings.apply_notifications();
                                    self.status = Some((error, true));
                                } else {
                                    self.status = Some((
                                        "Notification preferences saved.".to_string(),
                                        false,
                                    ));
                                }
                            }
                        });

                        ui.add_space(12.0);
                        temp_ui::card(ui, |ui| {
                            ui.label(
                                egui::RichText::new("Installation key")
                                    .size(14.5)
                                    .strong()
                                    .color(p.text),
                            );
                            ui.add_space(2.0);
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(
                                        "This per-install key unlocks only the local web app and APIs on this PC. It is not an account, subscription or remote license. Each installation creates its own key on first launch.",
                                    )
                                    .size(11.5)
                                    .color(p.muted),
                                )
                                .wrap(),
                            );
                            ui.add_space(8.0);
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(self.paths.token.display().to_string())
                                        .font(egui::FontId::monospace(10.0))
                                        .color(p.muted),
                                )
                                .truncate(),
                            );
                            ui.add_space(6.0);
                            ui.horizontal(|ui| {
                                if temp_ui::secondary_button(ui, "Copy installation key").clicked() {
                                    ctx.copy_text(self.auth_token.clone());
                                    self.status = Some((
                                        "Installation key copied to the clipboard.".to_string(),
                                        false,
                                    ));
                                }
                                if temp_ui::ghost_button(ui, "Open key folder").clicked() {
                                    if let Some(parent) = self.paths.token.parent() {
                                        open_folder(parent);
                                    }
                                }
                            });
                            ui.add_space(4.0);
                            ui.label(
                                egui::RichText::new(
                                    "Never share this key publicly. SMS carrier webhook credentials are separate environment variables.",
                                )
                                .size(10.0)
                                .color(p.muted),
                            );
                        });

                        ui.add_space(12.0);
                        temp_ui::card(ui, |ui| {
                            ui.label(
                                egui::RichText::new("Local paths")
                                    .size(14.5)
                                    .strong()
                                    .color(p.text),
                            );
                            ui.label(
                                egui::RichText::new(
                                    "Everything the app stores is on this device. Copy puts a path on the clipboard.",
                                )
                                .size(11.5)
                                .color(p.muted),
                            );
                            ui.add_space(8.0);
                            for (label, path) in self.paths.rows() {
                                ui.horizontal(|ui| {
                                    ui.label(
                                        egui::RichText::new(label).size(10.5).color(p.muted),
                                    );
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            if ui
                                                .add(
                                                    egui::Button::new(
                                                        egui::RichText::new("Copy")
                                                            .size(10.5)
                                                            .color(p.muted),
                                                    )
                                                    .fill(egui::Color32::TRANSPARENT)
                                                    .stroke(egui::Stroke::NONE)
                                                    .corner_radius(6),
                                                )
                                                .clicked()
                                            {
                                                ctx.copy_text(path.display().to_string());
                                                self.status = Some((
                                                    format!("{label} path copied."),
                                                    false,
                                                ));
                                            }
                                        },
                                    );
                                });
                                ui.add(
                                    egui::Label::new(
                                        egui::RichText::new(path.display().to_string())
                                            .font(egui::FontId::monospace(10.0))
                                            .color(p.text),
                                    )
                                    .truncate(),
                                );
                                ui.add_space(5.0);
                            }
                        });

                        ui.add_space(12.0);
                        temp_ui::card(ui, |ui| {
                            ui.label(
                                egui::RichText::new("Release notes and changelog")
                                    .size(14.5)
                                    .strong()
                                    .color(p.text),
                            );
                            ui.label(
                                egui::RichText::new(
                                    "Each integrated tool keeps its history here, even though there is one app now.",
                                )
                                .size(11.5)
                                .color(p.muted),
                            );
                            ui.add_space(4.0);
                            for note in TOOL_RELEASES {
                                egui::CollapsingHeader::new(
                                    egui::RichText::new(format!(
                                        "{} {} — {}",
                                        note.tool, note.version, note.summary
                                    ))
                                    .size(11.5)
                                    .color(p.text),
                                )
                                .default_open(false)
                                .show(ui, |ui| {
                                    for (version, change) in note.changelog {
                                        ui.horizontal_wrapped(|ui| {
                                            ui.label(
                                                egui::RichText::new(*version)
                                                    .font(egui::FontId::monospace(10.0))
                                                    .color(p.muted),
                                            );
                                            ui.add(
                                                egui::Label::new(
                                                    egui::RichText::new(*change)
                                                        .size(11.0)
                                                        .color(p.muted),
                                                )
                                                .wrap(),
                                            );
                                        });
                                    }
                                });
                            }
                        });

                        ui.add_space(12.0);
                        temp_ui::card(ui, |ui| {
                            ui.label(
                                egui::RichText::new("Clear all data")
                                    .size(14.5)
                                    .strong()
                                    .color(p.danger),
                            );
                            ui.label(
                                egui::RichText::new(
                                    "Deletes Mail, SMS, the OTP vault, settings, the installation key and logs. Provider data and carrier billing are outside this app.",
                                )
                                .size(11.5)
                                .color(p.muted),
                            );
                            ui.add_space(8.0);
                            if self.confirm_clear {
                                ui.label(
                                    egui::RichText::new(
                                        "This cannot be undone. Restart the app afterward.",
                                    )
                                    .size(11.5)
                                    .color(p.danger),
                                );
                                ui.add_space(4.0);
                                ui.horizontal(|ui| {
                                    if temp_ui::danger_button(ui, "Delete all local data")
                                        .clicked()
                                    {
                                        self.confirm_clear = false;
                                        self.status = Some(match self.paths.clear_all() {
                                            Ok(()) => (
                                                "All local data cleared. Restart GMO TempTools to create a new installation key.".to_string(),
                                                false,
                                            ),
                                            Err(error) => (
                                                format!("Could not clear everything: {error}"),
                                                true,
                                            ),
                                        });
                                    }
                                    if temp_ui::secondary_button(ui, "Cancel").clicked() {
                                        self.confirm_clear = false;
                                    }
                                });
                            } else if temp_ui::danger_button(ui, "Clear all data…").clicked() {
                                self.confirm_clear = true;
                            }
                        });

                        ui.add_space(10.0);
                    });
            });
    }
}

impl App for TempTools {
    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        // Keep the shell repainting even on Settings, so disk changes are
        // noticed without waiting for mouse or keyboard input.
        ctx.request_repaint_after(Duration::from_millis(500));
        self.sync_settings(ctx);
        #[cfg(windows)]
        if let Some(tray) = &self.tray {
            if ctx.input(|input| input.viewport().close_requested()) {
                // Keep the process, listeners and pollers alive; only the native
                // window is hidden. Tray Quit exits the process directly.
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                tray.hide();
            }
        }
        self.last_screen = self.screen;

        self.shell(ctx);

        // Each child owns the rest of the window. Only the visible screen is
        // drawn, but every service poller runs independently of this switch.
        match self.screen {
            Screen::Mail => self.email.update(ctx, frame),
            Screen::Sms => self.sms.update(ctx, frame),
            Screen::Otp => self.otp.update(ctx, frame),
            Screen::Settings => self.settings_screen(ctx),
        }
    }
}

fn toggle_row(
    ui: &mut egui::Ui,
    title: &str,
    hint: &str,
    checked: &mut bool,
    enabled: bool,
) -> bool {
    let p = temp_ui::palette(ui.ctx());
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.label(
                egui::RichText::new(title)
                    .size(12.5)
                    .color(if enabled { p.text } else { p.muted }),
            );
            ui.label(egui::RichText::new(hint).size(10.5).color(p.muted));
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            changed = temp_ui::toggle(ui, checked, enabled).changed();
        });
    });
    changed
}

#[cfg(windows)]
fn open_folder(path: &std::path::Path) {
    let _ = std::process::Command::new("explorer").arg(path).spawn();
}

#[cfg(not(windows))]
fn open_folder(_path: &std::path::Path) {}

fn main() -> eframe::Result<()> {
    let root = local_auth::install_root();
    // Test runs can keep their fixtures away from the user's real state. The
    // packaged app never sets this variable, so normal runs stay unchanged.
    let state_root = std::env::var_os("TEMPTOOLS_STATE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| root.clone());
    let mail_port = port("TEMPTOOLS_EMAIL_PORT", 8781);
    let sms_port = port("TEMPTOOLS_SMS_PORT", 8782);
    let web_port = port("TEMPTOOLS_WEB_PORT", 8780);
    let settings_path = state_root.join("settings.json");
    let settings = Settings::load(&settings_path);
    settings.apply_notifications();
    let paths = Paths::new(root.clone(), state_root.clone(), settings_path.clone());
    let auth_token = match security::load_or_create_token(&root) {
        Ok(token) => token,
        Err(err) => {
            eprintln!("GMO TempTools could not load its install token: {err}");
            std::process::exit(1);
        }
    };

    let email = match temp_email::start_with_auth(
        mail_port,
        state_root.join("temp-email").join("state"),
        auth_token.clone(),
    ) {
        Ok(app) => app,
        Err(err) => {
            eprintln!("GMO TempTools could not start mail: {err}");
            std::process::exit(1);
        }
    };
    let sms = match temp_sms::start_with_auth(
        sms_port,
        state_root.join("temp-sms").join("state"),
        auth_token.clone(),
    ) {
        Ok(app) => app,
        Err(err) => {
            eprintln!("GMO TempTools could not start SMS: {err}");
            std::process::exit(1);
        }
    };
    if let Err(err) = webapp::start(webapp::Config {
        port: web_port,
        auth_token: auth_token.clone(),
        mail_port,
        sms_port,
        otp_vault: paths.otp_vault.clone(),
    }) {
        eprintln!("GMO TempTools could not start the web app: {err}");
        std::process::exit(1);
    }
    let otp = temp_otp::gui::OtpApp::new_with_vault(paths.otp_vault.clone());

    if std::env::var("TEMPTOOLS_HEADLESS").as_deref() == Ok("1") {
        println!("GMO TempTools: headless test mode (no native window)");
        loop {
            std::thread::sleep(std::time::Duration::from_secs(3600));
        }
    }

    println!("GMO TempTools: one window, web on {web_port}, APIs on {mail_port} and {sms_port}");
    println!("Unlock the web app with the token at {}", paths.token.display());
    println!("Close the window to keep services and polling in the tray");

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1140.0, 800.0])
            .with_min_inner_size([960.0, 620.0]),
        ..Default::default()
    };
    eframe::run_native(
        "GMO TempTools",
        options,
        Box::new(move |cc| {
            settings.appearance.apply(&cc.egui_ctx);
            let tray = match tray::Tray::new(cc) {
                Ok(tray) => Some(tray),
                Err(err) => {
                    eprintln!("GMO TempTools tray unavailable: {err}");
                    None
                }
            };
            Ok(Box::new(TempTools::new(
                email,
                sms,
                otp,
                settings,
                settings_path,
                paths,
                auth_token,
                tray,
            )))
        }),
    )
}

fn port(env_name: &str, default: u16) -> u16 {
    std::env::var(env_name)
        .ok()
        .and_then(|value| value.trim().parse::<u16>().ok())
        .filter(|value| *value != 0)
        .unwrap_or(default)
}
