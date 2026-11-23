//! The OTP desktop window: read a code out of pasted text, or generate live
//! authenticator codes. Nothing is sent anywhere; the vault is a local file.

use crate::vault::{self, StoredSecret};
use eframe::egui;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use temp_ui::{
    banner, card, chip, danger_button, empty_state, eyebrow, palette, primary_button,
    secondary_button, segmented, Tone,
};

fn now_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

#[derive(PartialEq, Clone, Copy)]
enum Tab {
    Extract,
    Generate,
}

pub struct OtpApp {
    tab: Tab,
    paste: String,
    found: Option<crate::Extracted>,
    uri_input: String,
    label_input: String,
    secrets: Vec<StoredSecret>,
    vault: std::path::PathBuf,
    error: Option<String>,
    last_sync: Instant,
}

impl OtpApp {
    pub fn new() -> Self {
        Self::new_with_vault(vault::default_path())
    }

    pub fn new_with_vault(vault: std::path::PathBuf) -> Self {
        match vault::load_result(&vault) {
            Ok(secrets) => Self {
                tab: Tab::Extract,
                paste: String::new(),
                found: None,
                uri_input: String::new(),
                label_input: String::new(),
                secrets,
                vault,
                error: None,
                last_sync: Instant::now(),
            },
            Err(error) => Self {
                tab: Tab::Extract,
                paste: String::new(),
                found: None,
                uri_input: String::new(),
                label_input: String::new(),
                secrets: Vec::new(),
                vault,
                error: Some(error),
                last_sync: Instant::now(),
            },
        }
    }

    /// Pick up a change made by the web app in the shared vault. Returns true
    /// when the in-memory list changed.
    pub fn sync_from_disk(&mut self) -> bool {
        let on_disk = match vault::load_result(&self.vault) {
            Ok(secrets) => secrets,
            Err(error) => {
                self.error = Some(error);
                return false;
            }
        };
        if on_disk != self.secrets {
            self.secrets = on_disk;
            true
        } else {
            false
        }
    }

    fn sync_if_due(&mut self) {
        if self.last_sync.elapsed() >= Duration::from_secs(5) {
            self.sync_from_disk();
            self.last_sync = Instant::now();
        }
    }

    pub fn secret_count(&self) -> usize {
        self.secrets.len()
    }

    /// Add a secret without going through the text fields. This is the shared
    /// path used by the desktop button and by the synchronization tests.
    pub fn add_secret_from_input(&mut self, raw: &str, label: &str) -> bool {
        match StoredSecret::from_input(raw, label) {
            Some(secret) => {
                let result = vault::update(&self.vault, |secrets| {
                    if !secrets
                        .iter()
                        .any(|s| s.secret == secret.secret && s.label == secret.label)
                    {
                        secrets.push(secret);
                    }
                });
                match result {
                    Ok(()) => {
                        self.sync_from_disk();
                        self.error = None;
                        true
                    }
                    Err(error) => {
                        self.error = Some(error);
                        false
                    }
                }
            }
            None => {
                self.error =
                    Some("That is not an otpauth:// URI or a base32 secret (8+ characters).".into());
                false
            }
        }
    }

    fn add_secret(&mut self) {
        let raw = self.uri_input.clone();
        let label = self.label_input.clone();
        if self.add_secret_from_input(&raw, &label) {
            self.uri_input.clear();
            self.label_input.clear();
        }
    }
}

impl eframe::App for OtpApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        ctx.request_repaint_after(Duration::from_millis(500));
        // The web app shares this vault. Read it on a short interval instead of
        // decrypting and parsing it on every repaint.
        self.sync_if_due();

        let p = palette(ctx);
        egui::TopBottomPanel::top("otp_header")
            .frame(
                egui::Frame::new()
                    .fill(p.canvas)
                    .inner_margin(egui::Margin {
                        left: 26,
                        right: 26,
                        top: 18,
                        bottom: 12,
                    }),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    let available = ui.available_width();
                    ui.vertical(|ui| {
                        ui.set_max_width((available - 280.0).max(280.0));
                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new("OTP")
                                    .size(25.0)
                                    .strong()
                                    .color(p.text),
                            );
                            chip(ui, "OFFLINE", Tone::Accent);
                        });
                        ui.add_space(1.0);
                        ui.label(
                            egui::RichText::new(
                                "Read a code out of pasted text, or generate authenticator codes.",
                            )
                            .size(12.5)
                            .color(p.muted),
                        );
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(
                            egui::RichText::new("Nothing leaves this machine")
                                .size(11.0)
                                .color(p.muted),
                        );
                    });
                });
                ui.add_space(12.0);
                if segmented(
                    ui,
                    &mut self.tab,
                    &[
                        (Tab::Extract, "Read a code"),
                        (Tab::Generate, "Authenticator"),
                    ],
                ) {
                    self.sync_from_disk();
                    self.last_sync = Instant::now();
                }
                ui.add_space(2.0);
            });

        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(p.canvas)
                    .inner_margin(egui::Margin {
                        left: 26,
                        right: 26,
                        top: 10,
                        bottom: 20,
                    }),
            )
            .show(ctx, |ui| {
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.set_max_width(860.0);
                        match self.tab {
                            Tab::Extract => self.extract_tab(ui),
                            Tab::Generate => self.generate_tab(ui),
                        }
                    });
            });
    }
}

impl OtpApp {
    fn extract_tab(&mut self, ui: &mut egui::Ui) {
        let p = palette(ui.ctx());
        let mut copied = None;
        let mut find = false;
        card(ui, |ui| {
            eyebrow(ui, "PASTE A MESSAGE");
            ui.add_space(2.0);
            ui.label(
                egui::RichText::new(
                    "Codes near words like code, OTP or verification are ranked first. Nothing is uploaded.",
                )
                .size(11.0)
                .color(p.muted),
            );
            ui.add_space(6.0);
            ui.add(
                egui::TextEdit::multiline(&mut self.paste)
                    .desired_rows(8)
                    .hint_text("Your verification code is …")
                    .desired_width(f32::INFINITY)
                    .font(egui::TextStyle::Monospace)
                    .margin(egui::Margin::symmetric(12, 10)),
            );
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if primary_button(ui, "Find codes").clicked() {
                    find = true;
                }
                if temp_ui::ghost_button(ui, "Clear").clicked() {
                    self.paste.clear();
                    self.found = None;
                }
            });
        });

        if find {
            self.found = Some(crate::extract(&self.paste));
        }

        ui.add_space(14.0);
        let links: Vec<String> = self
            .found
            .as_ref()
            .map(|found| found.links.clone())
            .unwrap_or_default();
        for code in self
            .found
            .as_ref()
            .map(|found| found.codes.clone())
            .unwrap_or_default()
        {
            card(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        eyebrow(ui, "CANDIDATE CODE");
                        ui.label(
                            egui::RichText::new(&code)
                                .monospace()
                                .size(26.0)
                                .color(p.accent),
                        );
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if secondary_button(ui, "Copy code").clicked() {
                            copied = Some(code.clone());
                        }
                    });
                });
            });
            ui.add_space(6.0);
        }

        if !links.is_empty() {
            ui.add_space(4.0);
            eyebrow(ui, "VERIFICATION LINKS");
            ui.add_space(2.0);
            for link in links {
                card(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.vertical(|ui| {
                            let short = if link.len() > 78 { &link[..78] } else { &link };
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(short)
                                        .monospace()
                                        .size(11.0)
                                        .color(p.text),
                                )
                                .truncate(),
                            );
                        });
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if secondary_button(ui, "Copy").clicked() {
                                copied = Some(link.clone());
                            }
                            if temp_ui::ghost_button(ui, "Open").clicked() {
                                ui.ctx().open_url(egui::OpenUrl::new_tab(link.clone()));
                            }
                        });
                    });
                });
                ui.add_space(6.0);
            }
        }

        if self.found.is_none() {
            ui.add_space(2.0);
            ui.vertical_centered(|ui| {
                ui.label(
                    egui::RichText::new("Results appear here after you press Find codes.")
                        .size(11.5)
                        .color(p.muted),
                );
            });
        }

        if let Some(text) = copied {
            ui.ctx().copy_text(text);
        }
    }

    fn generate_tab(&mut self, ui: &mut egui::Ui) {
        let p = palette(ui.ctx());
        let mut copied = None;
        let mut add = false;

        card(ui, |ui| {
            eyebrow(ui, "ADD AN ACCOUNT");
            ui.add_space(2.0);
            ui.label(
                egui::RichText::new(
                    "Paste an otpauth:// URI from a QR code, or a base32 secret. Stored in state/vault.json.",
                )
                .size(11.0)
                .color(p.muted),
            );
            ui.add_space(6.0);
            ui.add(
                egui::TextEdit::singleline(&mut self.uri_input)
                    .hint_text("otpauth://totp/… or JBSWY3DPEHPK3PXP")
                    .desired_width(f32::INFINITY)
                    .font(egui::TextStyle::Monospace)
                    .margin(egui::Margin::symmetric(12, 8)),
            );
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut self.label_input)
                        .hint_text("Label (optional)")
                        .desired_width(260.0)
                        .margin(egui::Margin::symmetric(12, 8)),
                );
                if primary_button(ui, "Add account").clicked() {
                    add = true;
                }
            });
        });
        if add {
            self.add_secret();
        }

        if let Some(error) = &self.error {
            ui.add_space(10.0);
            banner(ui, error, true);
        }

        ui.add_space(16.0);
        ui.horizontal(|ui| {
            eyebrow(ui, "ACCOUNTS");
            chip(ui, &self.secrets.len().to_string(), Tone::Neutral);
        });
        ui.add_space(4.0);

        if self.secrets.is_empty() {
            empty_state(
                ui,
                "AUTHENTICATOR",
                "No accounts yet",
                "Add an otpauth:// URI or a base32 secret above.\nCodes refresh in place, every second.",
            );
            return;
        }

        let now = now_secs();
        let mut remove: Option<usize> = None;
        for (index, secret) in self.secrets.iter().enumerate() {
            let code = secret.code(now);
            let left = crate::seconds_left(secret.period, now);
            card(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.label(
                            egui::RichText::new(&secret.label)
                                .size(13.5)
                                .strong()
                                .color(p.text),
                        );
                        ui.add_space(2.0);
                        ui.label(
                            egui::RichText::new(format!(
                                "{} · {} digits · {}s · {}",
                                secret.algorithm,
                                secret.digits,
                                secret.period,
                                secret.masked()
                            ))
                            .size(10.5)
                            .color(p.muted),
                        );
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if danger_button(ui, "Remove").clicked() {
                            remove = Some(index);
                        }
                        if secondary_button(ui, "Copy").clicked() {
                            copied = Some(code.clone());
                        }
                        ui.label(
                            egui::RichText::new(format!(
                                "{:0width$}",
                                code,
                                width = secret.digits as usize
                            ))
                            .monospace()
                            .size(25.0)
                            .color(p.accent),
                        );
                    });
                });
                ui.add_space(8.0);
                ui.add(
                    egui::ProgressBar::new(left as f32 / secret.period.max(1) as f32)
                        .desired_height(5.0)
                        .fill(p.accent),
                );
                ui.add_space(3.0);
                ui.label(
                    egui::RichText::new(format!("renews in {left}s"))
                        .size(10.0)
                        .color(p.muted),
                );
            });
            ui.add_space(8.0);
        }

        if let Some(index) = remove {
            if let Err(error) = vault::update(&self.vault, |secrets| {
                if index < secrets.len() {
                    secrets.remove(index);
                }
            }) {
                self.error = Some(error);
            }
            self.sync_from_disk();
        }
        if let Some(text) = copied {
            ui.ctx().copy_text(text);
        }
    }
}

pub fn theme(ctx: &egui::Context) {
    temp_ui::apply(ctx, temp_ui::Theme::Dark);
}
