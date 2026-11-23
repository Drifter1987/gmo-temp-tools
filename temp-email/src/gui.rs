//! The desktop window for temp-email.

use crate::{add_inbox, providers, util::*, Shared};
use eframe::egui;
use egui::text::{LayoutJob, TextFormat};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::time::{Duration, Instant};
use temp_ui::{
    banner, card, chip, danger_button, divider, empty_state, eyebrow, ghost_button, page_header,
    palette, primary_button, secondary_button, segmented, Tone,
};

fn clear_prior_error(status: &mut Option<(String, bool)>) {
    if status.as_ref().is_some_and(|(_, is_error)| *is_error) {
        *status = None;
    }
}

#[derive(PartialEq, Clone, Copy)]
enum Tab {
    Inboxes,
    Guide,
}

enum Job {
    Created(Result<crate::InboxCreation, String>),
    Messages(Result<Vec<providers::Brief>, String>),
    Detail(Result<providers::Detail, String>),
    Removed,
}

pub struct EmailApp {
    shared: Shared,
    tab: Tab,
    provider: providers::Provider,
    selected: Option<String>,
    briefs: Vec<providers::Brief>,
    detail: Option<providers::Detail>,
    status: Option<(String, bool)>,
    busy: bool,
    last_auto_refresh: Option<Instant>,
    rx: Receiver<Job>,
    tx: Sender<Job>,
}

impl EmailApp {
    pub(crate) fn new(shared: Shared) -> Self {
        let (tx, rx) = channel();
        Self {
            shared,
            tab: Tab::Inboxes,
            provider: providers::Provider::MailTm,
            selected: None,
            briefs: Vec::new(),
            detail: None,
            status: None,
            busy: false,
            last_auto_refresh: None,
            rx,
            tx,
        }
    }

    fn create_inbox(&mut self) {
        if self.busy {
            return;
        }
        self.busy = true;
        let shared = self.shared.clone();
        let tx = self.tx.clone();
        let provider = self.provider;
        std::thread::spawn(move || {
            let _ = tx.send(Job::Created(add_inbox(&shared, provider)));
        });
    }

    fn refresh(&mut self) {
        self.load_messages(true);
    }

    fn auto_refresh(&mut self) {
        self.load_messages(false);
    }

    fn load_messages(&mut self, fresh: bool) {
        let Some(id) = self.selected.clone() else {
            return;
        };
        if self.busy {
            return;
        }
        self.busy = true;
        self.last_auto_refresh = Some(Instant::now());
        let shared = self.shared.clone();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let result = if fresh {
                crate::refresh_account(&shared, &id)
            } else {
                crate::cached_account(&shared, &id)
            };
            let _ = tx.send(Job::Messages(result));
        });
    }

    fn open_message(&mut self, message_id: &str) {
        let Some(id) = self.selected.clone() else {
            return;
        };
        let account = {
            let state = self.shared.lock().unwrap();
            state.store.find(&id)
        };
        let Some(account) = account else {
            return;
        };
        self.busy = true;
        let tx = self.tx.clone();
        let message_id = message_id.to_string();
        std::thread::spawn(move || {
            let result = providers::detail(&account, &message_id);
            let _ = tx.send(Job::Detail(result));
        });
    }

    fn remove_inbox(&mut self, id: &str) {
        let account = {
            let state = self.shared.lock().unwrap();
            state.store.find(id)
        };
        if let Some(account) = account {
            let shared = self.shared.clone();
            let tx = self.tx.clone();
            let id = id.to_string();
            std::thread::spawn(move || {
                let _ = providers::remove(&account);
                shared.lock().unwrap().remove_inbox(&id);
                let _ = tx.send(Job::Removed);
            });
        }
    }

    fn poll(&mut self) {
        while let Ok(job) = self.rx.try_recv() {
            self.busy = false;
            match job {
                Job::Created(Ok(created)) => {
                    let message = if created.fallback {
                        let first = created
                            .failures
                            .first()
                            .map(|failure| format!("{} failed: {}", failure.provider, failure.error))
                            .unwrap_or_else(|| "the selected provider failed".to_string());
                        format!(
                            "{first}. {} created with {} instead.",
                            created.row.address, created.used_provider
                        )
                    } else {
                        format!("Inbox {} is ready.", created.row.address)
                    };
                    self.status = Some((message, false));
                    self.selected = Some(created.row.id);
                    self.refresh();
                }
                Job::Created(Err(err)) => self.status = Some((err, true)),
                Job::Messages(Ok(list)) => {
                    clear_prior_error(&mut self.status);
                    if let Some(detail) = &self.detail {
                        if !list.iter().any(|brief| brief.id == detail.id) {
                            self.detail = None;
                        }
                    }
                    self.briefs = list;
                }
                Job::Messages(Err(err)) => self.status = Some((err, true)),
                Job::Detail(Ok(detail)) => {
                    clear_prior_error(&mut self.status);
                    crate::mark_seen(&self.shared, &detail.id);
                    self.detail = Some(detail);
                }
                Job::Detail(Err(err)) => self.status = Some((err, true)),
                Job::Removed => {
                    self.selected = None;
                    self.briefs.clear();
                    self.detail = None;
                }
            }
        }
    }
}

impl eframe::App for EmailApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll();
        ctx.request_repaint_after(Duration::from_millis(500));
        self.shared.lock().unwrap().purge_expired(now_ms());

        if self.tab == Tab::Inboxes && self.selected.is_some() {
            let due = self
                .last_auto_refresh
                .map(|last| last.elapsed() >= Duration::from_secs(6))
                .unwrap_or(true);
            if due {
                self.auto_refresh();
            }
        }

        let p = palette(ctx);
        egui::TopBottomPanel::top("mail_header")
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
                        ui.set_max_width((available - 380.0).max(280.0));
                        page_header(
                            ui,
                            "Mail",
                            "Disposable inboxes and verification codes, all on this PC.",
                        );
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if self.tab == Tab::Guide {
                            if secondary_button(ui, "Back to inboxes").clicked() {
                                self.tab = Tab::Inboxes;
                            }
                        } else {
                            if primary_button(ui, "New inbox").clicked() {
                                self.create_inbox();
                            }
                            if self.busy {
                                ui.spinner();
                            }
                            egui::ComboBox::from_id_salt("provider")
                                .width(132.0)
                                .selected_text(self.provider.label())
                                .show_ui(ui, |ui| {
                                    for provider in providers::ALL {
                                        ui.selectable_value(
                                            &mut self.provider,
                                            provider,
                                            provider.label(),
                                        );
                                    }
                                });
                            ui.label(
                                egui::RichText::new("Provider")
                                    .size(11.0)
                                    .color(p.muted),
                            );
                        }
                    });
                });
                ui.add_space(12.0);
                if segmented(
                    ui,
                    &mut self.tab,
                    &[(Tab::Inboxes, "Inboxes"), (Tab::Guide, "How it works")],
                ) && self.tab == Tab::Inboxes
                    && self.selected.is_none()
                {
                    self.refresh();
                }
                ui.add_space(2.0);
            });

        if self.tab == Tab::Guide {
            egui::CentralPanel::default()
                .frame(
                    egui::Frame::new()
                        .fill(p.canvas)
                        .inner_margin(egui::Margin {
                            left: 26,
                            right: 26,
                            top: 8,
                            bottom: 20,
                        }),
                )
                .show(ctx, |ui| self.guide(ui));
            return;
        }

        egui::SidePanel::left("inbox_list")
            .resizable(false)
            .exact_width(298.0)
            .frame(
                egui::Frame::new()
                    .fill(p.canvas)
                    .inner_margin(egui::Margin {
                        left: 20,
                        right: 16,
                        top: 10,
                        bottom: 14,
                    }),
            )
            .show(ctx, |ui| {
                let accounts = {
                    let state = self.shared.lock().unwrap();
                    state
                        .store
                        .accounts
                        .iter()
                        .map(|account| {
                            let last = state
                                .lists
                                .get(&account.row.id)
                                .and_then(|(_, list)| {
                                    list.iter().max_by(|a, b| {
                                        a.received_at
                                            .partial_cmp(&b.received_at)
                                            .unwrap_or(std::cmp::Ordering::Equal)
                                    })
                                })
                                .map(|brief| {
                                    if brief.subject.is_empty() {
                                        "(no subject)".to_string()
                                    } else {
                                        brief.subject.clone()
                                    }
                                });
                            (
                                account.row.clone(),
                                state.cached_unread(&account.row.id).unwrap_or(0),
                                last,
                            )
                        })
                        .collect::<Vec<_>>()
                };

                ui.horizontal(|ui| {
                    eyebrow(ui, "INBOXES");
                    if !accounts.is_empty() {
                        chip(ui, &accounts.len().to_string(), Tone::Neutral);
                    }
                });
                ui.add_space(4.0);

                if accounts.is_empty() {
                    ui.add_space(6.0);
                    ui.label(
                        egui::RichText::new("No inboxes yet. Create one from the header.")
                            .size(11.5)
                            .color(p.muted),
                    );
                }

                egui::ScrollArea::vertical()
                    .id_salt("inbox_scroll")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        for (row, unread, last) in &accounts {
                            let selected = self.selected.as_deref() == Some(row.id.as_str());
                            let text = inbox_label(ui, row, *unread, last.as_deref());
                            if ui.selectable_label(selected, text).clicked() {
                                self.selected = Some(row.id.clone());
                                self.briefs.clear();
                                self.detail = None;
                                self.refresh();
                            }
                            ui.add_space(2.0);
                        }

                        let (health, active) = {
                            let state = self.shared.lock().unwrap();
                            let mut active = std::collections::HashMap::new();
                            for account in &state.store.accounts {
                                *active.entry(account.row.provider.clone()).or_insert(0_u64) += 1;
                            }
                            (crate::provider_health_rows(&state, now_ms()), active)
                        };
                        ui.add_space(12.0);
                        temp_ui::divider(ui);
                        egui::CollapsingHeader::new(
                            egui::RichText::new("Provider health")
                                .size(11.5)
                                .color(p.muted),
                        )
                        .default_open(false)
                        .show(ui, |ui| {
                            for provider in providers::ALL {
                                let row =
                                    health.iter().find(|row| row.provider == provider.slug());
                                let inboxes = active.get(provider.slug()).copied().unwrap_or(0);
                                let legacy_failures =
                                    row.map(|row| row.list_failures).unwrap_or(0);
                                let create_failures =
                                    row.map(|row| row.create_failures).unwrap_or(0);
                                let current_error = row
                                    .filter(|row| row.error_is_current)
                                    .map(|row| row.last_error.as_str())
                                    .unwrap_or("");
                                let history = legacy_failures + create_failures;
                                let last_success = row.map(|row| row.last_success_at).unwrap_or(0.0);
                                let (label, color) = if inboxes == 0 && history == 0 {
                                    ("unused", p.muted)
                                } else if !current_error.is_empty() {
                                    ("issue now", p.danger)
                                } else if history > 0 {
                                    ("ok · history", p.accent)
                                } else {
                                    ("ok", p.accent)
                                };
                                ui.add_space(2.0);
                                ui.horizontal_wrapped(|ui| {
                                    ui.label(
                                        egui::RichText::new(format!(
                                            "{} · {} · {} inbox{}",
                                            provider.label(),
                                            label,
                                            inboxes,
                                            if inboxes == 1 { "" } else { "es" }
                                        ))
                                        .size(10.5)
                                        .color(color),
                                    )
                                    .on_hover_text(provider.address_rule());
                                });
                                if !current_error.is_empty() {
                                    ui.label(
                                        egui::RichText::new(preview(current_error, 70))
                                            .color(p.danger)
                                            .size(10.0),
                                    );
                                } else if history > 0 {
                                    ui.label(
                                        egui::RichText::new(format!(
                                            "{} create / {} list failures · last success {}",
                                            create_failures,
                                            legacy_failures,
                                            since(last_success)
                                        ))
                                        .color(p.muted)
                                        .size(10.0),
                                    );
                                }
                            }
                        });
                    });
            });

        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(p.canvas)
                    .inner_margin(egui::Margin {
                        left: 22,
                        right: 26,
                        top: 8,
                        bottom: 16,
                    }),
            )
            .show(ctx, |ui| {
                if let Some((message, is_error)) = &self.status {
                    banner(ui, message, *is_error);
                    ui.add_space(8.0);
                }

                let Some(id) = self.selected.clone() else {
                    empty_state(
                        ui,
                        "MAIL",
                        "No inbox selected",
                        "Create an inbox, then paste its address into a signup form.\nCodes are pulled out of the mail automatically.",
                    );
                    ui.add_space(18.0);
                    ui.vertical_centered(|ui| {
                        if primary_button(ui, "Create your first inbox").clicked() {
                            self.create_inbox();
                        }
                    });
                    return;
                };
                let (account, unread) = {
                    let state = self.shared.lock().unwrap();
                    (state.store.find(&id), state.cached_unread(&id).unwrap_or(0))
                };
                let Some(account) = account else {
                    self.selected = None;
                    return;
                };

                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new(&account.row.address)
                                    .monospace()
                                    .size(18.0)
                                    .strong()
                                    .color(p.text),
                            );
                            if unread > 0 {
                                chip(
                                    ui,
                                    &format!("{unread} unread"),
                                    Tone::Accent,
                                );
                            }
                        });
                        ui.add_space(3.0);
                        ui.horizontal(|ui| {
                            chip(ui, &account.row.provider, Tone::Neutral);
                            chip(
                                ui,
                                &format!("expires in {}", ttl(account.row.expires_at)),
                                Tone::Neutral,
                            );
                        });
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if danger_button(ui, "Delete").clicked() {
                            self.remove_inbox(&id);
                        }
                        if secondary_button(ui, "Refresh").clicked() {
                            self.refresh();
                        }
                        if secondary_button(ui, "Copy").clicked() {
                            ui.ctx().copy_text(account.row.address.clone());
                        }
                    });
                });

                ui.add_space(10.0);
                divider(ui);

                ui.horizontal(|ui| {
                    eyebrow(ui, "MESSAGES");
                    if !self.briefs.is_empty() {
                        chip(ui, &self.briefs.len().to_string(), Tone::Neutral);
                    }
                });
                ui.add_space(2.0);

                if self.briefs.is_empty() {
                    ui.add_space(6.0);
                    ui.label(
                        egui::RichText::new("No messages yet. New mail appears here on its own.")
                            .size(12.0)
                            .color(p.muted),
                    );
                }

                let list_height = (ui.available_height() * 0.30).clamp(110.0, 210.0);
                egui::ScrollArea::vertical()
                    .id_salt("mail_messages")
                    .max_height(list_height)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        for brief in self.briefs.clone() {
                            let unread = !self
                                .shared
                                .lock()
                                .unwrap()
                                .is_message_seen(&brief);
                            let selected = self.detail.as_ref().map(|d| d.id.as_str())
                                == Some(brief.id.as_str());
                            let text = message_label(ui, &brief, unread);
                            if ui.selectable_label(selected, text).clicked() {
                                self.open_message(&brief.id);
                            }
                            ui.add_space(2.0);
                        }
                    });

                ui.add_space(6.0);
                divider(ui);

                if let Some(detail) = &self.detail {
                    let mut copied = None;
                    egui::ScrollArea::vertical()
                        .id_salt("mail_reader_pane")
                        .max_height(ui.available_height())
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            card(ui, |ui| {
                                ui.label(
                                    egui::RichText::new(if detail.subject.is_empty() {
                                        "(no subject)"
                                    } else {
                                        &detail.subject
                                    })
                                    .size(17.0)
                                    .strong()
                                    .color(p.text),
                                );
                                ui.add_space(2.0);
                                ui.horizontal_wrapped(|ui| {
                                    ui.label(
                                        egui::RichText::new(&detail.from.address)
                                            .size(11.5)
                                            .color(p.muted),
                                    );
                                    ui.label(
                                        egui::RichText::new("·")
                                            .size(11.5)
                                            .color(p.line),
                                    );
                                    ui.label(
                                        egui::RichText::new(format!(
                                            "received {}",
                                            since(detail.received_at)
                                        ))
                                        .size(11.5)
                                        .color(p.muted),
                                    );
                                });
                                ui.add_space(8.0);

                                let text = detail
                                    .text
                                    .clone()
                                    .or_else(|| detail.html.clone())
                                    .unwrap_or_default();
                                let text = strip_tags(&text);
                                if let Some(code) = extract_code(&text) {
                                    if temp_ui::code_card(
                                        ui,
                                        "VERIFICATION CODE",
                                        &code,
                                        "Copy code",
                                    ) {
                                        copied = Some(code.clone());
                                    }
                                    ui.add_space(8.0);
                                }
                                if let Some(link) =
                                    extract_link(&detail.html.clone().unwrap_or_default())
                                        .or_else(|| extract_link(&text))
                                {
                                    ui.horizontal(|ui| {
                                        eyebrow(ui, "LINK");
                                        if secondary_button(ui, "Copy link").clicked() {
                                            copied = Some(link.clone());
                                        }
                                        if ghost_button(ui, "Open").clicked() {
                                            ui.ctx()
                                                .open_url(egui::OpenUrl::new_tab(link.clone()));
                                        }
                                    });
                                    ui.add_space(6.0);
                                }
                                ui.add(
                                    egui::Label::new(
                                        egui::RichText::new(&text)
                                            .size(12.5)
                                            .color(p.text)
                                            .line_height(Some(19.0)),
                                    )
                                    .wrap(),
                                );
                            });
                        });
                    if let Some(code) = copied {
                        ui.ctx().copy_text(code);
                    }
                } else {
                    ui.add_space(16.0);
                    ui.vertical_centered(|ui| {
                        ui.label(
                            egui::RichText::new("Select a message to read it.")
                                .size(12.0)
                                .color(p.muted),
                        );
                    });
                }
            });
    }
}

impl EmailApp {
    fn guide(&mut self, ui: &mut egui::Ui) {
        let p = palette(ui.ctx());
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.set_max_width(780.0);
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new("How Temp Mail works")
                        .size(20.0)
                        .strong()
                        .color(p.text),
                );
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new(
                        "This window creates a real mailbox at the provider you pick and keeps its credentials in this folder. The app fetches new mail itself, so a verification code appears without opening a browser.",
                    )
                    .size(12.5)
                    .color(p.muted),
                );
                ui.add_space(14.0);

                card(ui, |ui| {
                    ui.label(egui::RichText::new("What you get").size(14.0).strong());
                    ui.add_space(6.0);
                    bullet(ui, "A throwaway address you can paste into a signup form.");
                    bullet(
                        ui,
                        &format!(
                            "{} providers to choose from when one domain is blocked.",
                            providers::ALL.len()
                        ),
                    );
                    bullet(ui, "Codes and verification links pulled out of each message.");
                    bullet(ui, "Unread badges that clear as soon as you open a message.");
                });
                ui.add_space(10.0);

                card(ui, |ui| {
                    ui.label(egui::RichText::new("Limits").size(14.0).strong());
                    ui.add_space(6.0);
                    bullet(
                        ui,
                        "Some services refuse disposable domains. That is the sender's rule, not a bug.",
                    );
                    bullet(
                        ui,
                        "Provider quotas differ: Best-TempMail allows 3 free inboxes a day, Catchmail 1 request a second, and mail.tm is rate-limited.",
                    );
                    bullet(ui, "Lists are cached briefly; Refresh bypasses the cache.");
                    bullet(ui, "Inboxes and their mail are deleted an hour after creation.");
                });
                ui.add_space(10.0);

                card(ui, |ui| {
                    ui.label(egui::RichText::new("Privacy").size(14.0).strong());
                    ui.add_space(6.0);
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(
                                "Whatever credential the provider needs — a password, token, or just the address — is stored in state/inboxes.json in this folder. Do not use a disposable inbox for anything you need to recover later.",
                            )
                            .size(12.5)
                            .color(p.muted),
                        )
                        .wrap(),
                    );
                });
            });
    }
}

fn bullet(ui: &mut egui::Ui, text: &str) {
    let p = palette(ui.ctx());
    ui.horizontal_wrapped(|ui| {
        ui.label(egui::RichText::new("•").color(p.accent));
        ui.add(egui::Label::new(egui::RichText::new(text).size(12.5)).wrap());
    });
}

fn since(timestamp: f64) -> String {
    if timestamp <= 0.0 {
        return "never".to_string();
    }
    let seconds = ((now_ms() - timestamp) / 1000.0).max(0.0) as i64;
    if seconds < 60 {
        format!("{seconds}s ago")
    } else if seconds < 3600 {
        format!("{}m ago", seconds / 60)
    } else {
        format!("{}h ago", seconds / 3600)
    }
}

fn ttl(expires_at: f64) -> String {
    let mins = ((expires_at - now_ms()) / 60_000.0).ceil().max(0.0) as i64;
    if mins >= 60 {
        format!("{}h {:02}m", mins / 60, mins % 60)
    } else {
        format!("{mins}m")
    }
}

fn inbox_label(
    ui: &egui::Ui,
    row: &crate::store::InboxRow,
    unread: usize,
    last: Option<&str>,
) -> LayoutJob {
    let p = palette(ui.ctx());
    let mut job = LayoutJob::default();
    job.wrap.max_width = (ui.available_width() - 18.0).max(120.0);
    job.append(
        &row.address,
        0.0,
        TextFormat {
            font_id: egui::FontId::monospace(12.0),
            color: p.text,
            ..Default::default()
        },
    );
    if unread > 0 {
        job.append(
            &format!("  {unread} unread"),
            0.0,
            TextFormat {
                font_id: egui::FontId::proportional(10.0),
                color: p.accent,
                background: p.accent_soft,
                ..Default::default()
            },
        );
    }
    job.append(
        &format!("\n{} · expires in {}", row.provider, ttl(row.expires_at)),
        0.0,
        TextFormat {
            font_id: egui::FontId::proportional(10.5),
            color: p.muted,
            ..Default::default()
        },
    );
    job.append(
        &format!("\n{}", last.unwrap_or("Waiting for the first message")),
        0.0,
        TextFormat {
            font_id: egui::FontId::proportional(11.5),
            color: if unread > 0 { p.text } else { p.muted },
            ..Default::default()
        },
    );
    job
}

fn message_label(ui: &egui::Ui, brief: &providers::Brief, unread: bool) -> LayoutJob {
    let p = palette(ui.ctx());
    let mut job = LayoutJob::default();
    job.wrap.max_width = (ui.available_width() - 18.0).max(120.0);
    let title = if brief.subject.is_empty() {
        "(no subject)"
    } else {
        &brief.subject
    };
    job.append(
        if unread { "● " } else { "" },
        0.0,
        TextFormat {
            font_id: egui::FontId::proportional(9.0),
            color: p.accent,
            ..Default::default()
        },
    );
    job.append(
        title,
        0.0,
        TextFormat {
            font_id: egui::FontId::proportional(12.5),
            color: if unread { p.text } else { p.muted },
            ..Default::default()
        },
    );
    job.append(
        &format!("\n{} · {}", brief.from.address, brief.created_at),
        0.0,
        TextFormat {
            font_id: egui::FontId::proportional(10.5),
            color: p.muted,
            ..Default::default()
        },
    );
    job
}

#[cfg(test)]
mod tests {
    use super::clear_prior_error;

    #[test]
    fn a_successful_load_clears_only_an_earlier_error() {
        let mut error = Some(("provider timed out".to_string(), true));
        clear_prior_error(&mut error);
        assert!(error.is_none());

        let mut notice = Some(("Inbox is ready.".to_string(), false));
        clear_prior_error(&mut notice);
        assert_eq!(
            notice.as_ref().map(|(text, _)| text.as_str()),
            Some("Inbox is ready.")
        );
    }
}

pub fn theme(ctx: &egui::Context) {
    temp_ui::apply(ctx, temp_ui::Theme::Dark);
}
