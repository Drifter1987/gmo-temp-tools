//! Thin client for the public mail.tm API (free disposable mailboxes).
//!
//! Everything runs server-side here, so the browser talks only to localhost and
//! the API's missing CORS headers never matter.

use serde::Deserialize;
use std::time::Duration;

const BASE: &str = "https://api.mail.tm";

#[derive(Deserialize)]
struct Domains {
    #[serde(rename = "hydra:member")]
    member: Vec<Domain>,
}

#[derive(Deserialize)]
struct Domain {
    domain: String,
    #[serde(rename = "isActive")]
    is_active: bool,
}

#[derive(Deserialize, Clone)]
pub struct Addr {
    #[serde(default)]
    pub address: String,
    #[serde(default)]
    pub name: String,
}

#[derive(Deserialize, Clone)]
pub struct Brief {
    pub id: String,
    pub from: Addr,
    #[serde(default)]
    pub subject: String,
    #[serde(default)]
    pub intro: String,
    #[serde(default)]
    pub seen: bool,
    #[serde(rename = "createdAt")]
    pub created_at: String,
}

#[derive(Deserialize)]
pub struct Detail {
    pub id: String,
    #[serde(default)]
    pub subject: String,
    pub from: Addr,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub html: Option<Vec<String>>,
    #[serde(rename = "createdAt")]
    pub created_at: String,
}

#[derive(Deserialize)]
struct List {
    #[serde(rename = "hydra:member")]
    member: Vec<Brief>,
}

#[derive(Deserialize)]
struct TokenResp {
    token: String,
    id: String,
}

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(8))
        .timeout(Duration::from_secs(20))
        .build()
}

fn describe(err: impl std::fmt::Display) -> String {
    let text = err.to_string();
    if text.contains("429") {
        "mail.tm is rate limiting us; try again in a moment".to_string()
    } else if text.contains("timed out") || text.contains("timeout") {
        "mail.tm did not answer in time".to_string()
    } else {
        format!("mail.tm: {text}")
    }
}

fn describe_create(err: ureq::Error) -> String {
    match err {
        ureq::Error::Status(code, response) => {
            let body = response.into_string().unwrap_or_default();
            let detail = serde_json::from_str::<serde_json::Value>(&body)
                .ok()
                .and_then(|value| {
                    value
                        .get("detail")
                        .and_then(|detail| detail.as_str())
                        .map(str::to_string)
                })
                .unwrap_or(body);
            format!("mail.tm: status code {code}: {detail}")
        }
        err => describe(err),
    }
}

pub fn active_domain() -> Result<String, String> {
    let body: Domains = agent()
        .get(&format!("{BASE}/domains"))
        .call()
        .map_err(describe)?
        .into_json()
        .map_err(describe)?;
    body.member
        .into_iter()
        .find(|d| d.is_active)
        .map(|d| d.domain)
        .ok_or_else(|| "mail.tm has no active domain right now".to_string())
}

pub fn create(address: &str, password: &str) -> Result<String, String> {
    #[derive(Deserialize)]
    struct Created {
        id: String,
    }
    let created: Created = agent()
        .post(&format!("{BASE}/accounts"))
        .send_json(serde_json::json!({ "address": address, "password": password }))
        .map_err(|err| format!("{} (address: {address})", describe_create(err)))?
        .into_json()
        .map_err(describe)?;
    Ok(created.id)
}

pub fn token(address: &str, password: &str) -> Result<(String, String), String> {
    let got: TokenResp = agent()
        .post(&format!("{BASE}/token"))
        .send_json(serde_json::json!({ "address": address, "password": password }))
        .map_err(describe)?
        .into_json()
        .map_err(describe)?;
    Ok((got.token, got.id))
}

/// mail.tm answers 429 when requests bunch up; a single short retry covers the
/// burst caused by a page loading several inboxes at once.
fn get_json<T: serde::de::DeserializeOwned>(url: &str, token: Option<&str>) -> Result<T, String> {
    let mut attempt = 0;
    loop {
        let mut request = agent().get(url);
        if let Some(token) = token {
            request = request.set("Authorization", &format!("Bearer {token}"));
        }
        match request.call() {
            Ok(response) => return response.into_json().map_err(describe),
            Err(ureq::Error::Status(429, _)) if attempt == 0 => {
                attempt += 1;
                std::thread::sleep(Duration::from_millis(450));
            }
            Err(err) => return Err(describe(err)),
        }
    }
}

pub fn list(token: &str) -> Result<Vec<Brief>, String> {
    let body: List = get_json(&format!("{BASE}/messages?page=1"), Some(token))?;
    Ok(body.member)
}

pub fn detail(token: &str, id: &str) -> Result<Detail, String> {
    get_json(&format!("{BASE}/messages/{id}"), Some(token))
}

pub fn remove_account(token: &str, id: &str) -> Result<(), String> {
    agent()
        .delete(&format!("{BASE}/accounts/{id}"))
        .set("Authorization", &format!("Bearer {token}"))
        .call()
        .map(|_| ())
        .map_err(describe)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_mailtm_list_and_detail_shapes() {
        let list: List = serde_json::from_str(
            r#"{
                "hydra:member": [{
                    "id": "m1",
                    "from": {"address": "sender@example.test", "name": "Sender"},
                    "subject": "Your code",
                    "intro": "Use 123456",
                    "seen": false,
                    "createdAt": "2025-01-02T03:04:05+00:00"
                }]
            }"#,
        )
        .unwrap();
        assert_eq!(list.member.len(), 1);
        assert_eq!(list.member[0].from.name, "Sender");
        assert_eq!(list.member[0].created_at, "2025-01-02T03:04:05+00:00");

        let detail: Detail = serde_json::from_str(
            r#"{
                "id": "m1",
                "subject": "Your code",
                "from": {"address": "sender@example.test", "name": "Sender"},
                "text": "Use 123456",
                "html": ["<p>Use 123456</p>"],
                "createdAt": "2025-01-02T03:04:05+00:00"
            }"#,
        )
        .unwrap();
        assert_eq!(detail.text.as_deref(), Some("Use 123456"));
        assert_eq!(detail.html.unwrap(), vec!["<p>Use 123456</p>".to_string()]);
    }
}
