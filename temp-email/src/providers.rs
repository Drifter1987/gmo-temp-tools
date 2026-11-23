//! Provider layer for disposable mailboxes.
//!
//! mail.tm keeps its original client in `mailtm.rs`; the five other services are
//! implemented here behind the same small interface. All requests stay in this
//! process, so browser CORS rules and provider tokens never reach the page.

use crate::mailtm;
use crate::store::Account;
use crate::util::{now_ms, parse_iso_ms};
use serde::Deserialize;
use serde_json::Value;
use std::time::Duration;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Provider {
    MailTm,
    BestTempMail,
    Catchmail,
    GrabMail,
    Guerrilla,
    TempMailAi,
}

pub const ALL: [Provider; 6] = [
    Provider::MailTm,
    Provider::BestTempMail,
    Provider::Catchmail,
    Provider::GrabMail,
    Provider::Guerrilla,
    Provider::TempMailAi,
];

/// How the provider decides the mailbox address. Some providers accept a
/// caller-chosen local part on a fixed domain; others allocate the full address
/// themselves. Keeping the distinction explicit prevents a fallback from
/// sending an address the provider never agreed to host.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AddressMode {
    LocalPart,
    ProviderAssigned,
}

impl Provider {
    pub fn slug(self) -> &'static str {
        match self {
            Provider::MailTm => "mail.tm",
            Provider::BestTempMail => "best-tempmail",
            Provider::Catchmail => "catchmail",
            Provider::GrabMail => "grabmail",
            Provider::Guerrilla => "guerrilla",
            Provider::TempMailAi => "tempmail.ai",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Provider::MailTm => "Mail.tm",
            Provider::BestTempMail => "Best-TempMail",
            Provider::Catchmail => "Catchmail",
            Provider::GrabMail => "GrabMail",
            Provider::Guerrilla => "Guerrilla Mail",
            Provider::TempMailAi => "TempMail.ai",
        }
    }

    pub fn default_domain(self) -> &'static str {
        match self {
            Provider::MailTm => "mail.tm",
            Provider::BestTempMail => "best-tempmail.com",
            Provider::Catchmail => "catchmail.io",
            Provider::GrabMail => "grabmail.io",
            Provider::Guerrilla => "guerrillamail.com",
            Provider::TempMailAi => "tempmail.ai",
        }
    }

    pub fn address_mode(self) -> AddressMode {
        match self {
            Provider::MailTm | Provider::Catchmail | Provider::GrabMail => AddressMode::LocalPart,
            Provider::BestTempMail | Provider::Guerrilla | Provider::TempMailAi => {
                AddressMode::ProviderAssigned
            }
        }
    }

    /// A short, user-facing statement of the address rule. This is shown next
    /// to the provider picker and in `/api/status` so a fallback is predictable.
    pub fn address_rule(self) -> &'static str {
        match self {
            Provider::MailTm => "random local part on mail.tm's active domain",
            Provider::BestTempMail => "provider-assigned @best-tempmail.com",
            Provider::Catchmail => "random local part @catchmail.io",
            Provider::GrabMail => "random local part @grabmail.io",
            Provider::Guerrilla => "provider-assigned Guerrilla domain",
            Provider::TempMailAi => "provider-assigned @tempmail.ai",
        }
    }

    /// Requested provider first, then the other providers in a stable order.
    /// Every provider is attempted at most once by the fallback caller.
    pub fn fallback_order(requested: Self) -> Vec<Self> {
        let mut order = vec![requested];
        order.extend(ALL.into_iter().filter(|provider| *provider != requested));
        order
    }

    /// Validate the address the provider returned before storing credentials.
    /// This catches a provider changing its domain or returning a value from a
    /// different service, both of which would make later list calls fail with a
    /// confusing mailbox error.
    pub fn validate_address(self, address: &str) -> Result<(), String> {
        let address = address.trim();
        let (local, domain) = address
            .rsplit_once('@')
            .ok_or_else(|| "provider returned an address without @".to_string())?;
        if local.is_empty()
            || local.len() > 64
            || !local
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '+' | '-'))
        {
            return Err("provider returned an invalid local part".to_string());
        }
        if domain.is_empty()
            || !domain.contains('.')
            || domain.len() > 253
            || !domain
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-'))
        {
            return Err("provider returned an invalid domain".to_string());
        }
        let domain = domain.to_ascii_lowercase();
        let allowed = match self {
            Provider::Catchmail => domain == "catchmail.io",
            Provider::GrabMail => domain == "grabmail.io",
            Provider::BestTempMail => domain == "best-tempmail.com",
            Provider::TempMailAi => domain == "tempmail.ai" || domain.ends_with(".tempmail.ai"),
            Provider::Guerrilla => domain.contains("guerrillamail"),
            // mail.tm publishes its active domain at runtime; syntax is the
            // only stable check available before its API answers.
            Provider::MailTm => true,
        };
        if !allowed {
            return Err(format!(
                "provider returned an address on an unexpected domain: {domain}"
            ));
        }
        Ok(())
    }

    pub fn from_slug(value: &str) -> Option<Self> {
        ALL.into_iter().find(|provider| provider.slug() == value)
    }

    /// How long a message list may be reused. Providers with tighter free
    /// quotas get a longer cache so the three-second polling loop stays polite.
    pub fn cache_ttl_ms(self) -> f64 {
        match self {
            Provider::BestTempMail => 30_000.0,
            Provider::Catchmail => 5_000.0,
            _ => 4_000.0,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Addr {
    pub address: String,
    pub name: String,
}

impl Addr {
    pub fn display(&self) -> String {
        if self.name.is_empty() {
            self.address.clone()
        } else {
            format!("{} <{}>", self.name, self.address)
        }
    }
}

#[derive(Clone, Debug)]
pub struct Brief {
    pub id: String,
    pub from: Addr,
    pub subject: String,
    pub intro: String,
    pub seen: bool,
    pub created_at: String,
    pub received_at: f64,
}

/// Provider-generated welcome/system messages are not delivery evidence.
/// Keep this deliberately narrow: a false negative only delays a count for a
/// message the provider itself sent, while a false positive would treat a real
/// signup mail as noise.
pub fn is_system_message(provider: Provider, message: &Brief) -> bool {
    provider == Provider::Guerrilla
        && (message.from.address.eq_ignore_ascii_case("no-reply@guerrillamail.com")
            || (message
                .from
                .name
                .eq_ignore_ascii_case("Guerrilla Mail")
                && message
                    .subject
                    .eq_ignore_ascii_case("Welcome to Guerrilla Mail")))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MessageClass {
    External,
    System,
    Unclassified,
}

/// Classify a message without guessing that provider-origin mail is delivery
/// evidence. A sender on the provider's own default domain, a sender on the
/// inbox's actual domain, or a sender that cannot be parsed is recorded as
/// unclassified and never increments delivery.
pub fn classify_message(
    provider: Provider,
    inbox_domain: &str,
    message: &Brief,
) -> MessageClass {
    if is_system_message(provider, message) {
        return MessageClass::System;
    }
    let sender_domain = message
        .from
        .address
        .rsplit_once('@')
        .map(|(_, domain)| domain.trim().trim_end_matches('.'))
        .unwrap_or("");
    let inbox_domain = inbox_domain.trim().trim_end_matches('.');
    if sender_domain.is_empty()
        || sender_domain.eq_ignore_ascii_case(provider.default_domain())
        || sender_domain.eq_ignore_ascii_case(inbox_domain)
    {
        MessageClass::Unclassified
    } else {
        MessageClass::External
    }
}

#[derive(Clone, Debug)]
pub struct Detail {
    pub id: String,
    pub subject: String,
    pub from: Addr,
    pub text: Option<String>,
    pub html: Option<String>,
    pub received_at: f64,
}

pub struct Created {
    pub address: String,
    pub password: String,
    pub token: String,
    pub remote_id: String,
    pub expires_at: Option<f64>,
}

/// Shared mailbox-list shape for Best-TempMail, Catchmail, GrabMail and
/// TempMail.ai. TempMail.ai calls the array `items` and the timestamp
/// `received_at`; GrabMail adds `from_name`, `preview` and `seen`.
#[derive(Deserialize)]
struct ListResponse {
    #[serde(default, alias = "items")]
    messages: Vec<MailSummary>,
}

#[derive(Deserialize)]
struct MailSummary {
    id: String,
    #[serde(default)]
    from: String,
    #[serde(default)]
    from_name: String,
    #[serde(default)]
    subject: String,
    #[serde(default)]
    preview: String,
    #[serde(default, alias = "received_at")]
    date: String,
    #[serde(default)]
    seen: bool,
}

impl MailSummary {
    fn into_brief(self) -> Brief {
        let mut from = sender(&self.from);
        if from.name.is_empty() {
            from.name = self.from_name;
        }
        Brief {
            id: self.id,
            from,
            subject: self.subject,
            intro: self.preview,
            seen: self.seen,
            received_at: parse_iso_ms(&self.date),
            created_at: self.date,
        }
    }
}

#[derive(Deserialize, Default)]
struct MessageBody {
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    html: Option<String>,
}

/// Shared detail shape. Catchmail nests the parts under `body`; the other
/// providers put `text` and `html` at the top level.
#[derive(Deserialize)]
struct FullMessage {
    id: String,
    #[serde(default)]
    from: String,
    #[serde(default)]
    from_name: String,
    #[serde(default)]
    subject: String,
    #[serde(default, alias = "received_at")]
    date: String,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    html: Option<String>,
    #[serde(default)]
    body: MessageBody,
}

impl FullMessage {
    fn into_detail(self) -> Detail {
        let FullMessage {
            id,
            from,
            from_name,
            subject,
            date,
            text,
            html,
            body,
        } = self;
        let mut from = sender(&from);
        if from.name.is_empty() {
            from.name = from_name;
        }
        let (text, html) = if body.text.is_some() || body.html.is_some() {
            (body.text, body.html)
        } else {
            (text, html)
        };
        Detail {
            id,
            subject,
            from,
            text,
            html,
            received_at: parse_iso_ms(&date),
        }
    }
}

#[derive(Deserialize)]
struct MessageEnvelope {
    message: FullMessage,
}

#[derive(Deserialize)]
struct GuerrillaList {
    #[serde(default)]
    list: Vec<GuerrillaSummary>,
}

#[derive(Deserialize)]
struct GuerrillaSummary {
    #[serde(default)]
    mail_id: Value,
    #[serde(default)]
    mail_from: String,
    #[serde(default)]
    mail_subject: String,
    #[serde(default)]
    mail_excerpt: String,
    #[serde(default)]
    mail_date: String,
    #[serde(default)]
    mail_timestamp: Value,
    #[serde(default)]
    mail_read: Value,
}

#[derive(Deserialize)]
struct GuerrillaMessage {
    #[serde(default)]
    mail_from: String,
    #[serde(default)]
    mail_subject: String,
    #[serde(default)]
    mail_body: String,
    #[serde(default)]
    mail_timestamp: Value,
}

fn parse_guerrilla_list(value: Value, account_id: &str) -> Result<Vec<Brief>, serde_json::Error> {
    let response: GuerrillaList = serde_json::from_value(value)?;
    Ok(response
        .list
        .into_iter()
        .map(|message| {
            let remote_id = digits(&message.mail_id);
            let timestamp = numeric(&message.mail_timestamp);
            let received_at = if timestamp > 0.0 {
                timestamp * 1000.0
            } else {
                now_ms()
            };
            Brief {
                id: format!("{account_id}~{remote_id}"),
                from: sender(&message.mail_from),
                subject: message.mail_subject,
                intro: message.mail_excerpt,
                seen: numeric(&message.mail_read) != 0.0,
                created_at: message.mail_date,
                received_at,
            }
        })
        .collect())
}

fn parse_guerrilla_detail(
    value: Value,
    id: &str,
) -> Result<Option<Detail>, serde_json::Error> {
    let message: GuerrillaMessage = serde_json::from_value(value)?;
    if message.mail_body.trim().is_empty() && message.mail_subject.trim().is_empty() {
        return Ok(None);
    }
    let timestamp = numeric(&message.mail_timestamp);
    let received_at = if timestamp > 0.0 {
        timestamp * 1000.0
    } else {
        now_ms()
    };
    let is_html = message.mail_body.trim_start().starts_with('<');
    let (text, html) = if is_html {
        (None, Some(message.mail_body))
    } else {
        (Some(message.mail_body), None)
    };
    Ok(Some(Detail {
        id: id.to_string(),
        subject: message.mail_subject,
        from: sender(&message.mail_from),
        text,
        html,
        received_at,
    }))
}

fn provider_of(account: &Account) -> Result<Provider, String> {
    Provider::from_slug(&account.row.provider)
        .ok_or_else(|| format!("unknown mail provider “{}”", account.row.provider))
}

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(8))
        .timeout(Duration::from_secs(20))
        .build()
}

fn describe(provider: Provider, err: impl std::fmt::Display) -> String {
    let text = err.to_string();
    let name = provider.label();
    if text.contains("429") {
        format!("{name} is rate limiting us; try again in a moment")
    } else if text.contains("timed out") || text.contains("timeout") {
        format!("{name} did not answer in time")
    } else {
        format!("{name}: {text}")
    }
}

fn enc(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

fn get_json<T: serde::de::DeserializeOwned>(
    provider: Provider,
    url: &str,
    bearer: Option<&str>,
) -> Result<T, String> {
    let mut request = agent().get(url);
    if let Some(token) = bearer {
        request = request.set("Authorization", &format!("Bearer {token}"));
    }
    match request.call() {
        Ok(response) => response.into_json().map_err(|err| describe(provider, err)),
        Err(err) => Err(describe(provider, err)),
    }
}

fn post_json<T: serde::de::DeserializeOwned>(
    provider: Provider,
    url: &str,
    body: Value,
    bearer: Option<&str>,
) -> Result<T, String> {
    let mut request = agent().post(url).set("Content-Type", "application/json");
    if let Some(token) = bearer {
        request = request.set("Authorization", &format!("Bearer {token}"));
    }
    match request.send_json(body) {
        Ok(response) => response.into_json().map_err(|err| describe(provider, err)),
        Err(err) => Err(describe(provider, err)),
    }
}

fn parse_list(value: Value) -> Result<Vec<Brief>, serde_json::Error> {
    let value = value.get("data").cloned().unwrap_or(value);
    let response: ListResponse = serde_json::from_value(value)?;
    Ok(response
        .messages
        .into_iter()
        .map(MailSummary::into_brief)
        .collect())
}

fn parse_detail(value: Value) -> Result<Detail, serde_json::Error> {
    let response: FullMessage = serde_json::from_value(value)?;
    Ok(response.into_detail())
}

fn parse_envelope(value: Value) -> Result<Detail, serde_json::Error> {
    let response: MessageEnvelope = serde_json::from_value(value)?;
    Ok(response.message.into_detail())
}

fn parse_tempmail_detail(value: Value) -> Result<Detail, serde_json::Error> {
    let value = value.get("data").cloned().unwrap_or(value);
    parse_detail(value)
}

fn list_messages(
    provider: Provider,
    url: &str,
    bearer: Option<&str>,
) -> Result<Vec<Brief>, String> {
    let value: Value = get_json(provider, url, bearer)?;
    parse_list(value).map_err(|err| describe(provider, err))
}

fn detail_message(
    provider: Provider,
    url: &str,
    bearer: Option<&str>,
) -> Result<Detail, String> {
    let value: Value = get_json(provider, url, bearer)?;
    parse_detail(value).map_err(|err| describe(provider, err))
}

fn delete(provider: Provider, url: &str) -> Result<(), String> {
    match agent().delete(url).call() {
        Ok(_) => Ok(()),
        Err(err) => Err(describe(provider, err)),
    }
}

fn sender(raw: &str) -> Addr {
    let raw = raw.trim();
    if let Some(open) = raw.rfind('<') {
        if let Some(close_rel) = raw[open..].find('>') {
            let address = raw[open + 1..open + close_rel].trim().to_string();
            let name = raw[..open].trim().trim_matches('"').to_string();
            if !address.is_empty() {
                return Addr { address, name };
            }
        }
    }
    Addr {
        address: raw.to_string(),
        name: String::new(),
    }
}

fn digits(value: &Value) -> String {
    value
        .as_str()
        .map(str::to_string)
        .unwrap_or_else(|| value.to_string())
}

fn numeric(value: &Value) -> f64 {
    value
        .as_f64()
        .or_else(|| value.as_str().and_then(|text| text.trim().parse().ok()))
        .unwrap_or(0.0)
}

pub fn create(provider: Provider) -> Result<Created, String> {
    match provider {
        Provider::MailTm => {
            let domain = mailtm::active_domain()?;
            let address = crate::new_address(&domain);
            let password = format!("Gmo-{}-{}", crate::rand_hex(24), crate::rand_hex(12));
            mailtm::create(&address, &password)?;
            let (token, remote_id) = mailtm::token(&address, &password)?;
            Ok(Created {
                address,
                password,
                token,
                remote_id,
                expires_at: None,
            })
        }
        Provider::BestTempMail => {
            #[derive(Deserialize)]
            struct CreatedResponse {
                address: String,
                #[serde(default)]
                expires_at: String,
            }
            let created: CreatedResponse = post_json(
                provider,
                "https://api.best-tempmail.com/v1/inboxes",
                serde_json::json!({}),
                None,
            )?;
            Ok(Created {
                address: created.address,
                password: String::new(),
                token: String::new(),
                remote_id: String::new(),
                expires_at: if created.expires_at.is_empty() {
                    None
                } else {
                    Some(parse_iso_ms(&created.expires_at))
                },
            })
        }
        Provider::Catchmail => Ok(Created {
            address: crate::new_address("catchmail.io"),
            password: String::new(),
            token: String::new(),
            remote_id: String::new(),
            expires_at: None,
        }),
        Provider::GrabMail => Ok(Created {
            address: crate::new_address("grabmail.io"),
            password: String::new(),
            token: String::new(),
            remote_id: String::new(),
            expires_at: None,
        }),
        Provider::Guerrilla => {
            #[derive(Deserialize)]
            struct CreatedResponse {
                email_addr: String,
                sid_token: String,
            }
            let created: CreatedResponse = get_json(
                provider,
                "https://api.guerrillamail.com/ajax.php?f=get_email_address",
                None,
            )?;
            Ok(Created {
                address: created.email_addr,
                password: String::new(),
                token: created.sid_token,
                remote_id: String::new(),
                expires_at: None,
            })
        }
        Provider::TempMailAi => {
            #[derive(Deserialize)]
            struct Account {
                id: String,
                address: String,
                #[serde(default)]
                password: String,
                #[serde(default)]
                expires_at: String,
            }
            #[derive(Deserialize)]
            struct Token {
                token: String,
            }
            let account: Account = post_json(
                provider,
                "https://www.tempmail.ai/api/v1/accounts",
                serde_json::json!({}),
                None,
            )?;
            let token: Token = post_json(
                provider,
                "https://www.tempmail.ai/api/v1/token",
                serde_json::json!({
                    "address": account.address,
                    "password": account.password,
                }),
                None,
            )?;
            Ok(Created {
                address: account.address,
                password: account.password,
                token: token.token,
                remote_id: account.id,
                expires_at: if account.expires_at.is_empty() {
                    None
                } else {
                    Some(parse_iso_ms(&account.expires_at))
                },
            })
        }
    }
}

/// Retry only failures that can plausibly succeed on a second request. A 404,
/// authentication failure or malformed payload is not retried; it is surfaced
/// immediately so the caller can fall back or repair the account.
pub fn is_transient_error(error: &str) -> bool {
    let error = error.to_ascii_lowercase();
    [
        "429",
        "rate limit",
        "timed out",
        "timeout",
        "connection",
        "network",
        "dns",
        "status code 5",
        "server error",
        "bad gateway",
        "service unavailable",
        "temporarily unavailable",
    ]
    .iter()
    .any(|needle| error.contains(needle))
}

pub const RETRY_DELAYS_MS: [u64; 2] = [300, 900];

fn with_retry<T>(
    mut operation: impl FnMut() -> Result<T, String>,
) -> Result<T, String> {
    let mut last_error = String::new();
    for attempt in 0..=RETRY_DELAYS_MS.len() {
        match operation() {
            Ok(value) => return Ok(value),
            Err(error) => {
                if attempt == RETRY_DELAYS_MS.len() || !is_transient_error(&error) {
                    return Err(error);
                }
                last_error = error;
                std::thread::sleep(std::time::Duration::from_millis(
                    RETRY_DELAYS_MS[attempt],
                ));
            }
        }
    }
    Err(last_error)
}

/// Create a mailbox with one transient retry. The caller owns provider
/// fallback after this returns an error.
pub fn create_with_retry(provider: Provider) -> Result<Created, String> {
    with_retry(|| {
        let created = create(provider)?;
        provider.validate_address(&created.address)?;
        Ok(created)
    })
}

/// List a mailbox with a short transient retry. The retry is deliberately
/// bounded: the background poller and the UI can still move on to another
/// inbox or show a health warning rather than waiting on an outage.
pub fn list_with_retry(account: &Account) -> Result<Vec<Brief>, String> {
    with_retry(|| list(account))
}

pub fn list(account: &Account) -> Result<Vec<Brief>, String> {
    let provider = provider_of(account)?;
    match provider {
        Provider::MailTm => Ok(mailtm::list(&account.token)?
            .into_iter()
            .map(|message| Brief {
                id: message.id,
                from: Addr {
                    address: message.from.address,
                    name: message.from.name,
                },
                subject: message.subject,
                intro: message.intro,
                seen: message.seen,
                received_at: parse_iso_ms(&message.created_at),
                created_at: message.created_at,
            })
            .collect()),
        Provider::BestTempMail => list_messages(
            provider,
            &format!(
                "https://api.best-tempmail.com/v1/inboxes/{}/messages",
                enc(&account.row.address)
            ),
            None,
        ),
        Provider::Catchmail => list_messages(
            provider,
            &format!(
                "https://api.catchmail.io/api/v1/mailbox?address={}",
                enc(&account.row.address)
            ),
            None,
        ),
        Provider::GrabMail => list_messages(
            provider,
            &format!(
                "https://grabmail.io/api/v1/mailbox?address={}",
                enc(&account.row.address)
            ),
            None,
        ),
        Provider::Guerrilla => {
            let value: Value = get_json(
                provider,
                &format!(
                    "https://api.guerrillamail.com/ajax.php?f=get_email_list&offset=0&sid_token={}",
                    enc(&account.token)
                ),
                None,
            )?;
            parse_guerrilla_list(value, &account.row.id).map_err(|err| describe(provider, err))
        }
        Provider::TempMailAi => list_messages(
            provider,
            "https://www.tempmail.ai/api/v1/messages",
            Some(&account.token),
        ),
    }
}

pub fn detail(account: &Account, id: &str) -> Result<Detail, String> {
    let provider = provider_of(account)?;
    match provider {
        Provider::MailTm => {
            let detail = mailtm::detail(&account.token, id)?;
            Ok(Detail {
                id: detail.id,
                subject: detail.subject,
                from: Addr {
                    address: detail.from.address,
                    name: detail.from.name,
                },
                text: detail.text,
                html: detail.html.map(|parts| parts.join("\n")),
                received_at: parse_iso_ms(&detail.created_at),
            })
        }
        Provider::BestTempMail => {
            let value: Value = get_json(
                provider,
                &format!(
                    "https://api.best-tempmail.com/v1/inboxes/{}/messages/{}",
                    enc(&account.row.address),
                    enc(id)
                ),
                None,
            )?;
            parse_envelope(value).map_err(|err| describe(provider, err))
        }
        Provider::Catchmail => detail_message(
            provider,
            &format!(
                "https://api.catchmail.io/api/v1/message/{}?mailbox={}",
                enc(id),
                enc(&account.row.address)
            ),
            None,
        ),
        Provider::GrabMail => detail_message(
            provider,
            &format!(
                "https://grabmail.io/api/v1/message/{}?mailbox={}",
                enc(id),
                enc(&account.row.address)
            ),
            None,
        ),
        Provider::Guerrilla => {
            let remote_id = id.rsplit_once('~').map(|(_, tail)| tail).unwrap_or(id);
            let value: Value = get_json(
                provider,
                &format!(
                    "https://api.guerrillamail.com/ajax.php?f=fetch_email&email_id={}&sid_token={}",
                    enc(remote_id),
                    enc(&account.token)
                ),
                None,
            )?;
            parse_guerrilla_detail(value, id)
                .map_err(|err| describe(provider, err))?
                .ok_or_else(|| "Guerrilla Mail: message not found".to_string())
        }
        Provider::TempMailAi => {
            let value: Value = get_json(
                provider,
                &format!(
                    "https://www.tempmail.ai/api/v1/messages/{}",
                    enc(id)
                ),
                Some(&account.token),
            )?;
            parse_tempmail_detail(value).map_err(|err| describe(provider, err))
        }
    }
}

pub fn remove(account: &Account) -> Result<(), String> {
    let provider = provider_of(account)?;
    match provider {
        Provider::MailTm => mailtm::remove_account(&account.token, &account.remote_id),
        Provider::BestTempMail => delete(
            provider,
            &format!(
                "https://api.best-tempmail.com/v1/inboxes/{}",
                enc(&account.row.address)
            ),
        ),
        Provider::Guerrilla => {
            let _ = get_json::<Value>(
                provider,
                &format!(
                    "https://api.guerrillamail.com/ajax.php?f=forget_me&sid_token={}",
                    enc(&account.token)
                ),
                None,
            );
            Ok(())
        }
        Provider::Catchmail | Provider::GrabMail | Provider::TempMailAi => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_slugs_round_trip() {
        for provider in ALL {
            assert_eq!(Provider::from_slug(provider.slug()), Some(provider));
        }
    }

    #[test]
    fn fallback_order_starts_with_the_requested_provider_and_is_stable() {
        let order = Provider::fallback_order(Provider::Catchmail);
        assert_eq!(order.first(), Some(&Provider::Catchmail));
        assert_eq!(order.len(), ALL.len());
        assert!(order
            .iter()
            .skip(1)
            .eq(ALL.iter().filter(|provider| **provider != Provider::Catchmail)));
        let mut unique = order.clone();
        unique.sort_by_key(|provider| provider.slug());
        unique.dedup_by_key(|provider| provider.slug());
        assert_eq!(unique.len(), ALL.len());
    }

    #[test]
    fn provider_address_rules_accept_only_the_provider_shape() {
        assert!(Provider::Catchmail.validate_address("quiet-otter@catchmail.io").is_ok());
        assert!(Provider::Catchmail.validate_address("quiet-otter@example.test").is_err());
        assert!(Provider::GrabMail.validate_address("quiet-otter@grabmail.io").is_ok());
        assert!(Provider::GrabMail.validate_address("quiet-otter@catchmail.io").is_err());
        assert!(Provider::BestTempMail
            .validate_address("provider-name@best-tempmail.com")
            .is_ok());
        assert!(Provider::BestTempMail
            .validate_address("provider-name@tempmail.ai")
            .is_err());
        assert!(Provider::TempMailAi
            .validate_address("provider-name@eu.tempmail.ai")
            .is_ok());
        assert!(Provider::Guerrilla
            .validate_address("provider-name@guerrillamailblock.com")
            .is_ok());
        assert!(Provider::MailTm
            .validate_address("local-part@some-active-domain.test")
            .is_ok());
        assert!(Provider::MailTm.validate_address("not-an-address").is_err());
        assert!(Provider::MailTm.validate_address("bad local@mail.tm").is_err());
    }

    #[test]
    fn retry_classification_ignores_permanent_failures() {
        assert!(is_transient_error("HTTP 429: rate limited"));
        assert!(is_transient_error("request timed out"));
        assert!(is_transient_error("status code 503"));
        assert!(!is_transient_error("HTTP 404: mailbox not found"));
        assert!(!is_transient_error("invalid token"));
    }

    #[test]
    fn excludes_only_the_known_guerrilla_welcome() {
        let welcome = Brief {
            id: "welcome".to_string(),
            from: Addr {
                address: "no-reply@guerrillamail.com".to_string(),
                name: "Guerrilla Mail".to_string(),
            },
            subject: "Welcome to Guerrilla Mail".to_string(),
            intro: String::new(),
            seen: false,
            created_at: String::new(),
            received_at: 1.0,
        };
        assert!(is_system_message(Provider::Guerrilla, &welcome));

        let external = Brief {
            id: "external".to_string(),
            from: Addr {
                address: "noreply@example.test".to_string(),
                name: "Example".to_string(),
            },
            subject: "Your verification code".to_string(),
            intro: String::new(),
            seen: false,
            created_at: String::new(),
            received_at: 2.0,
        };
        assert!(!is_system_message(Provider::Guerrilla, &external));
        assert!(!is_system_message(Provider::MailTm, &welcome));
        assert_eq!(
            classify_message(Provider::Guerrilla, "guerrillamailblock.com", &welcome),
            MessageClass::System
        );
        assert_eq!(
            classify_message(Provider::Guerrilla, "guerrillamailblock.com", &external),
            MessageClass::External
        );

        let provider_mail = Brief {
            from: Addr {
                address: "welcome@mail.tm".to_string(),
                name: "Mail.tm".to_string(),
            },
            ..external.clone()
        };
        assert_eq!(
            classify_message(Provider::MailTm, "fldubai.com", &provider_mail),
            MessageClass::Unclassified
        );
        let unparseable = Brief {
            from: Addr {
                address: String::new(),
                name: String::new(),
            },
            ..external
        };
        assert_eq!(
            classify_message(Provider::MailTm, "fldubai.com", &unparseable),
            MessageClass::Unclassified
        );
    }

    #[test]
    fn classifies_guerrilla_domain_fixture_as_unclassified() {
        let fixture = fixture("guerrilla-domain");
        let list = parse_guerrilla_list(fixture["list"].clone(), "local-1").unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(
            list[0].from.address,
            "no-reply@guerrillamailblock.com"
        );
        assert_eq!(
            classify_message(
                Provider::Guerrilla,
                "guerrillamailblock.com",
                &list[0]
            ),
            MessageClass::Unclassified
        );
        assert_eq!(
            classify_message(
                Provider::Guerrilla,
                "guerrillamailblock.com",
                &list[1]
            ),
            MessageClass::External
        );
    }

    #[test]
    fn splits_display_senders() {
        let parsed = sender("Acme <no-reply@acme.test>");
        assert_eq!(parsed.name, "Acme");
        assert_eq!(parsed.address, "no-reply@acme.test");
        assert_eq!(parsed.display(), "Acme <no-reply@acme.test>");
        let bare = sender("no-reply@acme.test");
        assert_eq!(bare.name, "");
        assert_eq!(bare.address, "no-reply@acme.test");
        assert_eq!(bare.display(), "no-reply@acme.test");
    }

    #[test]
    fn encodes_addresses_for_paths_and_queries() {
        assert_eq!(enc("a+b@example.test"), "a%2Bb%40example.test");
    }

    #[test]
    fn parses_shared_message_shapes() {
        let list: ListResponse = serde_json::from_value(serde_json::json!({
            "items": [{
                "id": "m1",
                "from": "Acme <no-reply@acme.test>",
                "from_name": "Acme",
                "subject": "Hello",
                "preview": "A short preview",
                "received_at": "2026-11-14T22:13:20+00:00",
                "seen": true
            }]
        }))
        .unwrap();
        let brief = list.messages.into_iter().next().unwrap().into_brief();
        assert_eq!(brief.id, "m1");
        assert_eq!(brief.from.address, "no-reply@acme.test");
        assert_eq!(brief.from.name, "Acme");
        assert_eq!(brief.intro, "A short preview");
        assert!(brief.seen);

        let full: FullMessage = serde_json::from_value(serde_json::json!({
            "id": "m2",
            "from": "no-reply@acme.test",
            "from_name": "Acme",
            "subject": "Nested body",
            "body": { "text": "plain body" }
        }))
        .unwrap();
        let detail = full.into_detail();
        assert_eq!(detail.id, "m2");
        assert_eq!(detail.from.name, "Acme");
        assert_eq!(detail.text.as_deref(), Some("plain body"));

        let envelope: MessageEnvelope = serde_json::from_value(serde_json::json!({
            "message": {
                "id": "m3",
                "from": "no-reply@acme.test",
                "subject": "Top-level body",
                "text": "body text"
            }
        }))
        .unwrap();
        assert_eq!(
            envelope.message.into_detail().text.as_deref(),
            Some("body text")
        );
    }

    fn fixture(name: &str) -> Value {
        let raw = match name {
            "best-tempmail" => include_str!("../tests/fixtures/best-tempmail.json"),
            "tempmail-ai" => include_str!("../tests/fixtures/tempmail-ai.json"),
            "catchmail" => include_str!("../tests/fixtures/catchmail.json"),
            "grabmail" => include_str!("../tests/fixtures/grabmail.json"),
            "guerrilla" => include_str!("../tests/fixtures/guerrilla.json"),
            "guerrilla-domain" => include_str!("../tests/fixtures/guerrilla-domain.json"),
            other => panic!("unknown fixture {other}"),
        };
        serde_json::from_str(raw).expect("fixture is valid JSON")
    }

    #[test]
    fn parses_best_tempmail_list_and_detail_fixtures() {
        let fixture = fixture("best-tempmail");
        let list = parse_list(fixture["list"].clone()).unwrap();
        assert_eq!(list.len(), 1);
        let brief = &list[0];
        assert_eq!(brief.id, "a1b2c3d4e5f6a7b8");
        assert_eq!(brief.from.address, "noreply@service.com");
        assert_eq!(brief.from.name, "");
        assert_eq!(brief.subject, "Your verification code");
        assert_eq!(brief.intro, "");
        assert!(!brief.seen);
        assert_eq!(
            brief.received_at,
            parse_iso_ms("2026-07-04T20:01:00.000Z")
        );

        let detail = parse_envelope(fixture["detail"].clone()).unwrap();
        assert_eq!(detail.id, "a1b2c3d4e5f6a7b8");
        assert_eq!(detail.from.name, "Service");
        assert_eq!(detail.from.address, "noreply@service.com");
        assert_eq!(detail.text.as_deref(), Some("Your code is 123456"));
        assert_eq!(detail.html.as_deref(), Some("<p>Your code is 123456</p>"));
    }

    #[test]
    fn parses_tempmail_ai_data_wrapped_list_and_detail_fixtures() {
        let fixture = fixture("tempmail-ai");
        let list = parse_list(fixture["list"].clone()).unwrap();
        assert_eq!(list.len(), 1);
        let brief = &list[0];
        assert_eq!(brief.id, "5a1b2c3d-4e5f-6a7b-8c9d-0e1f2a3b4c5d");
        assert_eq!(brief.from.address, "noreply@github.com");
        assert_eq!(brief.subject, "Your verification code");
        assert_eq!(brief.intro, "");
        assert!(!brief.seen);
        assert_eq!(
            brief.received_at,
            parse_iso_ms("2026-11-13T20:01:00Z")
        );

        let detail = parse_tempmail_detail(fixture["detail"].clone()).unwrap();
        assert_eq!(detail.from.address, "noreply@github.com");
        assert_eq!(detail.text.as_deref(), Some("Use code 482913 to continue."));
        assert_eq!(
            detail.html.as_deref(),
            Some("<p>Use code 482913 to continue.</p>")
        );
        assert_eq!(
            detail.received_at,
            parse_iso_ms("2026-11-13T20:01:00Z")
        );
    }

    #[test]
    fn parses_catchmail_list_and_nested_body_fixtures() {
        let fixture = fixture("catchmail");
        let list = parse_list(fixture["list"].clone()).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, "abc123");
        assert_eq!(list[0].from.address, "sender@example.com");
        assert_eq!(list[0].subject, "Welcome!");
        assert_eq!(
            list[0].received_at,
            parse_iso_ms("2024-01-15T10:30:00+00:00")
        );

        let detail = parse_detail(fixture["detail"].clone()).unwrap();
        assert_eq!(detail.text.as_deref(), Some("Plain text content"));
        assert_eq!(detail.html.as_deref(), Some("<html>HTML content</html>"));
    }

    #[test]
    fn parses_grabmail_list_metadata_and_plain_detail_fixtures() {
        let fixture = fixture("grabmail");
        let list = parse_list(fixture["list"].clone()).unwrap();
        assert_eq!(list.len(), 1);
        let brief = &list[0];
        assert_eq!(brief.id, "01JR8W2K4Q");
        assert_eq!(brief.from.name, "Example");
        assert_eq!(brief.from.address, "no-reply@example.com");
        assert_eq!(brief.intro, "Your code is 481920. It expires in 10 minutes.");
        assert!(brief.seen);

        let detail = parse_detail(fixture["detail"].clone()).unwrap();
        assert_eq!(
            detail.text.as_deref(),
            Some("Your code is 481920. It expires in 10 minutes.")
        );
        assert_eq!(detail.html, None);
    }

    #[test]
    fn accepts_string_numeric_guerrilla_fields() {
        let list = parse_guerrilla_list(
            serde_json::json!({
                "list": [{
                    "mail_id": "1",
                    "mail_from": "sender@example.test",
                    "mail_subject": "Hello",
                    "mail_excerpt": "Body",
                    "mail_date": "2026-11-15 05:46:00",
                    "mail_timestamp": "0",
                    "mail_read": "0"
                }]
            }),
            "local-1",
        )
        .unwrap();
        assert_eq!(list.len(), 1);
        assert!(!list[0].seen);
        assert!(list[0].received_at > 0.0);

        let detail = parse_guerrilla_detail(
            serde_json::json!({
                "mail_from": "sender@example.test",
                "mail_subject": "Hello",
                "mail_body": "Body",
                "mail_timestamp": "0"
            }),
            "local-1~1",
        )
        .unwrap()
        .expect("detail");
        assert!(detail.received_at > 0.0);
    }

    #[test]
    fn parses_guerrilla_list_and_html_detail_fixtures() {
        let fixture = fixture("guerrilla");
        let list = parse_guerrilla_list(fixture["list"].clone(), "local-1").unwrap();
        assert_eq!(list.len(), 1);
        let brief = &list[0];
        assert_eq!(brief.id, "local-1~425");
        assert_eq!(brief.from.name, "Guerrilla Mail");
        assert_eq!(brief.from.address, "no-reply@example.com");
        assert_eq!(brief.created_at, "2026-11-13 20:01:00");
        assert!(!brief.seen);
        assert_eq!(brief.received_at, 1_780_000_000_000.0);

        let detail = parse_guerrilla_detail(fixture["detail"].clone(), "local-1~425")
            .unwrap()
            .expect("fixture has a body");
        assert_eq!(detail.id, "local-1~425");
        assert_eq!(detail.from.name, "Guerrilla Mail");
        assert_eq!(detail.text, None);
        assert_eq!(detail.html.as_deref(), Some("<p>Use code 482913 to continue.</p>"));
        assert_eq!(detail.received_at, 1_780_000_000_000.0);
    }
}
