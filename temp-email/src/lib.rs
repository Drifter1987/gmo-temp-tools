mod background;
pub mod gui;
mod health;
mod mailtm;
mod persist;
mod providers;
mod store;
mod util;

use local_auth::ApiGuard;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tiny_http::{Header, Method, Response, Server};
use util::*;

pub(crate) const TTL_MS: f64 = 60.0 * 60.0 * 1000.0;
const LIST_TTL_MS: f64 = 4_000.0;
const PENDING_AFTER_MS: f64 = 5.0 * 60.0 * 1000.0;
const QUIET_AFTER_MS: f64 = 15.0 * 60.0 * 1000.0;

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
    health: health::Health,
    lists: HashMap<String, (f64, Vec<providers::Brief>)>,
    owners: HashMap<String, String>,
    seen: HashSet<String>,
}

impl State {
    pub(crate) fn is_message_seen(&self, message: &providers::Brief) -> bool {
        message.seen || self.seen.contains(&message.id)
    }

    pub(crate) fn unread_count(&self, messages: &[providers::Brief]) -> usize {
        messages
            .iter()
            .filter(|message| !self.is_message_seen(message))
            .count()
    }

    /// Unread counts come from the list the background poller already cached,
    /// so native badges do not add provider requests.
    pub(crate) fn cached_unread(&self, id: &str) -> Option<usize> {
        self.lists
            .get(id)
            .map(|(_, messages)| self.unread_count(messages))
    }

    /// Remove expired inboxes and settle their terminal outcome in the same
    /// critical section. The store row is gone afterward; the ledger row stays.
    pub(crate) fn purge_expired(&mut self, now: f64) {
        // Persist the ledger first. If the process stops between these two
        // operations, the next startup can settle the still-present row again;
        // the terminal outcome is idempotent.
        let expired = self.store.expired(now);
        for account in &expired {
            self.health.settle_outcome(&account.row, now);
        }
        self.store.purge(now);
    }

    /// Delete one inbox after recording whether its 15-minute window had a
    /// terminal outcome. This keeps a deleted inbox in the domain ledger.
    pub(crate) fn remove_inbox(&mut self, id: &str) {
        if let Some(account) = self.store.find(id) {
            self.health.settle_outcome(&account.row, now_ms());
        }
        self.store.remove(id);
        self.lists.remove(id);
    }

    fn storage_error(&self) -> Option<String> {
        let mut errors = Vec::new();
        if let Some(err) = self.store.last_error() {
            errors.push(err.to_string());
        }
        if let Some(err) = self.health.last_error() {
            errors.push(err.to_string());
        }
        if errors.is_empty() {
            None
        } else {
            Some(errors.join("; "))
        }
    }
}

pub(crate) type Shared = Arc<Mutex<State>>;

/// One provider attempt that did not produce a mailbox. Kept in the creation
/// result so the API and native window can say which provider failed and why.
#[derive(Clone, Debug)]
pub(crate) struct ProviderFailure {
    pub provider: String,
    pub error: String,
}

#[derive(Clone)]
pub(crate) struct InboxCreation {
    pub row: store::InboxRow,
    pub requested_provider: String,
    pub used_provider: String,
    pub fallback: bool,
    pub failures: Vec<ProviderFailure>,
}

fn json_header() -> Header {
    Header::from_bytes(&b"Content-Type"[..], &b"application/json; charset=utf-8"[..]).unwrap()
}

fn html_header() -> Header {
    Header::from_bytes(&b"Content-Type"[..], &b"text/html; charset=utf-8"[..]).unwrap()
}

fn reply(request: tiny_http::Request, status: u16, body: String, header: Header) {
    let response = Response::from_string(body)
        .with_status_code(status)
        .with_header(header);
    let _ = request.respond(response);
}

fn reply_json(request: tiny_http::Request, status: u16, value: serde_json::Value) {
    reply(request, status, value.to_string(), json_header());
}

const ADJECTIVES: [&str; 12] = [
    "quiet", "amber", "silver", "copper", "rapid", "velvet", "north", "hollow", "cobalt", "linen",
    "brisk", "mellow",
];
const NOUNS: [&str; 12] = [
    "delta", "harbor", "falcon", "willow", "summit", "otter", "ember", "marlin", "canyon", "cedar",
    "pilot", "river",
];

pub(crate) fn new_address(domain: &str) -> String {
    let mut rng = rand::thread_rng();
    use rand::Rng;
    format!(
        "{}-{}-{}@{}",
        ADJECTIVES[rng.gen_range(0..ADJECTIVES.len())],
        NOUNS[rng.gen_range(0..NOUNS.len())],
        rand_hex(4),
        domain
    )
}

pub(crate) fn add_inbox(
    shared: &Shared,
    provider: providers::Provider,
) -> Result<InboxCreation, String> {
    let requested = provider.slug();
    let mut failures = Vec::new();

    for candidate in providers::Provider::fallback_order(provider) {
        let slug = candidate.slug();
        let created = match providers::create_with_retry(candidate) {
            Ok(created) => created,
            Err(err) => {
                let mut state = shared.lock().unwrap();
                state.health.attempt(slug, candidate.default_domain());
                state
                    .health
                    .create_failed(slug, candidate.default_domain(), &err);
                failures.push(ProviderFailure {
                    provider: slug.to_string(),
                    error: err,
                });
                continue;
            }
        };

        let now = now_ms();
        let domain = domain_of(&created.address).to_string();
        let row = store::InboxRow {
            id: rand_hex(20),
            address: created.address,
            provider: slug.to_string(),
            created_at: now,
            expires_at: created
                .expires_at
                .unwrap_or(now + TTL_MS)
                .min(now + TTL_MS)
                .max(now + 60_000.0),
            first_message_at: None,
            first_external_message_at: None,
            first_unclassified_message_at: None,
        };
        let mut state = shared.lock().unwrap();
        state.health.attempt(slug, &domain);
        state.health.created(slug, &domain);
        state.health.start_outcome(&row);
        state.store.push(store::Account {
            row: row.clone(),
            password: created.password,
            token: created.token,
            remote_id: created.remote_id,
        });
        return Ok(InboxCreation {
            row,
            requested_provider: requested.to_string(),
            used_provider: slug.to_string(),
            fallback: candidate != provider,
            failures,
        });
    }

    let detail = failures
        .iter()
        .map(|failure| format!("{}: {}", failure.provider, failure.error))
        .collect::<Vec<_>>()
        .join("; ");
    Err(format!("all mail providers failed ({detail})"))
}

fn list_for(
    state: &mut State,
    account: &store::Account,
    fresh: bool,
) -> Result<Vec<providers::Brief>, String> {
    let now = now_ms();
    let ttl = providers::Provider::from_slug(&account.row.provider)
        .map(providers::Provider::cache_ttl_ms)
        .unwrap_or(LIST_TTL_MS);
    if !fresh {
        if let Some((fetched, cached)) = state.lists.get(&account.row.id) {
            if now - fetched < ttl {
                return Ok(cached.clone());
            }
        }
    }
    let messages = match providers::list_with_retry(account) {
        Ok(messages) => messages,
        Err(err) => {
            record_list_failure(state, account, &err);
            return Err(err);
        }
    };
    record_list_success(state, account, &messages);
    Ok(messages)
}

pub(crate) fn record_list_success(
    state: &mut State,
    account: &store::Account,
    messages: &[providers::Brief],
) {
    let domain = domain_of(&account.row.address);
    state
        .health
        .provider_list_succeeded(&account.row.provider);
    state
        .health
        .list_succeeded(&account.row.provider, domain);
    record_messages(state, account, &messages);
    state
        .lists
        .insert(account.row.id.clone(), (now_ms(), messages.to_vec()));
}

pub(crate) fn record_list_failure(
    state: &mut State,
    account: &store::Account,
    err: &str,
) {
    let domain = domain_of(&account.row.address);
    if provider_wide_list_error(err) {
        state
            .health
            .provider_list_failed(&account.row.provider, err);
    } else {
        state
            .health
            .list_failed(&account.row.provider, domain, err);
    }
}

/// Refresh one inbox through the same cache/health path as the API. Manual
/// refresh bypasses the provider cache but still records failures and retries.
pub(crate) fn refresh_account(shared: &Shared, id: &str) -> Result<Vec<providers::Brief>, String> {
    let mut state = shared.lock().unwrap();
    state.purge_expired(now_ms());
    let account = state
        .store
        .find(id)
        .ok_or_else(|| "That inbox is gone.".to_string())?;
    list_for(&mut state, &account, true)
}

/// Mark a message read in the shared local state. Native and web both use
/// this so the unread counts reported by `/api/inboxes` agree.
pub(crate) fn mark_seen(shared: &Shared, message_id: &str) {
    shared.lock().unwrap().seen.insert(message_id.to_string());
}

/// Background refresh for the visible desktop tab. The provider's own cache
/// TTL still applies, so a fast repaint does not spend the provider quota.
pub(crate) fn cached_account(shared: &Shared, id: &str) -> Result<Vec<providers::Brief>, String> {
    let mut state = shared.lock().unwrap();
    state.purge_expired(now_ms());
    let account = state
        .store
        .find(id)
        .ok_or_else(|| "That inbox is gone.".to_string())?;
    list_for(&mut state, &account, false)
}

/// Errors that describe the provider connection or its response shape belong
/// to the provider, not to a particular inbox domain. Account/mailbox errors
/// such as a 404 or 401 stay on the domain row.
fn provider_wide_list_error(err: &str) -> bool {
    let err = err.to_ascii_lowercase();
    [
        "429",
        "rate limit",
        "timed out",
        "timeout",
        "did not answer",
        "connection",
        "network",
        "dns",
        "status code 5",
        "server error",
        "bad gateway",
        "gateway",
        "service unavailable",
        "parser",
        "parse error",
        "invalid json",
        "invalid type",
        "invalid response",
        "expected",
        "missing field",
        "deserialization",
    ]
    .iter()
    .any(|needle| err.contains(needle))
}

fn earliest(current: Option<f64>, at: f64) -> Option<f64> {
    Some(current.map_or(at, |previous| previous.min(at)))
}

/// Record the first message and the first external message for an inbox.
/// The store's transition methods make duplicate polls idempotent; only the
/// first external transition increments the domain's delivery count.
pub(crate) fn record_messages(
    state: &mut State,
    account: &store::Account,
    messages: &[providers::Brief],
) {
    let now = now_ms();
    let provider = providers::Provider::from_slug(&account.row.provider);
    let inbox_domain = domain_of(&account.row.address);
    let mut first_message_at = None;
    let mut external_at = None;
    let mut unclassified_at = None;

    for message in messages {
        let received_at = (message.received_at > 0.0).then_some(message.received_at);
        if let Some(at) = received_at {
            first_message_at = earliest(first_message_at, at);
        }
        let at = received_at.unwrap_or(now);
        let class = provider
            .map(|provider| providers::classify_message(provider, inbox_domain, message))
            .unwrap_or(providers::MessageClass::Unclassified);
        match class {
            providers::MessageClass::External => external_at = earliest(external_at, at),
            providers::MessageClass::Unclassified => {
                unclassified_at = earliest(unclassified_at, at)
            }
            providers::MessageClass::System => {}
        }
        state.owners.insert(message.id.clone(), account.row.id.clone());
    }

    if let Some(at) = first_message_at {
        state.store.mark_delivered(&account.row.id, at);
    }
    if let Some(at) = unclassified_at {
        if state.store.mark_unclassified(&account.row.id, at) {
            state
                .health
                .unclassified_message(&account.row.provider, inbox_domain);
        }
    }
    if let Some(at) = external_at {
        state.health.mark_outcome_delivered(&account.row, at);
        if state.store.mark_external_delivered(&account.row.id, at) {
            state
                .health
                .external_delivered(&account.row.provider, inbox_domain);
        }
    }
}

fn detail_for(state: &State, id: &str) -> Option<providers::Detail> {
    if let Some(account) = state
        .owners
        .get(id)
        .and_then(|owner| state.store.find(owner))
    {
        return providers::detail(&account, id).ok();
    }
    state
        .store
        .accounts
        .iter()
        .find_map(|account| providers::detail(account, id).ok())
}

fn acceptance_rows(state: &State, now: f64) -> Vec<health::Row> {
    let mut rows = state.health.rows();
    for account in &state.store.accounts {
        let domain = domain_of(&account.row.address);
        if rows.iter().any(|row| {
            row.provider == account.row.provider && row.domain == domain
        }) {
            continue;
        }
        let mut created = 0;
        let mut delivered = 0;
        let mut unclassified = 0;
        for other in &state.store.accounts {
            if other.row.provider == account.row.provider
                && domain_of(&other.row.address) == domain
            {
                created += 1;
                if other.row.first_external_message_at.is_some() {
                    delivered += 1;
                }
                if other.row.first_unclassified_message_at.is_some() {
                    unclassified += 1;
                }
            }
        }
        rows.push(health::Row {
            provider: account.row.provider.clone(),
            domain: domain.to_string(),
            attempts: created,
            created,
            delivered,
            unclassified,
            ..health::Row::default()
        });
    }
    for row in &mut rows {
        if row.legacy {
            row.pending = 0;
            row.quiet = 0;
            continue;
        }
        let mut pending = 0;
        let mut quiet = 0;
        let mut live_delivered = 0;
        for account in &state.store.accounts {
            if account.row.provider != row.provider
                || domain_of(&account.row.address) != row.domain
            {
                continue;
            }
            if account.row.first_external_message_at.is_some() {
                live_delivered += 1;
                continue;
            }
            let age = now - account.row.created_at;
            if age >= QUIET_AFTER_MS {
                quiet += 1;
            } else if age >= PENDING_AFTER_MS {
                pending += 1;
            }
        }
        row.pending = pending;
        row.quiet = quiet;
        if row.delivered < live_delivered {
            row.delivered = live_delivered;
        }
    }
    rows.sort_by(|a, b| (&a.provider, &a.domain).cmp(&(&b.provider, &b.domain)));
    rows
}

pub(crate) fn provider_health_rows(state: &State, now: f64) -> Vec<health::ProviderRow> {
    let mut active: HashMap<String, u64> = HashMap::new();
    for account in &state.store.accounts {
        if account.row.expires_at > now {
            *active.entry(account.row.provider.clone()).or_insert(0) += 1;
        }
    }

    let mut rows = state.health.provider_rows();
    for row in &mut rows {
        row.active_inboxes = active.get(&row.provider).copied().unwrap_or(0);
        row.error_is_current = row.active_inboxes > 0 && !row.last_error.is_empty();
        if !row.error_is_current {
            row.last_error.clear();
        }
    }
    rows
}

fn query_value(url: &str, key: &str) -> Option<String> {
    let (_, query) = url.split_once('?')?;
    query.split('&').find_map(|pair| {
        let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
        (name == key).then(|| value.to_string())
    })
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
    let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();

    match (&method, segments.as_slice()) {
        (Method::Get, ["api", "acceptance"]) => {
            let state = shared.lock().unwrap();
            let now = now_ms();
            let rows = acceptance_rows(&state, now);
            let provider_health = provider_health_rows(&state, now);
            let storage_error = state.storage_error();
            drop(state);
            reply_json(
                request,
                200,
                serde_json::json!({
                    "window_ms": PENDING_AFTER_MS,
                    "quiet_after_ms": QUIET_AFTER_MS,
                    "providers": rows,
                    "provider_health": provider_health,
                    "storage_error": storage_error,
                }),
            );
        }

        (Method::Get, ["api", "status"]) => {
            let state = shared.lock().unwrap();
            let count = state.store.accounts.len();
            let provider_health = provider_health_rows(&state, now_ms());
            let by_provider = providers::ALL
                .iter()
                .map(|provider| {
                    let count = state
                        .store
                        .accounts
                        .iter()
                        .filter(|account| account.row.provider == provider.slug())
                        .count();
                    serde_json::json!({
                        "id": provider.slug(),
                        "label": provider.label(),
                        "inboxes": count,
                        "address_rule": provider.address_rule(),
                        "address_mode": match provider.address_mode() {
                            providers::AddressMode::LocalPart => "local-part",
                            providers::AddressMode::ProviderAssigned => "provider-assigned",
                        },
                    })
                })
                .collect::<Vec<_>>();
            let storage_error = state.storage_error();
            drop(state);
            reply_json(
                request,
                200,
                serde_json::json!({
                    "mode": "live",
                    "provider": "multiple",
                    "inboxes": count,
                    "ttl_minutes": 60,
                    "providers": by_provider,
                    "provider_health": provider_health,
                    "storage_error": storage_error,
                }),
            );
        }

        (Method::Get, ["api", "inboxes"]) => {
            let mut state = shared.lock().unwrap();
            let now = now_ms();
            state.purge_expired(now);
            let accounts = state.store.accounts.clone();
            let fresh = query_value(&raw_url, "fresh").is_some_and(|value| value == "1");
            let mut inboxes = Vec::new();
            for account in accounts {
                let list = list_for(&mut state, &account, fresh).unwrap_or_default();
                let unread = state.unread_count(&list);
                let last = list.first().map(|message| {
                    serde_json::json!({
                        "sender": message.from.display(),
                        "subject": message.subject,
                        "preview": preview(&message.intro, 90),
                        "received_at": message.received_at,
                    })
                });
                inboxes.push(serde_json::json!({
                    "id": account.row.id,
                    "address": account.row.address,
                    "provider": account.row.provider,
                    "created_at": account.row.created_at,
                    "expires_at": account.row.expires_at,
                    "unread": unread,
                    "last": last,
                }));
            }
            drop(state);
            reply_json(
                request,
                200,
                serde_json::json!({ "inboxes": inboxes }),
            );
        }

        (Method::Post, ["api", "inboxes"]) => {
            let mut body = String::new();
            let _ = request.as_reader().read_to_string(&mut body);
            #[derive(serde::Deserialize)]
            struct Request {
                provider: Option<String>,
            }
            let requested = serde_json::from_str::<Request>(&body)
                .ok()
                .and_then(|request| request.provider)
                .or_else(|| query_value(&raw_url, "provider"));
            let provider = match requested.as_deref() {
                None => providers::Provider::MailTm,
                Some(slug) => match providers::Provider::from_slug(slug) {
                    Some(provider) => provider,
                    None => {
                        return reply_json(
                            request,
                            400,
                            serde_json::json!({ "error": format!("unknown provider “{slug}”") }),
                        )
                    }
                },
            };
            let created = match add_inbox(shared, provider) {
                Ok(created) => created,
                Err(err) => return reply_json(request, 502, serde_json::json!({ "error": err })),
            };
            let storage_error = shared.lock().unwrap().storage_error();

            reply_json(
                request,
                200,
                serde_json::json!({
                    "id": created.row.id,
                    "address": created.row.address,
                    "provider": created.used_provider,
                    "requested_provider": created.requested_provider,
                    "fallback": created.fallback,
                    "attempts": created.failures.iter().map(|failure| serde_json::json!({
                        "provider": failure.provider,
                        "error": failure.error,
                    })).collect::<Vec<_>>(),
                    "storage_error": storage_error,
                }),
            );
        }

        (Method::Delete, ["api", "inboxes", id]) => {
            let mut state = shared.lock().unwrap();
            if let Some(account) = state.store.find(id) {
                let _ = providers::remove(&account);
            }
            state.remove_inbox(id);
            drop(state);
            reply(request, 200, String::new(), json_header());
        }

        (Method::Get, ["api", "inboxes", id, "messages"]) => {
            let mut state = shared.lock().unwrap();
            state.purge_expired(now_ms());
            let Some(account) = state.store.find(id) else {
                drop(state);
                return reply_json(request, 404, serde_json::json!({ "error": "Inbox not found" }));
            };

            // Respect the provider's cache so the 3s poll stays inside its
            // quota, unless the caller explicitly asks for fresh data.
            let fresh = query_value(&raw_url, "fresh").is_some_and(|value| value == "1");
            let list = match list_for(&mut state, &account, fresh) {
                Ok(list) => list,
                Err(err) => {
                    drop(state);
                    return reply_json(request, 502, serde_json::json!({ "error": err }));
                }
            };

            let seen_snapshot = state.seen.clone();
            let messages: Vec<serde_json::Value> = list
                .iter()
                .map(|m| {
                    serde_json::json!({
                        "id": m.id,
                        "sender": m.from.display(),
                        "subject": m.subject,
                        "preview": preview(&m.intro, 90),
                        "received_at": m.received_at,
                        "seen": m.seen || seen_snapshot.contains(&m.id),
                    })
                })
                .collect();
            drop(state);

            reply_json(
                request,
                200,
                serde_json::json!({ "count": messages.len(), "messages": messages }),
            );
        }

        (Method::Get, ["api", "messages", id]) => {
            let detail = {
                let mut state = shared.lock().unwrap();
                let detail = detail_for(&state, id);
                if let Some(detail) = &detail {
                    state.seen.insert(detail.id.clone());
                }
                detail
            };
            let Some(detail) = detail else {
                return reply_json(request, 404, serde_json::json!({ "error": "Message not found" }));
            };

            let html = detail.html.clone().unwrap_or_default();
            let text = match detail.text.clone() {
                Some(text) if !text.trim().is_empty() => text,
                _ => strip_tags(&html),
            };
            let code = extract_code(&text)
                .or_else(|| extract_code(&strip_tags(&html)))
                .unwrap_or_default();
            let link = extract_link(&text)
                .or_else(|| find_link_in_html(&html))
                .unwrap_or_default();

            reply_json(
                request,
                200,
                serde_json::json!({
                    "id": detail.id,
                    "sender": detail.from.display(),
                    "subject": detail.subject,
                    "text": text,
                    "code": code,
                    "link": link,
                    "received_at": detail.received_at,
                    "has_html": !html.trim().is_empty(),
                }),
            );
        }

        (Method::Get, ["api", "messages", id, "html"]) => {
            let mut html = {
                let state = shared.lock().unwrap();
                detail_for(&state, id)
                    .and_then(|detail| detail.html)
                    .unwrap_or_default()
            };
            if html.trim().is_empty() {
                html = "<p>This message has no HTML part.</p>".to_string();
            }
            let sanitized = sanitize_html(&html);
            let mut response = Response::from_string(sanitized)
                .with_status_code(200)
                .with_header(html_header())
                .with_header(
                    Header::from_bytes(
                        &b"Content-Security-Policy"[..],
                        &b"sandbox; default-src 'none'; img-src data: https:; style-src 'unsafe-inline'"[..],
                    )
                    .unwrap(),
                )
                .with_header(
                    Header::from_bytes(&b"Cache-Control"[..], &b"no-store"[..]).unwrap(),
                );
            let _ = response.add_header(
                Header::from_bytes(&b"X-Content-Type-Options"[..], &b"nosniff"[..]).unwrap(),
            );
            let _ = request.respond(response);
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
) -> Result<gui::EmailApp, String> {
    let auth_token = auth_token.trim().to_string();
    if auth_token.is_empty() {
        return Err("an install token is required".to_string());
    }
    let state_dir = state_dir.as_ref();
    let addr = format!("127.0.0.1:{port}");
    let server = Server::http(&addr).map_err(|err| format!("cannot listen on {addr}: {err}"))?;

    let store = store::Store::load(state_dir.join("inboxes.json"))?;
    let mut health = health::Health::load(state_dir.join("acceptance.json"))?;
    health.reconcile_with_store(&store);
    let state = State {
        store,
        health,
        lists: HashMap::new(),
        owners: HashMap::new(),
        seen: HashSet::new(),
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

    println!(
        "temp-email: API on http://{addr} ({} providers)",
        providers::ALL.len()
    );

    let server_shared = shared.clone();
    let server_guard = guard.clone();
    std::thread::spawn(move || {
        for request in server.incoming_requests() {
            let shared = server_shared.clone();
            let guard = server_guard.clone();
            std::thread::spawn(move || handle(request, &shared, &guard));
        }
    });

    let app = gui::EmailApp::new(shared.clone());
    background::start(shared);
    Ok(app)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::{Addr, Brief};
    use std::path::PathBuf;

    fn temp_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "gmo-temp-email-{name}-{}.json",
            std::process::id()
        ))
    }

    fn inbox(id: &str, address: &str, provider: &str, created_at: f64) -> store::Account {
        store::Account {
            row: store::InboxRow {
                id: id.to_string(),
                address: address.to_string(),
                provider: provider.to_string(),
                created_at,
                expires_at: created_at + TTL_MS,
                first_message_at: None,
                first_external_message_at: None,
                first_unclassified_message_at: None,
            },
            password: String::new(),
            token: String::new(),
            remote_id: String::new(),
        }
    }

    fn message(id: &str, from: &str, name: &str, subject: &str, at: f64) -> Brief {
        Brief {
            id: id.to_string(),
            from: Addr {
                address: from.to_string(),
                name: name.to_string(),
            },
            subject: subject.to_string(),
            intro: String::new(),
            seen: false,
            created_at: String::new(),
            received_at: at,
        }
    }

    fn state(store_path: &PathBuf, health_path: &PathBuf) -> State {
        State {
            store: store::Store::load(store_path).expect("store"),
            health: health::Health::load(health_path).expect("health"),
            lists: HashMap::new(),
            owners: HashMap::new(),
            seen: HashSet::new(),
        }
    }

    fn cleanup(store_path: &PathBuf, health_path: &PathBuf) {
        let _ = std::fs::remove_file(store_path);
        let _ = std::fs::remove_file(health_path);
        let backup = PathBuf::from(format!("{}.v1.bak", health_path.to_string_lossy()));
        let _ = std::fs::remove_file(backup);
    }

    #[test]
    fn unread_counts_use_provider_and_local_seen_state() {
        let store_path = temp_path("unread-store");
        let health_path = temp_path("unread-health");
        cleanup(&store_path, &health_path);
        let mut state = state(&store_path, &health_path);

        let unread = message(
            "unread",
            "sender@example.test",
            "Sender",
            "Unread message",
            1.0,
        );
        let mut provider_seen = message(
            "provider-seen",
            "sender@example.test",
            "Sender",
            "Provider-seen message",
            2.0,
        );
        provider_seen.seen = true;
        let local_seen = message(
            "local-seen",
            "sender@example.test",
            "Sender",
            "Locally seen message",
            3.0,
        );
        state.seen.insert(local_seen.id.clone());
        let messages = vec![unread, provider_seen, local_seen];

        assert!(state.is_message_seen(&messages[1]));
        assert!(state.is_message_seen(&messages[2]));
        assert_eq!(state.unread_count(&messages), 1);

        state
            .lists
            .insert("one".to_string(), (now_ms(), messages));
        assert_eq!(state.cached_unread("one"), Some(1));
        assert_eq!(state.cached_unread("missing"), None);

        drop(state);
        cleanup(&store_path, &health_path);
    }

    #[test]
    fn welcome_mail_is_ignored_and_external_mail_counts_once() {
        let store_path = temp_path("delivery-store");
        let health_path = temp_path("delivery-health");
        cleanup(&store_path, &health_path);
        let account = inbox("one", "one@guerrillamailblock.com", "guerrilla", now_ms());
        let mut state = state(&store_path, &health_path);
        state.store.push(account.clone());
        state.health.attempt("guerrilla", "guerrillamailblock.com");
        state.health.created("guerrilla", "guerrillamailblock.com");

        let welcome = message(
            "welcome",
            "no-reply@guerrillamail.com",
            "Guerrilla Mail",
            "Welcome to Guerrilla Mail",
            now_ms(),
        );
        record_messages(&mut state, &account, &[welcome]);
        assert_eq!(
            state.store.find("one").unwrap().row.first_external_message_at,
            None
        );
        assert_eq!(state.health.rows()[0].delivered, 0);

        let external = message(
            "external",
            "noreply@example.test",
            "Example",
            "Your verification code",
            now_ms(),
        );
        record_messages(&mut state, &account, std::slice::from_ref(&external));
        assert!(state
            .store
            .find("one")
            .unwrap()
            .row
            .first_external_message_at
            .is_some());
        assert_eq!(state.health.rows()[0].delivered, 1);

        record_messages(&mut state, &account, &[external]);
        assert_eq!(state.health.rows()[0].delivered, 1);

        drop(state);
        cleanup(&store_path, &health_path);
    }

    #[test]
    fn guerrilla_actual_domain_mail_does_not_increment_delivery() {
        let store_path = temp_path("guerrilla-domain-store");
        let health_path = temp_path("guerrilla-domain-health");
        cleanup(&store_path, &health_path);
        let account = inbox("one", "one@guerrillamailblock.com", "guerrilla", now_ms());
        let mut state = state(&store_path, &health_path);
        state.store.push(account.clone());
        state.health.attempt("guerrilla", "guerrillamailblock.com");
        state.health.created("guerrilla", "guerrillamailblock.com");

        let provider_mail = message(
            "provider",
            "no-reply@guerrillamailblock.com",
            "Guerrilla Mail",
            "Provider notice",
            now_ms(),
        );
        record_messages(&mut state, &account, &[provider_mail]);
        assert_eq!(
            state.store.find("one").unwrap().row.first_external_message_at,
            None
        );
        assert_eq!(state.health.rows()[0].delivered, 0);
        assert_eq!(state.health.rows()[0].unclassified, 1);

        drop(state);
        cleanup(&store_path, &health_path);
    }

    #[test]
    fn pending_and_quiet_counts_use_external_mail() {
        let store_path = temp_path("pending-store");
        let health_path = temp_path("pending-health");
        cleanup(&store_path, &health_path);
        let now = 1_000_000_000.0;
        let pre_pending = inbox(
            "pre-pending",
            "pre@example.test",
            "mail.tm",
            now - PENDING_AFTER_MS + 1.0,
        );
        let pending = inbox(
            "pending",
            "pending@example.test",
            "mail.tm",
            now - PENDING_AFTER_MS,
        );
        let late_pending = inbox(
            "late-pending",
            "late@example.test",
            "mail.tm",
            now - QUIET_AFTER_MS + 1.0,
        );
        let quiet = inbox(
            "quiet",
            "quiet@example.test",
            "mail.tm",
            now - QUIET_AFTER_MS,
        );
        let mut state = state(&store_path, &health_path);
        state.store.push(pre_pending);
        state.store.push(pending.clone());
        state.store.push(late_pending);
        state.store.push(quiet);
        for _ in 0..4 {
            state.health.attempt("mail.tm", "example.test");
            state.health.created("mail.tm", "example.test");
        }

        let rows = acceptance_rows(&state, now);
        let row = rows
            .iter()
            .find(|row| row.provider == "mail.tm" && row.domain == "example.test")
            .expect("domain row");
        assert_eq!(row.created, 4);
        assert_eq!(row.pending, 2);
        assert_eq!(row.quiet, 1);
        assert_eq!(row.delivered, 0);

        let external = message(
            "external",
            "noreply@external.test",
            "Example",
            "Your verification code",
            now,
        );
        record_messages(&mut state, &pending, &[external]);
        let rows = acceptance_rows(&state, now);
        let row = rows
            .iter()
            .find(|row| row.provider == "mail.tm" && row.domain == "example.test")
            .expect("domain row");
        assert_eq!(row.pending, 1);
        assert_eq!(row.quiet, 1);
        assert_eq!(row.delivered, 1);

        state.store.remove("late-pending");
        let rows = acceptance_rows(&state, now);
        let row = rows
            .iter()
            .find(|row| row.provider == "mail.tm" && row.domain == "example.test")
            .expect("domain row");
        assert_eq!(row.created, 4, "lifetime created count survives deletion");
        assert_eq!(row.pending, 0);
        assert_eq!(row.quiet, 1);

        drop(state);
        cleanup(&store_path, &health_path);
    }

    #[test]
    fn unclassified_provider_mail_does_not_increment_delivery() {
        let store_path = temp_path("unclassified-store");
        let health_path = temp_path("unclassified-health");
        cleanup(&store_path, &health_path);
        let account = inbox("one", "one@fldubai.com", "mail.tm", now_ms());
        let mut state = state(&store_path, &health_path);
        state.store.push(account.clone());
        state.health.attempt("mail.tm", "fldubai.com");
        state.health.created("mail.tm", "fldubai.com");

        let provider_mail = message(
            "provider",
            "welcome@mail.tm",
            "Mail.tm",
            "Welcome to Mail.tm",
            now_ms(),
        );
        record_messages(&mut state, &account, &[provider_mail]);
        assert_eq!(state.health.rows()[0].delivered, 0);
        assert_eq!(state.health.rows()[0].unclassified, 1);
        assert!(state
            .store
            .find("one")
            .unwrap()
            .row
            .first_external_message_at
            .is_none());

        record_messages(
            &mut state,
            &account,
            &[message(
                "provider-again",
                "welcome@mail.tm",
                "Mail.tm",
                "Welcome back",
                now_ms(),
            )],
        );
        assert_eq!(state.health.rows()[0].delivered, 0);
        assert_eq!(state.health.rows()[0].unclassified, 1);

        drop(state);
        cleanup(&store_path, &health_path);
    }

    #[test]
    fn expiry_and_delete_keep_terminal_outcomes_in_the_domain_ledger() {
        let store_path = temp_path("outcome-store");
        let health_path = temp_path("outcome-health");
        cleanup(&store_path, &health_path);
        let now = now_ms();
        let mut expired = inbox(
            "expired",
            "expired@outcome.test",
            "mail.tm",
            now - 20.0 * 60_000.0,
        );
        expired.row.expires_at = now - 1.0;
        let early = inbox(
            "early",
            "early@outcome.test",
            "mail.tm",
            now - 5.0 * 60_000.0,
        );
        let delivered = inbox(
            "delivered",
            "delivered@outcome.test",
            "mail.tm",
            now - 5.0 * 60_000.0,
        );
        let mut state = state(&store_path, &health_path);
        for account in [&expired, &early, &delivered] {
            state.store.push(account.clone());
            state.health.created("mail.tm", "outcome.test");
            state.health.start_outcome(&account.row);
        }

        state.purge_expired(now);
        assert!(state.store.find("expired").is_none());
        assert_eq!(
            state.health.outcome("expired").map(|outcome| outcome.state),
            Some(health::OutcomeState::Quiet)
        );

        state.remove_inbox("early");
        assert_eq!(
            state.health.outcome("early").map(|outcome| outcome.state),
            Some(health::OutcomeState::Unknown)
        );

        state
            .health
            .mark_outcome_delivered(&delivered.row, now - 1.0);
        state.remove_inbox("delivered");
        assert_eq!(
            state
                .health
                .outcome("delivered")
                .map(|outcome| outcome.state),
            Some(health::OutcomeState::Delivered)
        );

        let rows = state.health.rows();
        let row = rows
            .iter()
            .find(|row| row.domain == "outcome.test")
            .expect("outcome domain row");
        assert_eq!(row.created, 3);
        assert_eq!(row.delivered, 1);
        assert!(state.store.accounts.is_empty());

        drop(state);
        let reloaded = health::Health::load(&health_path).expect("health");
        assert_eq!(
            reloaded
                .outcome("expired")
                .map(|outcome| outcome.state),
            Some(health::OutcomeState::Quiet)
        );
        assert_eq!(
            reloaded
                .rows()
                .into_iter()
                .find(|row| row.domain == "outcome.test")
                .map(|row| (row.created, row.delivered)),
            Some((3, 1))
        );
        drop(reloaded);
        cleanup(&store_path, &health_path);
    }

    #[test]
    fn provider_wide_list_errors_are_separated_from_mailbox_errors() {
        assert!(provider_wide_list_error(
            "Guerrilla Mail: status code 502: bad gateway"
        ));
        assert!(provider_wide_list_error(
            "Catchmail: expected value at line 1 column 2"
        ));
        assert!(provider_wide_list_error("Mail.tm is rate limiting us"));
        assert!(!provider_wide_list_error(
            "Mail.tm: status code 404: mailbox not found"
        ));
    }

    #[test]
    fn provider_wide_errors_are_historical_without_active_inboxes() {
        let store_path = temp_path("provider-activity-store");
        let health_path = temp_path("provider-activity-health");
        cleanup(&store_path, &health_path);
        let now = now_ms();
        let mut state = state(&store_path, &health_path);
        state
            .health
            .provider_list_failed("guerrilla", "provider list unavailable");
        state
            .health
            .provider_list_failed("guerrilla", "provider list unavailable again");

        let inactive = provider_health_rows(&state, now);
        let row = inactive
            .iter()
            .find(|row| row.provider == "guerrilla")
            .expect("Guerrilla provider row");
        assert_eq!(row.list_failures, 2);
        assert_eq!(row.active_inboxes, 0);
        assert!(!row.error_is_current);
        assert_eq!(row.last_error, "");

        state.store.push(inbox(
            "active-guerrilla",
            "active@guerrillamailblock.com",
            "guerrilla",
            now,
        ));
        let active = provider_health_rows(&state, now);
        let row = active
            .iter()
            .find(|row| row.provider == "guerrilla")
            .expect("Guerrilla provider row");
        assert_eq!(row.list_failures, 2);
        assert_eq!(row.active_inboxes, 1);
        assert!(row.error_is_current);
        assert_eq!(row.last_error, "provider list unavailable again");

        drop(state);
        cleanup(&store_path, &health_path);
    }
}
