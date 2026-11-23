//! The desktop window for temp-sms.

use crate::{merged, now_ms, scrape, store, util::*, Shared};
use eframe::egui;
use std::sync::mpsc::{channel, Receiver, Sender};
use temp_ui::{
    banner, card, chip, danger_button, divider, empty_state, eyebrow, palette, primary_button,
    secondary_button, segmented, Tone,
};

fn clear_prior_error(status: &mut Option<(String, bool)>) {
    if status.as_ref().is_some_and(|(_, is_error)| *is_error) {
        *status = None;
    }
}

#[derive(PartialEq, Clone, Copy)]
enum Tab {
    Numbers,
    Webhook,
}

enum Job {
    Added(Result<String, String>),
    Messages(Result<Vec<scrape::ScrapedMessage>, String>),
    Countries(Result<Vec<String>, String>),
    Available(Result<Vec<String>, String>),
}

pub struct SmsApp {
    shared: Shared,
    tab: Tab,
    selected: Option<String>,
    messages: Vec<scrape::ScrapedMessage>,
    status: Option<(String, bool)>,
    busy: bool,
    countries: Vec<String>,
    country: Option<String>,
    available: Vec<String>,
    chosen_number: Option<String>,
    countries_pending: bool,
    numbers_pending: bool,
    rx: Receiver<Job>,
    tx: Sender<Job>,
}

impl SmsApp {
    pub(crate) fn new(shared: Shared) -> Self {
        let (tx, rx) = channel();
        let mut app = Self {
            shared,
            tab: Tab::Numbers,
            selected: None,
            messages: Vec::new(),
            status: None,
            busy: false,
            countries: Vec::new(),
            country: None,
            available: Vec::new(),
            chosen_number: None,
            countries_pending: false,
            numbers_pending: false,
            rx,
            tx,
        };
        app.load_countries();
        app
    }

    fn load_countries(&mut self) {
        if self.countries_pending {
            return;
        }
        self.countries_pending = true;
        let shared = self.shared.clone();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let result = {
                let mut state = shared.lock().unwrap();
                crate::country_list(&mut state, true)
            };
            let _ = tx.send(Job::Countries(result));
        });
    }

    fn load_numbers(&mut self) {
        let Some(country) = self.country.clone() else {
            return;
        };
        if self.numbers_pending {
            return;
        }
        self.numbers_pending = true;
        self.available.clear();
        self.chosen_number = None;
        let shared = self.shared.clone();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let result = {
                let mut state = shared.lock().unwrap();
                crate::numbers_for_country(&mut state, &country)
            };
            let _ = tx.send(Job::Available(result));
        });
    }

    fn refresh(&mut self) {
        let Some(id) = self.selected.clone() else {
            return;
        };
        if self.busy {
            return;
        }
        self.busy = true;
        let shared = self.shared.clone();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let result = (|| -> Result<Vec<scrape::ScrapedMessage>, String> {
                let mut state = shared.lock().unwrap();
                state.store.purge(now_ms());
                let card = state
                    .store
                    .find(&id)
                    .ok_or_else(|| "That number is gone.".to_string())?;
                Ok(merged(&mut state, &card, true))
            })();
            let _ = tx.send(Job::Messages(result));
        });
    }

    fn add_number(&mut self) {
        if self.busy {
            return;
        }
        let Some(country) = self.country.clone() else {
            self.status = Some(("Choose a country first.".to_string(), true));
            return;
        };
        let Some(number) = self.chosen_number.clone() else {
            self.status = Some(("Choose a public number first.".to_string(), true));
            return;
        };
        let Some(e164) = normalize_e164(&number) else {
            self.status = Some(("That public number is not valid.".to_string(), true));
            return;
        };
        self.busy = true;
        self.tab = Tab::Numbers;
        let shared = self.shared.clone();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let result = (|| -> Result<String, String> {
                let now = now_ms();
                let mut state = shared.lock().unwrap();
                state.store.purge(now);
                state.store.push_unique(store::Card {
                    row: store::NumberRow {
                        id: rand_hex(20),
                        owner_id: "local".to_string(),
                        e164: e164.clone(),
                        country: country.clone(),
                        provider: "receive-sms.io".to_string(),
                        created_at: now,
                        expires_at: now + crate::TTL_MS,
                    },
                    slug: country,
                });
                Ok(e164)
            })();
            let _ = tx.send(Job::Added(result));
        });
    }

    fn remove(&mut self, id: &str) {
        self.shared.lock().unwrap().store.remove(id);
        self.selected = None;
        self.messages.clear();
    }

    fn poll(&mut self) {
        while let Ok(job) = self.rx.try_recv() {
            match job {
                Job::Added(Ok(e164)) => {
                    self.busy = false;
                    self.status = Some((format!("Now watching {e164}."), false));
                    self.selected = {
                        let state = self.shared.lock().unwrap();
                        state
                            .store
                            .cards
                            .iter()
                            .find(|c| c.row.e164 == e164)
                            .map(|c| c.row.id.clone())
                    };
                    self.refresh();
                }
                Job::Added(Err(err)) => {
                    self.busy = false;
                    self.status = Some((err, true));
                }
                Job::Messages(Ok(list)) => {
                    self.busy = false;
                    clear_prior_error(&mut self.status);
                    self.messages = list;
                }
                Job::Messages(Err(err)) => {
                    self.busy = false;
                    self.status = Some((err, true));
                }
                Job::Countries(Ok(countries)) => {
                    self.countries_pending = false;
                    clear_prior_error(&mut self.status);
                    self.countries = countries;
                    let still_listed = self
                        .country
                        .as_ref()
                        .is_some_and(|country| self.countries.contains(country));
                    if !still_listed {
                        self.country = self.countries.first().cloned();
                    }
                    self.load_numbers();
                }
                Job::Countries(Err(err)) => {
                    self.countries_pending = false;
                    self.status = Some((err, true));
                }
                Job::Available(Ok(numbers)) => {
                    self.numbers_pending = false;
                    clear_prior_error(&mut self.status);
                    self.available = numbers;
                }
                Job::Available(Err(err)) => {
                    self.numbers_pending = false;
                    self.status = Some((err, true));
                }
            }
        }
    }
}

impl eframe::App for SmsApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll();
        ctx.request_repaint_after(std::time::Duration::from_millis(500));
        let source_health = self.shared.lock().unwrap().health.snapshot();
        let p = palette(ctx);

        egui::TopBottomPanel::top("sms_header")
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
                        ui.set_max_width((available - 390.0).max(280.0));
                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new("SMS")
                                    .size(25.0)
                                    .strong()
                                    .color(p.text),
                            );
                            chip(ui, "BETA", Tone::Warn);
                        });
                        ui.add_space(1.0);
                        ui.label(
                            egui::RichText::new(
                                "Public numbers from receive-sms.io, with the code already pulled out.",
                            )
                            .size(12.5)
                            .color(p.muted),
                        );
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if secondary_button(ui, "Refresh source").clicked() {
                            self.load_countries();
                        }
                        if self.busy || self.countries_pending || self.numbers_pending {
                            ui.spinner();
                        }
                        let tone = match source_health.status.as_str() {
                            "ok" => Tone::Accent,
                            "down" => Tone::Danger,
                            _ => Tone::Warn,
                        };
                        chip(
                            ui,
                            &format!("receive-sms.io · {}", source_health.status),
                            tone,
                        );
                    });
                });
                ui.add_space(12.0);
                segmented(
                    ui,
                    &mut self.tab,
                    &[(Tab::Numbers, "Numbers"), (Tab::Webhook, "Webhook")],
                );
                ui.add_space(2.0);
            });

        if self.tab == Tab::Webhook {
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
                .show(ctx, |ui| self.webhook_tab(ui));
            return;
        }

        egui::TopBottomPanel::top("sms_picker")
            .frame(
                egui::Frame::new()
                    .fill(p.canvas)
                    .inner_margin(egui::Margin {
                        left: 22,
                        right: 26,
                        top: 2,
                        bottom: 12,
                    }),
            )
            .show(ctx, |ui| {
                card(ui, |ui| {
                    self.number_picker(ui, &source_health);
                });
            });

        egui::SidePanel::left("saved_numbers")
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
                let cards = {
                    let state = self.shared.lock().unwrap();
                    state
                        .store
                        .cards
                        .iter()
                        .map(|c| c.row.clone())
                        .collect::<Vec<_>>()
                };
                ui.horizontal(|ui| {
                    eyebrow(ui, "SAVED NUMBERS");
                    if !cards.is_empty() {
                        chip(ui, &cards.len().to_string(), Tone::Neutral);
                    }
                });
                ui.add_space(4.0);
                if cards.is_empty() {
                    ui.label(
                        egui::RichText::new("No numbers yet. Pick one above.")
                            .size(11.5)
                            .color(p.muted),
                    );
                }
                egui::ScrollArea::vertical()
                    .id_salt("saved_numbers_scroll")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        for row in cards {
                            let selected = self.selected.as_deref() == Some(row.id.as_str());
                            let text = number_label(ui, &row);
                            if ui.selectable_label(selected, text).clicked() {
                                self.selected = Some(row.id.clone());
                                self.messages.clear();
                                self.refresh();
                            }
                            ui.add_space(2.0);
                        }
                        ui.add_space(12.0);
                        divider(ui);
                        ui.label(
                            egui::RichText::new(
                                "These are public shared inboxes. Anyone who opens the same page sees the same texts.",
                            )
                            .size(10.5)
                            .color(p.muted),
                        );
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
                        "SMS",
                        "No number selected",
                        "Add one of the public inboxes above.\nIts texts and verification code appear here.",
                    );
                    return;
                };
                let card_row = {
                    let state = self.shared.lock().unwrap();
                    state.store.find(&id)
                };
                let Some(card_row) = card_row else {
                    self.selected = None;
                    return;
                };

                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new(&card_row.row.e164)
                                    .monospace()
                                    .size(18.0)
                                    .strong()
                                    .color(p.text),
                            );
                            chip(ui, "BETA", Tone::Warn);
                        });
                        ui.add_space(3.0);
                        ui.horizontal(|ui| {
                            chip(
                                ui,
                                &country_label(&card_row.row.country),
                                Tone::Neutral,
                            );
                            chip(
                                ui,
                                &format!("expires in {}", ttl(card_row.row.expires_at)),
                                Tone::Neutral,
                            );
                        });
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if danger_button(ui, "Delete").clicked() {
                            self.remove(&id);
                        }
                        if secondary_button(ui, "Refresh").clicked() {
                            self.refresh();
                        }
                        if secondary_button(ui, "Copy").clicked() {
                            ui.ctx().copy_text(card_row.row.e164.clone());
                        }
                    });
                });

                ui.add_space(10.0);
                divider(ui);
                ui.horizontal(|ui| {
                    eyebrow(ui, "TEXTS");
                    if !self.messages.is_empty() {
                        chip(ui, &self.messages.len().to_string(), Tone::Neutral);
                    }
                });
                ui.add_space(2.0);

                if self.messages.is_empty() {
                    ui.add_space(8.0);
                    ui.label(
                        egui::RichText::new("No texts yet. New deliveries appear here on their own.")
                            .size(12.0)
                            .color(p.muted),
                    );
                    return;
                }

                let mut copied = None;
                egui::ScrollArea::vertical()
                    .id_salt("sms_messages")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        for message in &self.messages {
                            card(ui, |ui| {
                                ui.horizontal(|ui| {
                                    ui.label(
                                        egui::RichText::new(&message.sender)
                                            .size(12.5)
                                            .strong()
                                            .color(p.text),
                                    );
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            ui.label(
                                                egui::RichText::new(&message.when)
                                                    .size(10.5)
                                                    .color(p.muted),
                                            );
                                        },
                                    );
                                });
                                ui.add_space(4.0);
                                ui.add(
                                    egui::Label::new(
                                        egui::RichText::new(&message.body)
                                            .size(12.5)
                                            .color(p.text)
                                            .line_height(Some(19.0)),
                                    )
                                    .wrap(),
                                );
                                if let Some(code) = extract_code(&message.body) {
                                    ui.add_space(8.0);
                                    if temp_ui::code_card(
                                        ui,
                                        "CODE FOUND",
                                        &code,
                                        "Copy code",
                                    ) {
                                        copied = Some(code.clone());
                                    }
                                }
                            });
                            ui.add_space(8.0);
                        }
                    });
                if let Some(code) = copied {
                    ui.ctx().copy_text(code);
                }
            });
    }
}

impl SmsApp {
    fn number_picker(&mut self, ui: &mut egui::Ui, health: &crate::health::SourceHealth) {
        let p = palette(ui.ctx());
        ui.horizontal(|ui| {
            eyebrow(ui, "ADD A PUBLIC NUMBER");
            if health.last_source == "cache" {
                chip(ui, "last good list", Tone::Warn);
            }
        });
        ui.add_space(2.0);
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.label(
                    egui::RichText::new("Country")
                        .size(10.5)
                        .color(p.muted),
                );
                let old_country = self.country.clone();
                let mut current_country = old_country.clone().unwrap_or_default();
                egui::ComboBox::from_id_salt("sms-country")
                    .width(190.0)
                    .selected_text(if current_country.is_empty() {
                        "Choose country".to_string()
                    } else {
                        country_label(&current_country)
                    })
                    .show_ui(ui, |ui| {
                        if self.countries.is_empty() {
                            ui.label(
                                egui::RichText::new("Loading source countries…")
                                    .color(p.muted),
                            );
                        }
                        for slug in &self.countries {
                            if ui
                                .selectable_label(
                                    &current_country == slug,
                                    country_label(slug),
                                )
                                .clicked()
                            {
                                current_country = slug.clone();
                            }
                        }
                    });
                if !current_country.is_empty()
                    && Some(&current_country) != old_country.as_ref()
                {
                    self.country = Some(current_country);
                    self.load_numbers();
                }
            });
            ui.add_space(10.0);
            ui.vertical(|ui| {
                ui.label(
                    egui::RichText::new("Public number")
                        .size(10.5)
                        .color(p.muted),
                );
                let mut current_number = self.chosen_number.clone().unwrap_or_default();
                egui::ComboBox::from_id_salt("sms-number")
                    .width(190.0)
                    .selected_text(if current_number.is_empty() {
                        "Choose number".to_string()
                    } else {
                        format!("+{current_number}")
                    })
                    .show_ui(ui, |ui| {
                        if self.available.is_empty() {
                            ui.label(
                                egui::RichText::new(
                                    "No numbers listed for this country yet.",
                                )
                                .color(p.muted),
                            );
                        }
                        for digits in &self.available {
                            if ui
                                .selectable_label(
                                    &current_number == digits,
                                    format!("+{digits}"),
                                )
                                .clicked()
                            {
                                current_number = digits.clone();
                            }
                        }
                    });
                if Some(&current_number) != self.chosen_number.as_ref() {
                    self.chosen_number = Some(current_number);
                }
            });
            ui.add_space(10.0);
            ui.vertical(|ui| {
                ui.add_space(17.0);
                if primary_button(ui, "Add selected number").clicked() {
                    self.add_number();
                }
            });
            if self.numbers_pending {
                ui.add_space(4.0);
                ui.spinner();
            }
        });

        ui.add_space(6.0);
        if health.last_source == "cache" {
            ui.label(
                egui::RichText::new(format!(
                    "Source unavailable · showing the last good list. {}",
                    preview(&health.last_error, 110)
                ))
                .size(10.5)
                .color(p.warn),
            );
        } else if !health.last_error.is_empty() {
            ui.label(
                egui::RichText::new(format!(
                    "Source {} · {}",
                    health.status,
                    preview(&health.last_error, 110)
                ))
                .size(10.5)
                .color(p.muted),
            );
        } else {
            ui.label(
                egui::RichText::new(format!(
                    "{} countries listed · public numbers are shared inboxes.",
                    self.countries.len()
                ))
                .size(10.5)
                .color(p.muted),
            );
        }
    }

    fn webhook_tab(&mut self, ui: &mut egui::Ui) {
        let p = palette(ui.ctx());
        let has_twilio = configured("TWILIO_AUTH_TOKEN");
        let has_telnyx = configured("TELNYX_PUBLIC_KEY");
        let has_secret = configured("WEBHOOK_SECRET");

        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.set_max_width(820.0);
                ui.add_space(2.0);
                ui.label(
                    egui::RichText::new("Carrier webhook")
                        .size(20.0)
                        .strong()
                        .color(p.text),
                );
                ui.add_space(3.0);
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(
                            "A number you rent yourself can post its texts here. The endpoint is POST /api/inbound/sms (or /api/inbound); the payload decides the carrier.",
                        )
                        .size(12.5)
                        .color(p.muted),
                    )
                    .wrap(),
                );
                ui.add_space(12.0);

                card(ui, |ui| {
                    ui.label(egui::RichText::new("Contracts").size(14.0).strong());
                    ui.add_space(4.0);
                    contract_row(ui, "Twilio", "TWILIO_AUTH_TOKEN", has_twilio);
                    contract_row(ui, "Telnyx", "TELNYX_PUBLIC_KEY", has_telnyx);
                    contract_row(ui, "Anything else", "WEBHOOK_SECRET", has_secret);
                    ui.add_space(6.0);
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(
                                "An unset contract answers 503 instead of accepting an unsigned delivery. Start the app with the variables set; the status above reflects this process.",
                            )
                            .size(11.0)
                            .color(p.muted),
                        )
                        .wrap(),
                    );
                });
                ui.add_space(10.0);

                card(ui, |ui| {
                    ui.label(
                        egui::RichText::new("How a delivery is proven")
                            .size(14.0)
                            .strong(),
                    );
                    ui.add_space(6.0);
                    bullet(
                        ui,
                        "Twilio: HMAC-SHA1 over the full public URL and the sorted form fields.",
                    );
                    bullet(
                        ui,
                        "Telnyx: Ed25519 over “timestamp|body”, timestamp within five minutes.",
                    );
                    bullet(
                        ui,
                        "Anything else: ?k=<secret> or the x-gmo-secret header, compared in constant time.",
                    );
                    bullet(
                        ui,
                        "A retry is matched only when the carrier sends a message id; a generic payload without one is stored again.",
                    );
                });
                ui.add_space(10.0);

                card(ui, |ui| {
                    ui.label(egui::RichText::new("Behind a proxy").size(14.0).strong());
                    ui.add_space(6.0);
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(
                                "Set TWILIO_WEBHOOK_URL to the exact public URL when a proxy rewrites Host or X-Forwarded-Proto; otherwise the signature will not match.",
                            )
                            .size(12.0)
                            .color(p.muted),
                        )
                        .wrap(),
                    );
                });
            });
    }
}

fn configured(name: &str) -> bool {
    std::env::var(name)
        .map(|value| !value.trim().is_empty())
        .unwrap_or(false)
}

fn contract_row(ui: &mut egui::Ui, carrier: &str, env: &str, ready: bool) {
    let p = palette(ui.ctx());
    ui.horizontal_wrapped(|ui| {
        ui.label(
            egui::RichText::new(carrier)
                .strong()
                .size(12.5)
                .color(p.text),
        );
        ui.label(
            egui::RichText::new(env)
                .monospace()
                .size(11.0)
                .color(p.muted),
        );
        chip(
            ui,
            if ready { "configured" } else { "not set" },
            if ready { Tone::Accent } else { Tone::Neutral },
        );
    });
}

fn bullet(ui: &mut egui::Ui, text: &str) {
    let p = palette(ui.ctx());
    ui.horizontal_wrapped(|ui| {
        ui.label(egui::RichText::new("•").color(p.accent));
        ui.add(egui::Label::new(egui::RichText::new(text).size(12.0)).wrap());
    });
}

fn country_label(slug: &str) -> String {
    slug.split('-')
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => format!("{}{}", first.to_ascii_uppercase(), chars.as_str()),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn ttl(expires_at: f64) -> String {
    let mins = ((expires_at - now_ms()) / 60_000.0).ceil().max(0.0) as i64;
    if mins >= 60 {
        format!("{}h {:02}m", mins / 60, mins % 60)
    } else {
        format!("{mins}m")
    }
}

fn number_label(ui: &egui::Ui, row: &store::NumberRow) -> egui::text::LayoutJob {
    let p = palette(ui.ctx());
    let mut job = egui::text::LayoutJob::default();
    job.wrap.max_width = (ui.available_width() - 18.0).max(120.0);
    job.append(
        &row.e164,
        0.0,
        egui::text::TextFormat {
            font_id: egui::FontId::monospace(12.0),
            color: p.text,
            ..Default::default()
        },
    );
    job.append(
        &format!(
            "\n{} · expires in {}",
            country_label(&row.country),
            ttl(row.expires_at)
        ),
        0.0,
        egui::text::TextFormat {
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
        let mut error = Some(("receive-sms.io returned 404".to_string(), true));
        clear_prior_error(&mut error);
        assert!(error.is_none());

        let mut notice = Some(("Now watching +447400299988.".to_string(), false));
        clear_prior_error(&mut notice);
        assert_eq!(
            notice.as_ref().map(|(text, _)| text.as_str()),
            Some("Now watching +447400299988.")
        );
    }
}

pub fn theme(ctx: &egui::Context) {
    temp_ui::apply(ctx, temp_ui::Theme::Dark);
}
