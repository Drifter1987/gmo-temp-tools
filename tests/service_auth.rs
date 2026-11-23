//! HTTP-level checks for the install-token boundary shared by the service
//! listeners. These start real listeners on ephemeral local ports; they do not
//! open a native window.

use std::fs;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::time::Duration;

const TOKEN: &str = "service-auth-test-token";
const WEBHOOK_SECRET: &str = "service-auth-test-secret";

struct StateDir(PathBuf);

impl StateDir {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "gmo-service-auth-{name}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("create test state directory");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for StateDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn free_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("reserve a local test port");
    listener.local_addr().expect("local test port").port()
}

fn start_service<T>(
    label: &str,
    mut start: impl FnMut(u16) -> Result<T, String>,
) -> (T, u16) {
    let mut last_error = String::new();
    for _ in 0..20 {
        let port = free_port();
        match start(port) {
            Ok(app) => return (app, port),
            Err(error) => last_error = error,
        }
    }
    panic!("{label} did not start: {last_error}");
}

fn http(
    port: u16,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: &str,
) -> (u16, String) {
    let url = format!("http://127.0.0.1:{port}{path}");
    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(5))
        .build();
    let mut request = agent.request(method, &url);
    for (name, value) in headers {
        request = request.set(name, value);
    }
    let result = if body.is_empty() {
        request.call()
    } else {
        request
            .set("Content-Type", "application/json")
            .send_string(body)
    };
    match result {
        Ok(response) => (
            response.status(),
            response.into_string().unwrap_or_default(),
        ),
        Err(ureq::Error::Status(status, response)) => (
            status,
            response.into_string().unwrap_or_default(),
        ),
        Err(error) => panic!("{method} {path} on {port} failed: {error}"),
    }
}

fn redirect_response(port: u16) -> (u16, Option<String>) {
    let url = format!("http://127.0.0.1:{port}/");
    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(5))
        .redirects(0)
        .build();
    let result = agent.get(&url).call();
    let response = match result {
        Ok(response) => response,
        Err(ureq::Error::Status(_, response)) => response,
        Err(error) => panic!("GET / on {port} failed: {error}"),
    };
    (
        response.status(),
        response.header("location").map(str::to_string),
    )
}

fn assert_browser_token_boundary(port: u16) {
    let (status, location) = redirect_response(port);
    assert_eq!(status, 302, "a direct service page must redirect to the web app");
    assert_eq!(
        location.as_deref(),
        Some(local_auth::web_app_origin().as_str()),
        "the redirect must use the fixed local web-app origin"
    );

    let (status, _) = http(port, "GET", "/api/status", &[], "");
    assert_eq!(status, 401, "no-token request must be rejected");

    let (status, _) = http(
        port,
        "GET",
        "/api/status",
        &[("Authorization", &format!("Bearer {TOKEN}"))],
        "",
    );
    assert_eq!(status, 200, "valid token must be accepted");

    let (status, _) = http(
        port,
        "GET",
        "/api/status",
        &[
            ("Authorization", &format!("Bearer {TOKEN}")),
            ("Origin", "https://attacker.example"),
        ],
        "",
    );
    assert_eq!(status, 403, "a foreign Origin must be rejected");
}

#[test]
fn mail_service_enforces_token_and_origin() {
    let state = StateDir::new("mail");
    let (app, port) = start_service("temp-email", |port| {
        temp_email::start_with_auth(port, state.path(), TOKEN.to_string())
    });
    let _app = app;
    assert_browser_token_boundary(port);
}

#[test]
fn sms_service_enforces_token_and_origin() {
    let state = StateDir::new("sms");
    let (app, port) = start_service("temp-sms", |port| {
        temp_sms::start_with_auth(port, state.path(), TOKEN.to_string())
    });
    let _app = app;
    assert_browser_token_boundary(port);
}

#[test]
fn signed_sms_inbound_bypasses_the_browser_token() {
    std::env::set_var("WEBHOOK_SECRET", WEBHOOK_SECRET);
    let state = StateDir::new("sms-inbound");
    let (app, port) = start_service("temp-sms", |port| {
        temp_sms::start_with_auth(port, state.path(), TOKEN.to_string())
    });
    let _app = app;

    let body = r#"{"id":"auth-test-sms","from":"+15550001111","to":"+15550002222","body":"Your code is 123456"}"#;
    let (status, response) = http(
        port,
        "POST",
        "/api/inbound/generic",
        &[("x-gmo-secret", WEBHOOK_SECRET)],
        body,
    );
    assert_eq!(status, 200, "signed inbound must not require the browser token");
    assert!(response.contains("\"stored\":true"), "inbound message was not stored: {response}");
}
