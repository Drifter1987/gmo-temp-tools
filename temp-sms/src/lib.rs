mod background;
pub mod gui;
mod health;
mod scrape;
mod store;
mod util;
mod webhook;

use local_auth::ApiGuard;
use std::collections::HashMap;
use std::io::Read;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tiny_http::{Header, Method, Response, Server};
use util::*;

pub(crate) const TTL_MS: f64 = 7.0 * 24.0 * 3600.0 * 1000.0;
const LIST_TTL_MS: f64 = 8_000.0;
const COUNTRY_TTL_MS: f64 = 600_000.0;

static NOTIFICATIONS_ENABLED: AtomicBool = AtomicBool::new(true);

/// The desktop and web settings both call this. The background poller reads it
/// before every toast, so turning notifications off takes effect immediately.
pub fn set_notifications_enabled(enabled: bool) {
    NOTIFICATIONS_ENABLED.store(enabled, Ordering::Relaxed);
}

pub(crate) fn notifications_enabled() -> bool {
    NOTIFICATIONS_ENABLED.load(Ordering::Relaxed)
}

pub(crate) struct State {
    pub(crate) store: store::Store,
    countries: Option<(f64, Vec<String>)>,
    cache: HashMap<String, (f64, Vec<scrape::ScrapedMessage>)>,
    pub(crate) health: health::Health,
}

pub(crate) type Shared = Arc<Mutex<State>>;

fn json_header() -> Header {
    Header::from_bytes(&b"Content-Type"[..], &b"application/json; charset=utf-8"[..]).unwrap()
}

fn html_header() -> Header {
    Header::from_bytes(&b"Content-Type"[..], &b"text/html; charset=utf-8"[..]).unwrap()
}

fn reply(request: tiny_http::Request, status: u16, body: String, header: Header) {
    let _ = request.respond(Response::from_string(body).with_status_code(status).with_header(header));
}

fn reply_json(request: tiny_http::Request, status: u16, value: serde_json::Value) {
    reply(request, status, value.to_string(), json_header());
}

fn fetch_once(url: &str) -> Result<String, String> {
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(8))
        .timeout(Duration::from_secs(20))
        .user_agent(scrape::AGENT)
        .build();
    match agent.get(url).call() {
        Ok(response) => {
            // Read bytes and decode as UTF-8 ourselves: the site's charset header
            // is not reliable, and trusting it turned "Doğrulama" into mojibake.
            let mut buffer = Vec::new();
            response
                .into_reader()
                .read_to_end(&mut buffer)
                .map_err(|e| e.to_string())?;
            Ok(String::from_utf8_lossy(&buffer).to_string())
        }
        Err(ureq::Error::Status(code, _)) => Err(format!("receive-sms.io returned {code}")),
        Err(err) => Err(err.to_string()),
    }
}

const RETRY_DELAYS_MS: [u64; 2] = [250, 750];

fn is_transient_error(error: &str) -> bool {
    let error = error.to_ascii_lowercase();
    [
        "429",
        "rate limit",
        "timed out",
        "timeout",
        "connection",
        "network",
        "dns",
        "returned 5",
        "bad gateway",
        "service unavailable",
        "temporarily unavailable",
    ]
    .iter()
    .any(|needle| error.contains(needle))
}

/// Fetch a public page with two bounded retries for rate limits, timeouts and
/// server errors. A 404 (the number vanished) is returned immediately so the
/// caller can show it rather than wasting the page's quota.
pub(crate) fn fetch_with_retry(url: &str) -> Result<(String, u32), String> {
    let mut last_error = String::new();
    for attempt in 0..=RETRY_DELAYS_MS.len() {
        match fetch_once(url) {
            Ok(body) => return Ok((body, attempt as u32)),
            Err(error) => {
                if attempt == RETRY_DELAYS_MS.len() || !is_transient_error(&error) {
                    return Err(error);
                }
                last_error = error;
                std::thread::sleep(Duration::from_millis(RETRY_DELAYS_MS[attempt]));
            }
        }
    }
    Err(last_error)
}


fn message_url(card: &store::Card) -> String {
    format!(
        "{}/temporary-numbers/{}/{}/",
        scrape::BASE,
        card.slug,
        digits_only(&card.row.e164)
    )
}

fn messages_for(
    state: &mut State,
    card: &store::Card,
    fresh: bool,
) -> Result<Vec<scrape::ScrapedMessage>, String> {
    let now = now_ms();
    let cached = state
        .cache
        .get(&card.row.id)
        .map(|(_, messages)| messages.clone());
    if !fresh {
        if let Some((fetched, cached)) = state.cache.get(&card.row.id) {
            if now - fetched < LIST_TTL_MS {
                return Ok(cached.clone());
            }
        }
    }
    state.health.source.attempt();
    match fetch_with_retry(&message_url(card)) {
        Ok((html, retries)) => {
            state.health.source.succeeded(retries);
            let messages = scrape::messages(&html);
            state
                .cache
                .insert(card.row.id.clone(), (now, messages.clone()));
            Ok(messages)
        }
        Err(error) => {
            if let Some(cached) = cached {
                state.health.source.using_cache(&error);
                Ok(cached)
            } else {
                state.health.source.failed(&error);
                Err(error)
            }
        }
    }
}

fn message_id(card_id: &str, message: &scrape::ScrapedMessage) -> String {
    format!("{card_id}-{}", message.received_at as i64)
}

/// What a card shows: the public page's messages plus anything a signed webhook
/// delivered for that number, newest first and without duplicates.
pub(crate) fn merged(state: &mut State, card: &store::Card, fresh: bool) -> Vec<scrape::ScrapedMessage> {
    let mut list = if card.slug.is_empty() {
        Vec::new()
    } else {
        messages_for(state, card, fresh).unwrap_or_default()
    };

    for row in state.store.messages_for(&card.row.id) {
        let dup = list
            .iter()
            .any(|m| m.body == row.body && (m.received_at - row.received_at).abs() < 3000.0);
        if !dup {
            list.push(scrape::ScrapedMessage {
                sender: row.sender,
                body: row.body,
                received_at: row.received_at,
                when: String::new(),
            });
        }
    }

    list.sort_by(|a, b| {
        b.received_at
            .partial_cmp(&a.received_at)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    list
}

/// Fetch the public country list. A failed fetch keeps the last good list and
/// marks it as cache-backed instead of replacing the UI with a blank picker.
pub(crate) fn country_list(state: &mut State, fresh: bool) -> Result<Vec<String>, String> {
    let now = now_ms();
    let cached = state.countries.as_ref().map(|(_, list)| list.clone());
    if !fresh {
        if let Some((fetched, list)) = &state.countries {
            if now - fetched < COUNTRY_TTL_MS {
                return Ok(list.clone());
            }
        }
    }
    state.health.source.attempt();
    match fetch_with_retry(&format!("{}/", scrape::BASE)) {
        Ok((html, retries)) => {
            let countries = scrape::countries(&html);
            if countries.is_empty() {
                let error = "the source listed no countries".to_string();
                state.health.source.failed(&error);
                return Err(error);
            }
            state.health.source.succeeded(retries);
            state.countries = Some((now, countries.clone()));
            Ok(countries)
        }
        Err(error) => {
            if let Some(cached) = cached {
                state.health.source.using_cache(&error);
                Ok(cached)
            } else {
                state.health.source.failed(&error);
                Err(error)
            }
        }
    }
}

/// Read the currently listed numbers for one country. The result is the source
/// page's own list, so the user is choosing a public inbox that exists now.
pub(crate) fn numbers_for_country(
    state: &mut State,
    slug: &str,
) -> Result<Vec<String>, String> {
    let url = format!("{}/temporary-numbers/{}/", scrape::BASE, slug);
    state.health.source.attempt();
    match fetch_with_retry(&url) {
        Ok((html, retries)) => {
            state.health.source.succeeded(retries);
            let mut numbers: Vec<String> = scrape::numbers(&html)
                .into_iter()
                .filter(|(country, _)| country == slug)
                .map(|(_, digits)| digits)
                .collect();
            numbers.sort();
            numbers.dedup();
            if numbers.is_empty() {
                Err("no public numbers are listed for this country right now".to_string())
            } else {
                Ok(numbers)
            }
        }
        Err(error) => {
            state.health.source.failed(&error);
            Err(error)
        }
    }
}

fn header(request: &tiny_http::Request, name: &str) -> Option<String> {
    request
        .headers()
        .iter()
        .find(|h| h.field.as_str().as_str().eq_ignore_ascii_case(name))
        .map(|h| h.value.as_str().to_string())
}

fn env(name: &str) -> String {
    std::env::var(name).unwrap_or_default()
}



/// POST /api/inbound/sms, or /api/inbound/<provider>.
fn handle_inbound(
    mut request: tiny_http::Request,
    shared: &Shared,
    provider_path: Option<&str>,
    path: &str,
    query: &str,
) {
    let mut body = String::new();
    let _ = request.as_reader().read_to_string(&mut body);

    let provider = webhook::detect(provider_path, &body);
    let proto = header(&request, "x-forwarded-proto").unwrap_or_else(|| "http".to_string());
    let host = header(&request, "host").unwrap_or_default();
    let url = webhook::expected_url(&env("TWILIO_WEBHOOK_URL"), &proto, &host, path, query);
    let query_params = webhook::form_pairs(query);
    let provided_secret = header(&request, "x-gmo-secret")
        .or_else(|| webhook::form_value(&query_params, "k").map(|s| s.to_string()));

    let twilio_token = env("TWILIO_AUTH_TOKEN");
    let telnyx_key = env("TELNYX_PUBLIC_KEY");
    let generic_secret = env("WEBHOOK_SECRET");
    let creds = webhook::Credentials {
        twilio_auth_token: &twilio_token,
        telnyx_public_key: &telnyx_key,
        generic_secret: &generic_secret,
    };
    let twilio_signature = header(&request, "x-twilio-signature");
    let telnyx_signature = header(&request, "telnyx-signature-ed25519");
    let telnyx_timestamp = header(&request, "telnyx-timestamp");
    let proof = webhook::Proof {
        url: &url,
        twilio_signature: twilio_signature.as_deref(),
        telnyx_signature: telnyx_signature.as_deref(),
        telnyx_timestamp: telnyx_timestamp.as_deref(),
        generic_secret: provided_secret.as_deref(),
    };

    let message = match webhook::accept(provider, &body, &proof, &creds, webhook::now_secs()) {
        Ok(message) => message,
        Err(rejection) => {
            return reply_json(
                request,
                rejection.status(),
                serde_json::json!({ "error": rejection.message() }),
            )
        }
    };

    let Some(e164) = normalize_e164(&message.to) else {
        return reply_json(
            request,
            422,
            serde_json::json!({ "error": "The To number is not a usable E.164 number" }),
        );
    };
    let digits = digits_only(&e164);

    let mut state = shared.lock().unwrap();
    state.store.purge(now_ms());
    let card = match state.store.find_by_digits(&digits) {
        Some(card) => card,
        None => {
            // A webhook may name a number this app has never seen; keep the card
            // so the message has somewhere to live.
            let now = now_ms();
            let row = store::NumberRow {
                id: rand_hex(20),
                owner_id: "webhook".to_string(),
                e164: e164.clone(),
                country: "webhook".to_string(),
                provider: match provider {
                    webhook::Provider::Twilio => "twilio",
                    webhook::Provider::Telnyx => "telnyx",
                    webhook::Provider::Generic => "webhook",
                }
                .to_string(),
                created_at: now,
                expires_at: now + TTL_MS,
            };
            let card = store::Card {
                row,
                slug: String::new(),
            };
            state.store.push(card.clone());
            card
        }
    };

    let now = now_ms();
    let id = format!("{}-{}", card.row.id, now as i64);
    let code = extract_code(&message.body).unwrap_or_default();
    let stored = state.store.push_message(store::SmsRow {
        id: id.clone(),
        sid: message.sid.clone(),
        number_id: card.row.id.clone(),
        sender: message.from.clone(),
        body: message.body.clone(),
        code: code.clone(),
        received_at: now,
        seen: 0.0,
    });
    drop(state);

    reply_json(
        request,
        200,
        serde_json::json!({
            "ok": true,
            "stored": stored,
            "id": id,
            "number": card.row.e164,
            "code": code,
        }),
    );
}

fn handle(mut request: tiny_http::Request, shared: &Shared, guard: &ApiGuard) {
    let method = request.method().clone();
    let raw_url = request.url().to_string();
    let path = raw_url.split('?').next().unwrap_or("/").to_string();
    if method == Method::Get && !path.starts_with("/api/") {
        return local_auth::redirect_to_web(request);
    }
    if let Err(failure) = guard.authorize(&request, &path) {
        return reply_json(
            request,
            failure.status(),
            serde_json::json!({ "error": failure.message() }),
        );
    }
    let query = raw_url.splitn(2, '?').nth(1).unwrap_or("").to_string();
    let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();

    if method == Method::Post && segments.len() >= 2 && segments[0] == "api" && segments[1] == "inbound"
    {
        if segments.len() <= 3 {
            let provider_path = segments.get(2).copied();
            return handle_inbound(request, shared, provider_path, &path, &query);
        }
    }

    match (&method, segments.as_slice()) {
        (Method::Get, ["api", "status"]) => {
            let state = shared.lock().unwrap();
            let count = state.store.cards.len();
            let source_health = state.health.snapshot();
            drop(state);
            reply_json(
                request,
                200,
                serde_json::json!({
                    "mode": "live",
                    "provider": "receive-sms.io",
                    "numbers": count,
                    "source": "public shared inboxes",
                    "source_health": source_health,
                    "provider_rule": health::SOURCE_RULE,
                    "fallback_order": ["live public page", "last good cache", "signed carrier webhook"],
                    "webhook": {
                        "twilio": !env("TWILIO_AUTH_TOKEN").is_empty(),
                        "twilio_url_override": !env("TWILIO_WEBHOOK_URL").is_empty(),
                        "telnyx": !env("TELNYX_PUBLIC_KEY").is_empty(),
                        "generic": !env("WEBHOOK_SECRET").is_empty(),
                    },
                }),
            );
        }

        (Method::Get, ["api", "health"]) => {
            let health = shared.lock().unwrap().health.snapshot();
            reply_json(
                request,
                200,
                serde_json::json!({
                    "source": health,
                    "provider_rule": health::SOURCE_RULE,
                    "fallback_order": ["live public page", "last good cache", "signed carrier webhook"],
                }),
            );
        }

        (Method::Get, ["api", "countries"]) => {
            let mut state = shared.lock().unwrap();
            let countries = country_list(&mut state, false);
            let source_health = state.health.snapshot();
            drop(state);
            match countries {
                Ok(countries) => reply_json(
                    request,
                    200,
                    serde_json::json!({
                        "countries": countries,
                        "source_health": source_health,
                    }),
                ),
                Err(error) => reply_json(
                    request,
                    502,
                    serde_json::json!({ "error": error, "source_health": source_health }),
                ),
            }
        }

        (Method::Get, ["api", "countries", slug, "numbers"]) => {
            let mut state = shared.lock().unwrap();
            let numbers = numbers_for_country(&mut state, slug);
            let source_health = state.health.snapshot();
            drop(state);
            match numbers {
                Ok(numbers) => reply_json(
                    request,
                    200,
                    serde_json::json!({
                        "country": slug,
                        "numbers": numbers,
                        "source": "public shared inboxes",
                        "source_health": source_health,
                    }),
                ),
                Err(error) => reply_json(
                    request,
                    502,
                    serde_json::json!({ "error": error, "source_health": source_health }),
                ),
            }
        }

        (Method::Post, ["api", "source", "check"]) => {
            let mut state = shared.lock().unwrap();
            let countries = country_list(&mut state, true);
            let source_health = state.health.snapshot();
            drop(state);
            match countries {
                Ok(countries) => reply_json(
                    request,
                    200,
                    serde_json::json!({
                        "ok": true,
                        "country_count": countries.len(),
                        "countries": countries,
                        "source_health": source_health,
                    }),
                ),
                Err(error) => reply_json(
                    request,
                    502,
                    serde_json::json!({ "error": error, "source_health": source_health }),
                ),
            }
        }

        (Method::Get, ["api", "numbers"]) => {
            let mut state = shared.lock().unwrap();
            state.store.purge(now_ms());
            let numbers: Vec<serde_json::Value> = state
                .store
                .cards
                .iter()
                .map(|card| {
                    serde_json::json!({
                        "id": card.row.id,
                        "e164": card.row.e164,
                        "country": card.row.country,
                        "provider": card.row.provider,
                        "created_at": card.row.created_at,
                        "expires_at": card.row.expires_at,
                    })
                })
                .collect();
            drop(state);
            reply_json(request, 200, serde_json::json!({ "numbers": numbers }))
        }

        (Method::Post, ["api", "numbers"]) => {
            let mut body = String::new();
            let _ = request.as_reader().read_to_string(&mut body);
            let parsed: serde_json::Value = match serde_json::from_str(&body) {
                Ok(value) => value,
                Err(_) => {
                    return reply_json(
                        request,
                        400,
                        serde_json::json!({ "error": "expected { country, number }" }),
                    )
                }
            };
            let Some(slug) = parsed.get("country").and_then(|value| value.as_str()) else {
                return reply_json(
                    request,
                    422,
                    serde_json::json!({ "error": "choose a country from the source list" }),
                );
            };
            let slug = slug.trim().to_ascii_lowercase();
            if slug.is_empty()
                || !slug
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c == '-')
            {
                return reply_json(
                    request,
                    422,
                    serde_json::json!({ "error": "the country slug is invalid" }),
                );
            }
            let Some(number) = parsed.get("number").and_then(|value| value.as_str()) else {
                return reply_json(
                    request,
                    422,
                    serde_json::json!({ "error": "choose a number from the source list" }),
                );
            };
            let Some(e164) = normalize_e164(number) else {
                return reply_json(
                    request,
                    422,
                    serde_json::json!({ "error": "that public number is not a valid E.164 number" }),
                );
            };

            let mut state = shared.lock().unwrap();
            state.store.purge(now_ms());
            let now = now_ms();
            let row = store::NumberRow {
                id: rand_hex(20),
                owner_id: "local".to_string(),
                e164,
                country: slug.clone(),
                provider: "receive-sms.io".to_string(),
                created_at: now,
                expires_at: now + TTL_MS,
            };
            let card = state.store.push_unique(store::Card { row, slug });
            let id = card.row.id;
            let e164 = card.row.e164;
            drop(state);

            reply_json(
                request,
                200,
                serde_json::json!({ "id": id, "e164": e164 }),
            );
        }

        (Method::Delete, ["api", "numbers", id]) => {
            let mut state = shared.lock().unwrap();
            state.store.remove(id);
            state.cache.remove(*id);
            drop(state);
            reply(request, 200, String::new(), json_header());
        }

        (Method::Get, ["api", "numbers", id, "messages"]) => {
            let mut state = shared.lock().unwrap();
            state.store.purge(now_ms());
            let Some(card) = state.store.find(id) else {
                drop(state);
                return reply_json(request, 404, serde_json::json!({ "error": "Number not found" }));
            };

            // Respect the short cache, unless the caller explicitly asks for a
            // fresh read of the selected public page.
            let fresh = query.split('&').any(|pair| pair == "fresh=1");
            let messages = merged(&mut state, &card, fresh);

            let items: Vec<serde_json::Value> = messages
                .iter()
                .map(|m| {
                    let id = message_id(&card.row.id, m);
                    serde_json::json!({
                        "id": id,
                        "sender": m.sender,
                        "body": m.body,
                        "code": extract_code(&m.body).unwrap_or_default(),
                        "preview": preview(&m.body, 90),
                        "received_at": if m.received_at > 0.0 { m.received_at } else { now_ms() },
                        "seen": state.store.seen.contains(&message_id(&card.row.id, m)),
                    })
                })
                .collect();
            let source_health = state.health.snapshot();
            drop(state);

            reply_json(
                request,
                200,
                serde_json::json!({
                    "number": { "e164": card.row.e164, "country": card.row.country },
                    "count": items.len(),
                    "messages": items,
                    "source_health": source_health,
                }),
            );
        }

        (Method::Get, ["api", "messages", id]) => {
            let mut state = shared.lock().unwrap();

            // Webhook messages are held locally, so they resolve without a fetch.
            if let Some(row) = state.store.find_message(id) {
                let number = state
                    .store
                    .find(&row.number_id)
                    .map(|card| card.row.e164)
                    .unwrap_or_default();
                state.store.mark_seen(id);
                drop(state);
                return reply_json(
                    request,
                    200,
                    serde_json::json!({
                        "id": row.id,
                        "sender": row.sender,
                        "body": row.body,
                        "code": row.code,
                        "received_at": row.received_at,
                        "number": number,
                    }),
                );
            }

            let cards = state.store.cards.clone();
            let mut found = None;
            for card in cards {
                if !id.starts_with(&format!("{}-", card.row.id)) {
                    continue;
                }
                let messages = merged(&mut state, &card, false);
                if let Some(message) = messages
                    .into_iter()
                    .find(|m| message_id(&card.row.id, m) == *id)
                {
                    found = Some((card, message));
                    break;
                }
            }

            let Some((card, message)) = found else {
                drop(state);
                return reply_json(request, 404, serde_json::json!({ "error": "Message not found" }));
            };

            state.store.mark_seen(id);
            drop(state);

            reply_json(
                request,
                200,
                serde_json::json!({
                    "id": id,
                    "sender": message.sender,
                    "body": message.body,
                    "code": extract_code(&message.body).unwrap_or_default(),
                    "received_at": if message.received_at > 0.0 { message.received_at } else { now_ms() },
                    "number": card.row.e164,
                }),
            );
        }

        _ => reply(request, 404, "Not found".to_string(), html_header()),
    }
}

/// Start the API with the install token. The token is mandatory: there is no
/// unauthenticated mode for a shipped listener.
pub fn start_with_auth(
    port: u16,
    state_dir: impl AsRef<std::path::Path>,
    auth_token: String,
) -> Result<gui::SmsApp, String> {
    let auth_token = auth_token.trim().to_string();
    if auth_token.is_empty() {
        return Err("an install token is required".to_string());
    }
    let state_dir = state_dir.as_ref();
    let addr = format!("127.0.0.1:{port}");
    let server = Server::http(&addr).map_err(|err| format!("cannot listen on {addr}: {err}"))?;

    let state = State {
        store: store::Store::load(state_dir.join("numbers.json")),
        countries: None,
        cache: HashMap::new(),
        health: health::Health::new(),
    };
    let shared: Shared = Arc::new(Mutex::new(state));
    let web_port = std::env::var("TEMPTOOLS_WEB_PORT")
        .ok()
        .and_then(|value| value.trim().parse().ok())
        .unwrap_or(8780);
    let guard = Arc::new(ApiGuard::new(
        auth_token,
        local_auth::origins(&[port, web_port]),
    ));

    println!("temp-sms: API on http://{addr} (receive-sms.io)");

    let server_shared = shared.clone();
    let server_guard = guard.clone();
    std::thread::spawn(move || {
        for request in server.incoming_requests() {
            let shared = server_shared.clone();
            let guard = server_guard.clone();
            std::thread::spawn(move || handle(request, &shared, &guard));
        }
    });

    let app = gui::SmsApp::new(shared.clone());
    background::start(shared);
    Ok(app)
}

#[cfg(test)]
mod tests {
    use super::is_transient_error;

    #[test]
    fn retries_rate_limits_and_server_failures_but_not_missing_pages() {
        assert!(is_transient_error("receive-sms.io returned 429"));
        assert!(is_transient_error("request timed out"));
        assert!(is_transient_error("receive-sms.io returned 503"));
        assert!(!is_transient_error("receive-sms.io returned 404"));
        assert!(!is_transient_error("the page did not contain a number"));
    }
}
