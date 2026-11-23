//! The browser face of GMO TempTools. It lives on 127.0.0.1:8780, serves the
//! static single-page app, proxies the three service APIs and exposes the OTP
//! vault that the desktop tab also uses.

use crate::app_info::{APP_VERSION, OWNER, TOOL_RELEASES};
use crate::settings::{Appearance, Paths, Settings};
use std::io::Read;
use std::path::PathBuf;
use std::sync::Arc;
use std::thread;
use temp_otp::vault;
use tiny_http::{Header, Method, Response, Server};

const INDEX: &str = include_str!("../webapp/index.html");
const LOGIN: &str = include_str!("../webapp/login.html");
const CSS: &str = include_str!("../webapp/app.css");
const JS: &str = include_str!("../webapp/app.js");
const UI_JS: &str = include_str!("../web/ui.js");
const CSRF_HEADER: &str = "X-TempTools-CSRF";
const COOKIE_NAME: &str = "temptools_token";

pub struct Config {
    pub port: u16,
    pub auth_token: String,
    pub mail_port: u16,
    pub sms_port: u16,
    pub otp_vault: PathBuf,
}

pub fn start(config: Config) -> Result<(), String> {
    let addr = format!("127.0.0.1:{}", config.port);
    let server = Arc::new(
        Server::http(&addr).map_err(|err| format!("cannot listen on {addr}: {err}"))?,
    );
    println!("GMO TempTools web: http://{addr}");
    let _ = serve(server, config);
    Ok(())
}

/// Run the request loop on a background thread. Tests keep their own handle to
/// the server so they can unblock the loop when the test ends.
fn serve(server: Arc<Server>, config: Config) -> std::thread::JoinHandle<()> {
    thread::spawn(move || {
        for request in server.incoming_requests() {
            handle(request, &config);
        }
    })
}

fn handle(request: tiny_http::Request, config: &Config) {
    let method = request.method().clone();
    let raw_url = request.url().to_string();
    let path = raw_url.split('?').next().unwrap_or("/");

    if path == "/auth" {
        return handle_login(request, config);
    }

    if path.starts_with("/api/") {
        let bearer = local_auth::token_from_request(&request).map(str::to_string);
        let cookie = cookie_token(&request);
        let supplied = bearer.as_deref().or(cookie.as_deref());
        if !supplied
            .map(|value| local_auth::constant_time_eq(&config.auth_token, value))
            .unwrap_or(false)
        {
            return reply_json(
                request,
                401,
                serde_json::json!({ "error": "A per-install token is required" }),
            );
        }
        if !local_auth::origin_allowed(&request, &local_auth::origins(&[config.port]))
            || !local_auth::host_allowed(&request, &[config.port])
        {
            return reply_json(
                request,
                403,
                serde_json::json!({ "error": "Request origin is not allowed" }),
            );
        }
        if matches!(method, Method::Post | Method::Put | Method::Patch | Method::Delete)
            && bearer.is_none()
            && header(&request, CSRF_HEADER).as_deref() != Some("1")
        {
            return reply_json(
                request,
                403,
                serde_json::json!({ "error": "Missing CSRF header" }),
            );
        }
    }

    if method == Method::Get {
        match path {
            "/" | "/index.html" => {
                if is_authorized(&request, config) {
                    return reply(request, 200, INDEX.as_bytes(), "text/html; charset=utf-8");
                }
                return reply(request, 200, LOGIN.as_bytes(), "text/html; charset=utf-8");
            }
            "/app.css" => return reply(request, 200, CSS.as_bytes(), "text/css; charset=utf-8"),
            "/app.js" => {
                return reply(
                    request,
                    200,
                    JS.as_bytes(),
                    "text/javascript; charset=utf-8",
                )
            }
            "/ui.js" => {
                return reply(
                    request,
                    200,
                    UI_JS.as_bytes(),
                    "text/javascript; charset=utf-8",
                )
            }
            "/favicon.ico" => return reply(request, 204, &[], "image/x-icon"),
            _ => {}
        }
    }

    if path == "/api/settings" {
        return handle_settings(request, config, &method);
    }
    if path == "/api/notifications/test" {
        return handle_notification_test(request, &method);
    }
    if path == "/api/clear-data" {
        return handle_clear_data(request, config, &method);
    }

    if path == "/api/status" {
        return reply_json(
            request,
            200,
            serde_json::json!({
                "mode": "web",
                "mail_port": config.mail_port,
                "sms_port": config.sms_port,
                "otp": true,
            }),
        );
    }

    if path.starts_with("/api/mail/") {
        return proxy(
            request,
            config.mail_port,
            format!(
                "http://127.0.0.1:{}{}",
                config.mail_port,
                &raw_url["/api/mail".len()..]
            ),
            &config.auth_token,
        );
    }
    if path.starts_with("/api/sms/") {
        return proxy(
            request,
            config.sms_port,
            format!(
                "http://127.0.0.1:{}{}",
                config.sms_port,
                &raw_url["/api/sms".len()..]
            ),
            &config.auth_token,
        );
    }
    if path == "/api/otp" || path.starts_with("/api/otp/") {
        return handle_otp(request, config);
    }

    reply(request, 404, b"Not found", "text/plain; charset=utf-8");
}

fn app_paths(config: &Config) -> Paths {
    let state_root = config
        .otp_vault
        .parent()
        .and_then(|path| path.parent())
        .and_then(|path| path.parent())
        .map(PathBuf::from)
        .unwrap_or_else(|| local_auth::install_root());
    let settings = state_root.join("settings.json");
    Paths::new(local_auth::install_root(), state_root, settings)
}

fn settings_payload(paths: &Paths) -> serde_json::Value {
    let settings = Settings::load(&paths.settings);
    let rows: Vec<serde_json::Value> = paths
        .rows()
        .into_iter()
        .map(|(label, path)| serde_json::json!({ "label": label, "path": path.display().to_string() }))
        .collect();
    let notes: Vec<serde_json::Value> = TOOL_RELEASES
        .iter()
        .map(|note| {
            serde_json::json!({
                "tool": note.tool,
                "version": note.version,
                "summary": note.summary,
                "changelog": note.changelog.iter().map(|(version, change)| {
                    serde_json::json!({ "version": version, "change": change })
                }).collect::<Vec<_>>(),
            })
        })
        .collect();
    serde_json::json!({
        "appearance": settings.appearance.as_str(),
        "notifications": {
            "enabled": settings.notifications.enabled,
            "mail": settings.notifications.mail,
            "sms": settings.notifications.sms,
        },
        "owner": OWNER,
        "version": APP_VERSION,
        "paths": rows,
        "release_notes": notes,
    })
}

fn handle_settings(mut request: tiny_http::Request, config: &Config, method: &Method) {
    let paths = app_paths(config);
    match method {
        Method::Get => reply_json(request, 200, settings_payload(&paths)),
        Method::Post => {
            let bytes = body(&mut request);
            let parsed: serde_json::Value = match serde_json::from_slice(&bytes) {
                Ok(value) => value,
                Err(_) => {
                    return reply_json(
                        request,
                        422,
                        serde_json::json!({ "error": "Expected a JSON body" }),
                    )
                }
            };
            let mut settings = Settings::load(&paths.settings);
            let mut changed = false;
            if let Some(appearance) = parsed.get("appearance") {
                let Some(appearance) = appearance.as_str().and_then(Appearance::from_str) else {
                    return reply_json(
                        request,
                        422,
                        serde_json::json!({ "error": "appearance must be dark or light" }),
                    );
                };
                settings.appearance = appearance;
                changed = true;
            }
            if let Some(notifications) = parsed.get("notifications") {
                let Some(enabled) = notifications.get("enabled").and_then(|value| value.as_bool()) else {
                    return reply_json(
                        request,
                        422,
                        serde_json::json!({ "error": "notifications.enabled must be true or false" }),
                    );
                };
                let Some(mail) = notifications.get("mail").and_then(|value| value.as_bool()) else {
                    return reply_json(
                        request,
                        422,
                        serde_json::json!({ "error": "notifications.mail must be true or false" }),
                    );
                };
                let Some(sms) = notifications.get("sms").and_then(|value| value.as_bool()) else {
                    return reply_json(
                        request,
                        422,
                        serde_json::json!({ "error": "notifications.sms must be true or false" }),
                    );
                };
                settings.notifications.enabled = enabled;
                settings.notifications.mail = mail;
                settings.notifications.sms = sms;
                changed = true;
            }
            if !changed {
                return reply_json(
                    request,
                    422,
                    serde_json::json!({ "error": "No supported setting was supplied" }),
                );
            }
            match settings.save(&paths.settings) {
                Ok(()) => {
                    settings.apply_notifications();
                    reply_json(request, 200, settings_payload(&paths))
                }
                Err(error) => reply_json(request, 500, serde_json::json!({ "error": error })),
            }
        }
        _ => reply(request, 405, b"Method not allowed", "text/plain; charset=utf-8"),
    }
}

fn handle_notification_test(request: tiny_http::Request, method: &Method) {
    if method != &Method::Post {
        let _ = request.respond(
            Response::from_string("Method not allowed")
                .with_status_code(405)
                .with_header(Header::from_bytes(&b"Content-Type"[..], &b"text/plain; charset=utf-8"[..]).unwrap()),
        );
        return;
    }
    desktop_notifications::notify(
        "GMO TempTools · Test notification",
        "Desktop notifications are working.",
    );
    let _ = request.respond(
        Response::from_string(r#"{"ok":true}"#)
            .with_status_code(200)
            .with_header(Header::from_bytes(&b"Content-Type"[..], &b"application/json; charset=utf-8"[..]).unwrap())
            .with_header(Header::from_bytes(&b"Cache-Control"[..], &b"no-store"[..]).unwrap()),
    );
}

fn handle_clear_data(mut request: tiny_http::Request, config: &Config, method: &Method) {
    if method != &Method::Post {
        return reply(request, 405, b"Method not allowed", "text/plain; charset=utf-8");
    }
    let bytes = body(&mut request);
    let parsed: serde_json::Value = match serde_json::from_slice(&bytes) {
        Ok(value) => value,
        Err(_) => {
            return reply_json(
                request,
                422,
                serde_json::json!({ "error": "Expected a JSON body" }),
            )
        }
    };
    if parsed.get("confirm").and_then(|value| value.as_str()) != Some("CLEAR") {
        return reply_json(
            request,
            422,
            serde_json::json!({ "error": "Send {\"confirm\":\"CLEAR\"} to delete local data" }),
        );
    }
    let paths = app_paths(config);
    match paths.clear_all() {
        Ok(()) => reply_json(
            request,
            200,
            serde_json::json!({ "ok": true, "restart_required": true }),
        ),
        Err(error) => reply_json(request, 500, serde_json::json!({ "error": error })),
    }
}

fn cookie_token(request: &tiny_http::Request) -> Option<String> {
    let cookies = header(request, "cookie")?;
    cookies.split(';').find_map(|part| {
        let (name, value) = part.trim().split_once('=')?;
        (name == COOKIE_NAME).then(|| value.to_string())
    })
}

fn is_authorized(request: &tiny_http::Request, config: &Config) -> bool {
    local_auth::token_from_request(request)
        .map(str::to_string)
        .or_else(|| cookie_token(request))
        .map(|token| local_auth::constant_time_eq(&config.auth_token, &token))
        .unwrap_or(false)
}

fn handle_login(mut request: tiny_http::Request, config: &Config) {
    if request.method() != &Method::Post {
        return reply(request, 200, LOGIN.as_bytes(), "text/html; charset=utf-8");
    }
    if !local_auth::origin_allowed(&request, &local_auth::origins(&[config.port]))
        || !local_auth::host_allowed(&request, &[config.port])
    {
        return reply(request, 403, b"Request origin is not allowed", "text/plain; charset=utf-8");
    }
    let bytes = body(&mut request);
    let form = String::from_utf8_lossy(&bytes);
    let supplied = form.split('&').find_map(|pair| {
        let (name, value) = pair.split_once('=')?;
        (name == "token").then(|| value.replace('+', " "))
    });
    if !supplied
        .as_deref()
        .map(|value| local_auth::constant_time_eq(&config.auth_token, value.trim()))
        .unwrap_or(false)
    {
        return reply(
            request,
            401,
            b"Invalid install token. Reopen the login page and try again.",
            "text/plain; charset=utf-8",
        );
    }

    let mut response = Response::from_data(Vec::new()).with_status_code(303);
    if let Ok(value) = Header::from_bytes(&b"Location"[..], &b"/"[..]) {
        response.add_header(value);
    }
    let cookie = format!("{COOKIE_NAME}={}; HttpOnly; SameSite=Strict; Path=/", config.auth_token);
    if let Ok(value) = Header::from_bytes(&b"Set-Cookie"[..], cookie.as_bytes()) {
        response.add_header(value);
    }
    if let Ok(value) = Header::from_bytes(&b"Cache-Control"[..], &b"no-store"[..]) {
        response.add_header(value);
    }
    let _ = request.respond(response);
}

fn reply(request: tiny_http::Request, status: u16, body: &[u8], content_type: &str) {
    let mut response = Response::from_data(body.to_vec()).with_status_code(status);
    if let Ok(header) = Header::from_bytes(&b"Content-Type"[..], content_type.as_bytes()) {
        response.add_header(header);
    }
    if let Ok(header) = Header::from_bytes(&b"Cache-Control"[..], &b"no-store"[..]) {
        response.add_header(header);
    }
    if let Ok(header) = Header::from_bytes(&b"X-Content-Type-Options"[..], &b"nosniff"[..]) {
        response.add_header(header);
    }
    let _ = request.respond(response);
}

fn reply_json(request: tiny_http::Request, status: u16, value: serde_json::Value) {
    reply(
        request,
        status,
        value.to_string().as_bytes(),
        "application/json; charset=utf-8",
    );
}

fn body(request: &mut tiny_http::Request) -> Vec<u8> {
    let mut bytes = Vec::new();
    let _ = request.as_reader().read_to_end(&mut bytes);
    bytes
}

fn header(request: &tiny_http::Request, name: &str) -> Option<String> {
    request
        .headers()
        .iter()
        .find(|header| header.field.as_str().as_str().eq_ignore_ascii_case(name))
        .map(|header| header.value.as_str().to_string())
}

fn proxy(mut request: tiny_http::Request, _port: u16, url: String, auth_token: &str) {
    let method = request.method().clone();
    let bytes = body(&mut request);
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(std::time::Duration::from_secs(5))
        .timeout(std::time::Duration::from_secs(30))
        .build();

    let mut upstream = match method {
        Method::Get => agent.get(&url),
        Method::Post => agent.post(&url),
        Method::Delete => agent.delete(&url),
        Method::Put => agent.put(&url),
        Method::Patch => agent.patch(&url),
        _ => {
            return reply(request, 405, b"Method not allowed", "text/plain; charset=utf-8")
        }
    };
    if let Some(content_type) = header(&request, "content-type") {
        upstream = upstream.set("Content-Type", &content_type);
    }
    upstream = upstream
        .set("Authorization", &format!("Bearer {auth_token}"))
        .set("X-TempTools-Token", auth_token);

    let result = if bytes.is_empty() {
        upstream.call()
    } else {
        upstream.send_bytes(&bytes)
    };

    let response = match result {
        Ok(response) => response,
        Err(ureq::Error::Status(_, response)) => response,
        Err(err) => {
            return reply_json(
                request,
                502,
                serde_json::json!({ "error": format!("local service is unreachable: {err}") }),
            )
        }
    };

    let status = response.status();
    let content_type = response
        .header("content-type")
        .unwrap_or("application/json; charset=utf-8")
        .to_string();
    let csp = response
        .header("content-security-policy")
        .map(str::to_string);
    let mut payload = Vec::new();
    let _ = response.into_reader().read_to_end(&mut payload);

    let mut outgoing = Response::from_data(payload).with_status_code(status);
    if let Ok(value) = Header::from_bytes(&b"Content-Type"[..], content_type.as_bytes()) {
        outgoing.add_header(value);
    }
    if let Some(csp) = csp {
        if let Ok(value) = Header::from_bytes(&b"Content-Security-Policy"[..], csp.as_bytes()) {
            outgoing.add_header(value);
        }
    }
    if let Ok(value) = Header::from_bytes(&b"Cache-Control"[..], &b"no-store"[..]) {
        outgoing.add_header(value);
    }
    if let Ok(value) = Header::from_bytes(&b"X-Content-Type-Options"[..], &b"nosniff"[..]) {
        outgoing.add_header(value);
    }
    let _ = request.respond(outgoing);
}

fn now_secs() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs_f64())
        .unwrap_or(0.0)
}

fn otp_payload(config: &Config) -> Result<serde_json::Value, String> {
    let now = now_secs();
    let secrets = vault::load_result(&config.otp_vault)?;
    let rows: Vec<serde_json::Value> = secrets
        .iter()
        .enumerate()
        .map(|(index, secret)| {
            serde_json::json!({
                "index": index,
                "label": secret.label,
                "masked": secret.masked(),
                "code": secret.code(now),
                "digits": secret.digits,
                "period": secret.period,
                "algorithm": secret.algorithm,
                "remaining": temp_otp::seconds_left(secret.period, now),
            })
        })
        .collect();
    Ok(serde_json::json!({ "secrets": rows }))
}

fn handle_otp(mut request: tiny_http::Request, config: &Config) {
    let method = request.method().clone();
    let path = request.url().split('?').next().unwrap_or("/").to_string();
    let tail = &path["/api/otp".len()..];

    match (&method, tail) {
        (Method::Get, "") => match otp_payload(config) {
            Ok(payload) => reply_json(request, 200, payload),
            Err(error) => reply_json(
                request,
                500,
                serde_json::json!({ "error": error }),
            ),
        },
        (Method::Post, "/extract") => {
            let bytes = body(&mut request);
            let parsed: serde_json::Value = match serde_json::from_slice(&bytes) {
                Ok(value) => value,
                Err(_) => {
                    return reply_json(
                        request,
                        422,
                        serde_json::json!({ "error": "Expected a JSON body with text" }),
                    )
                }
            };
            let text = parsed.get("text").and_then(|value| value.as_str()).unwrap_or("");
            let found = temp_otp::extract(text);
            reply_json(
                request,
                200,
                serde_json::json!({ "codes": found.codes, "links": found.links }),
            )
        }
        (Method::Post, "") => {
            let bytes = body(&mut request);
            let parsed: serde_json::Value = match serde_json::from_slice(&bytes) {
                Ok(value) => value,
                Err(_) => {
                    return reply_json(
                        request,
                        422,
                        serde_json::json!({ "error": "Expected a JSON body" }),
                    )
                }
            };
            let input = parsed.get("input").and_then(|value| value.as_str()).unwrap_or("");
            let label = parsed.get("label").and_then(|value| value.as_str()).unwrap_or("");
            let Some(secret) = vault::StoredSecret::from_input(input, label) else {
                return reply_json(
                    request,
                    422,
                    serde_json::json!({
                        "error": "That is not an otpauth:// URI or a base32 secret (8+ characters)."
                    }),
                );
            };
            match vault::update(&config.otp_vault, |secrets| {
                let exists = secrets
                    .iter()
                    .any(|saved| saved.secret == secret.secret && saved.label == secret.label);
                if !exists {
                    secrets.push(secret);
                }
                !exists
            }) {
                Ok(added) => reply_json(
                    request,
                    200,
                    serde_json::json!({ "ok": true, "added": added }),
                ),
                Err(error) => reply_json(
                    request,
                    500,
                    serde_json::json!({ "error": error }),
                ),
            }
        }
        (Method::Delete, suffix) => {
            let Some(index) = suffix.trim_start_matches('/').parse::<usize>().ok() else {
                return reply_json(
                    request,
                    400,
                    serde_json::json!({ "error": "Missing secret index" }),
                );
            };
            match vault::update(&config.otp_vault, |secrets| {
                if index >= secrets.len() {
                    return false;
                }
                secrets.remove(index);
                true
            }) {
                Ok(true) => reply_json(request, 200, serde_json::json!({ "ok": true })),
                Ok(false) => reply_json(
                    request,
                    404,
                    serde_json::json!({ "error": "Secret not found" }),
                ),
                Err(error) => reply_json(
                    request,
                    500,
                    serde_json::json!({ "error": error }),
                ),
            }
        }
        _ => reply(request, 404, b"Not found", "text/plain; charset=utf-8"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    use std::fs;
    use std::sync::Mutex;
    use std::thread::JoinHandle;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    struct TempVault {
        path: PathBuf,
    }

    impl TempVault {
        fn new(name: &str) -> Self {
            let stamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|duration| duration.as_nanos())
                .unwrap_or(0);
            let path = std::env::temp_dir().join(format!(
                "gmo-temptools-web-{name}-{}-{stamp}.json",
                std::process::id()
            ));
            let _ = fs::remove_file(&path);
            Self { path }
        }

        fn path(&self) -> PathBuf {
            self.path.clone()
        }
    }

    impl Drop for TempVault {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.path);
            let _ = fs::remove_file(self.path.with_extension("json.tmp"));
        }
    }

    struct TestWeb {
        port: u16,
        server: Arc<Server>,
        thread: Option<JoinHandle<()>>,
    }

    impl TestWeb {
        fn start(mut config: Config) -> Self {
            let server = Arc::new(
                Server::http("127.0.0.1:0").expect("test web server should bind an ephemeral port"),
            );
            let port = server
                .server_addr()
                .to_ip()
                .expect("test web server should have an IP address")
                .port();
            config.port = port;
            let thread = serve(server.clone(), config);
            Self {
                port,
                server,
                thread: Some(thread),
            }
        }
    }

    impl Drop for TestWeb {
        fn drop(&mut self) {
            self.server.unblock();
            if let Some(thread) = self.thread.take() {
                let _ = thread.join();
            }
        }
    }

    #[derive(Clone, Debug)]
    struct Hit {
        method: String,
        url: String,
        body: String,
        authorization: String,
    }

    struct FakeUpstream {
        port: u16,
        server: Arc<Server>,
        thread: Option<JoinHandle<()>>,
        hits: Arc<Mutex<Vec<Hit>>>,
    }

    impl FakeUpstream {
        fn start(service: &str) -> Self {
            let server = Arc::new(
                Server::http("127.0.0.1:0").expect("fake upstream should bind an ephemeral port"),
            );
            let port = server
                .server_addr()
                .to_ip()
                .expect("fake upstream should have an IP address")
                .port();
            let hits = Arc::new(Mutex::new(Vec::new()));
            let thread_server = server.clone();
            let thread_hits = hits.clone();
            let label = service.to_string();
            let thread = thread::spawn(move || {
                for mut request in thread_server.incoming_requests() {
                    let method = request.method().as_str().to_string();
                    let url = request.url().to_string();
                    let mut body = String::new();
                    let _ = request.as_reader().read_to_string(&mut body);
                    let authorization = request
                        .headers()
                        .iter()
                        .find(|header| {
                            header
                                .field
                                .as_str()
                                .as_str()
                                .eq_ignore_ascii_case("authorization")
                        })
                        .map(|header| header.value.as_str().to_string())
                        .unwrap_or_default();
                    thread_hits.lock().unwrap().push(Hit {
                        method: method.clone(),
                        url: url.clone(),
                        body: body.clone(),
                        authorization,
                    });
                    let payload = serde_json::json!({
                        "service": label.clone(),
                        "method": method,
                        "url": url,
                        "body": body,
                    })
                    .to_string();
                    let response = Response::from_string(payload)
                        .with_status_code(200)
                        .with_header(
                            Header::from_bytes(
                                &b"Content-Type"[..],
                                &b"application/json"[..],
                            )
                            .expect("test content-type header"),
                        );
                    let _ = request.respond(response);
                }
            });
            Self {
                port,
                server,
                thread: Some(thread),
                hits,
            }
        }

        fn hits(&self) -> Vec<Hit> {
            self.hits.lock().unwrap().clone()
        }
    }

    impl Drop for FakeUpstream {
        fn drop(&mut self) {
            self.server.unblock();
            if let Some(thread) = self.thread.take() {
                let _ = thread.join();
            }
        }
    }

    fn request(method: &str, port: u16, path: &str, body: Option<&str>) -> (u16, Value) {
        request_with(method, port, path, body, Some("test-token"), None)
    }

    fn request_with(
        method: &str,
        port: u16,
        path: &str,
        body: Option<&str>,
        token: Option<&str>,
        origin: Option<&str>,
    ) -> (u16, Value) {
        let agent = ureq::AgentBuilder::new()
            .timeout_connect(Duration::from_secs(2))
            .timeout(Duration::from_secs(5))
            .build();
        let url = format!("http://127.0.0.1:{port}{path}");
        let auth = token.map(|token| format!("Bearer {token}"));
        let origin = origin
            .map(str::to_string)
            .or_else(|| Some(format!("http://127.0.0.1:{port}")));
        let result = match method {
            "GET" => {
                let mut outgoing = agent.get(&url);
                if let Some(auth) = &auth {
                    outgoing = outgoing.set("Authorization", auth);
                }
                if let Some(origin) = &origin {
                    outgoing = outgoing.set("Origin", origin);
                }
                outgoing.call()
            }
            "POST" => {
                let mut outgoing = agent
                    .post(&url)
                    .set("Content-Type", "application/json");
                if let Some(auth) = &auth {
                    outgoing = outgoing.set("Authorization", auth);
                }
                if let Some(origin) = &origin {
                    outgoing = outgoing.set("Origin", origin);
                }
                if token.is_some() {
                    outgoing = outgoing.set("X-TempTools-CSRF", "1");
                }
                match body {
                    Some(body) => outgoing.send_string(body),
                    None => outgoing.call(),
                }
            }
            "DELETE" => {
                let mut outgoing = agent.delete(&url);
                if let Some(auth) = &auth {
                    outgoing = outgoing.set("Authorization", auth);
                }
                if let Some(origin) = &origin {
                    outgoing = outgoing.set("Origin", origin);
                }
                if token.is_some() {
                    outgoing = outgoing.set("X-TempTools-CSRF", "1");
                }
                outgoing.call()
            }
            other => panic!("unsupported test method {other}"),
        };
        let response = match result {
            Ok(response) => response,
            Err(ureq::Error::Status(_, response)) => response,
            Err(error) => panic!("{method} {path} did not reach the web server: {error}"),
        };
        let status = response.status();
        let text = response.into_string().unwrap_or_default();
        let value = serde_json::from_str(&text).unwrap_or(Value::String(text));
        (status, value)
    }

    fn raw_get(port: u16, path: &str, headers: &[(&str, &str)]) -> (u16, String) {
        use std::io::{Read as _, Write as _};
        use std::net::TcpStream;

        let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect to test web");
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .expect("set read timeout");
        let mut outgoing = format!("GET {path} HTTP/1.1\r\n");
        if !headers
            .iter()
            .any(|(name, _)| name.eq_ignore_ascii_case("host"))
        {
            outgoing.push_str(&format!("Host: 127.0.0.1:{port}\r\n"));
        }
        for (name, value) in headers {
            outgoing.push_str(name);
            outgoing.push_str(": ");
            outgoing.push_str(value);
            outgoing.push_str("\r\n");
        }
        outgoing.push_str("Connection: close\r\n\r\n");
        stream
            .write_all(outgoing.as_bytes())
            .expect("write raw test request");
        let mut response = String::new();
        stream
            .read_to_string(&mut response)
            .expect("read raw test response");
        let status = response
            .lines()
            .next()
            .and_then(|line| line.split_whitespace().nth(1))
            .and_then(|value| value.parse().ok())
            .unwrap_or(0);
        let body = response
            .split_once("\r\n\r\n")
            .map(|(_, body)| body.to_string())
            .unwrap_or_default();
        (status, body)
    }

    #[test]
    fn unauthenticated_inbound_path_cannot_proxy_through_the_web_app() {
        let sms = FakeUpstream::start("sms");
        let vault = TempVault::new("inbound-bypass");
        let web = TestWeb::start(Config {
            port: 0,
            auth_token: "test-token".to_string(),
            mail_port: 0,
            sms_port: sms.port,
            otp_vault: vault.path(),
        });

        // A raw client can preserve `..` even though ureq would normalize it.
        let (status, _) = raw_get(
            web.port,
            "/api/sms/api/inbound/../../api/status",
            &[],
        );
        assert_eq!(status, 401, "the web app must not exempt inbound-shaped proxy paths");
        assert!(sms.hits().is_empty(), "the upstream must not receive the unauthenticated request");
    }

    #[test]
    fn web_app_rejects_a_foreign_host_even_with_a_token() {
        let vault = TempVault::new("host");
        let web = TestWeb::start(Config {
            port: 0,
            auth_token: "test-token".to_string(),
            mail_port: 0,
            sms_port: 0,
            otp_vault: vault.path(),
        });

        let (status, _) = raw_get(
            web.port,
            "/api/status",
            &[
                ("Authorization", "Bearer test-token"),
                ("Host", "attacker.example"),
            ],
        );
        assert_eq!(status, 403);
    }

    #[test]
    fn proxy_routes_services_and_forwards_the_request() {
        let mail = FakeUpstream::start("mail");
        let sms = FakeUpstream::start("sms");
        let vault = TempVault::new("routes");
        let web = TestWeb::start(Config {
            port: 0,
            auth_token: "test-token".to_string(),
            mail_port: mail.port,
            sms_port: sms.port,
            otp_vault: vault.path(),
        });

        let (status, body) = request("GET", web.port, "/api/mail/api/status", None);
        assert_eq!(status, 200);
        assert_eq!(body["service"], "mail");
        let hits = mail.hits();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].method, "GET");
        assert_eq!(hits[0].url, "/api/status");
        assert_eq!(hits[0].authorization, "Bearer test-token");

        let (status, body) = request(
            "POST",
            web.port,
            "/api/sms/api/numbers",
            Some(r#"{"country":"US"}"#),
        );
        assert_eq!(status, 200);
        assert_eq!(body["service"], "sms");
        assert_eq!(body["method"], "POST");
        assert_eq!(body["body"], r#"{"country":"US"}"#);
        let hits = sms.hits();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].url, "/api/numbers");
        assert_eq!(hits[0].body, r#"{"country":"US"}"#);
    }

    #[test]
    fn otp_list_create_extract_and_delete() {
        let vault = TempVault::new("otp-crud");
        let web = TestWeb::start(Config {
            port: 0,
            auth_token: "test-token".to_string(),
            mail_port: 0,
            sms_port: 0,
            otp_vault: vault.path(),
        });

        let (status, body) = request("GET", web.port, "/api/otp", None);
        assert_eq!(status, 200);
        assert_eq!(body["secrets"].as_array().expect("secrets array").len(), 0);

        let (status, body) = request(
            "POST",
            web.port,
            "/api/otp",
            Some(r#"{"input":"JBSWY3DPEHPK3PXP","label":"Test account"}"#),
        );
        assert_eq!(status, 200);
        assert_eq!(body["added"], true);

        let (status, body) = request("GET", web.port, "/api/otp", None);
        assert_eq!(status, 200);
        let secrets = body["secrets"].as_array().expect("secrets array");
        assert_eq!(secrets.len(), 1);
        assert_eq!(secrets[0]["label"], "Test account");
        assert_eq!(secrets[0]["algorithm"], "SHA1");
        assert_eq!(secrets[0]["digits"], 6);
        assert_eq!(secrets[0]["code"].as_str().expect("code").len(), 6);

        let (status, found) = request(
            "POST",
            web.port,
            "/api/otp/extract",
            Some(r#"{"text":"Your verification code is 481920"}"#),
        );
        assert_eq!(status, 200);
        assert!(
            found["codes"]
                .as_array()
                .expect("codes array")
                .iter()
                .any(|code| code == "481920"),
            "extractor should return the verification code: {found}"
        );

        let (status, body) = request("DELETE", web.port, "/api/otp/0", None);
        assert_eq!(status, 200);
        assert_eq!(body["ok"], true);
        let (_, body) = request("GET", web.port, "/api/otp", None);
        assert_eq!(body["secrets"].as_array().expect("secrets array").len(), 0);
    }

    #[test]
    fn desktop_tab_and_web_share_the_vault() {
        let vault = TempVault::new("shared");
        let mut desktop = temp_otp::gui::OtpApp::new_with_vault(vault.path());
        assert_eq!(desktop.secret_count(), 0);

        let web = TestWeb::start(Config {
            port: 0,
            auth_token: "test-token".to_string(),
            mail_port: 0,
            sms_port: 0,
            otp_vault: vault.path(),
        });

        let (status, _) = request(
            "POST",
            web.port,
            "/api/otp",
            Some(r#"{"input":"JBSWY3DPEHPK3PXP","label":"Web account"}"#),
        );
        assert_eq!(status, 200);
        assert_eq!(desktop.secret_count(), 0, "desktop caches its own list");
        assert!(desktop.sync_from_disk());
        assert_eq!(desktop.secret_count(), 1);
        assert!(!desktop.sync_from_disk(), "a second sync is a no-op");

        assert!(desktop.add_secret_from_input("GEZDGNBVGY3TQOJQ", "Desktop account"));
        let (status, body) = request("GET", web.port, "/api/otp", None);
        assert_eq!(status, 200);
        let labels: Vec<&str> = body["secrets"]
            .as_array()
            .expect("secrets array")
            .iter()
            .filter_map(|secret| secret["label"].as_str())
            .collect();
        assert_eq!(labels, vec!["Web account", "Desktop account"]);

        let (status, _) = request("DELETE", web.port, "/api/otp/0", None);
        assert_eq!(status, 200);
        assert!(desktop.sync_from_disk());
        assert_eq!(desktop.secret_count(), 1);
    }

    #[test]
    fn web_app_requires_token_origin_and_csrf() {
        let vault = TempVault::new("auth");
        let web = TestWeb::start(Config {
            port: 0,
            auth_token: "test-token".to_string(),
            mail_port: 0,
            sms_port: 0,
            otp_vault: vault.path(),
        });

        let (status, body) = request_with("GET", web.port, "/", None, None, None);
        assert_eq!(status, 200);
        assert!(body
            .as_str()
            .unwrap_or("")
            .contains("Unlock the local app"));

        let (status, _) = request_with("GET", web.port, "/api/otp", None, None, None);
        assert_eq!(status, 401);

        let (status, _) = request_with(
            "GET",
            web.port,
            "/api/otp",
            None,
            Some("test-token"),
            Some("http://evil.example"),
        );
        assert_eq!(status, 403);

        let agent = ureq::AgentBuilder::new()
            .timeout_connect(Duration::from_secs(2))
            .timeout(Duration::from_secs(5))
            .build();
        let url = format!("http://127.0.0.1:{}/api/otp", web.port);
        let origin = format!("http://127.0.0.1:{}", web.port);
        let response = agent
            .post(&url)
            .set("Cookie", "temptools_token=test-token")
            .set("Origin", &origin)
            .set("Content-Type", "application/json")
            .send_string(r#"{"input":"JBSWY3DPEHPK3PXP","label":"CSRF account"}"#);
        assert!(matches!(response, Err(ureq::Error::Status(403, _))));

        let response = agent
            .post(&url)
            .set("Cookie", "temptools_token=test-token")
            .set("Origin", &origin)
            .set("X-TempTools-CSRF", "1")
            .set("Content-Type", "application/json")
            .send_string(r#"{"input":"JBSWY3DPEHPK3PXP","label":"CSRF account"}"#);
        assert_eq!(response.expect("CSRF-authenticated request").status(), 200);

        let (status, body) = request("GET", web.port, "/api/otp", None);
        assert_eq!(status, 200);
        assert_eq!(body["secrets"].as_array().expect("secrets array").len(), 1);
    }

    #[test]
    fn unreachable_upstream_returns_a_502() {
        let vault = TempVault::new("unreachable");
        let web = TestWeb::start(Config {
            port: 0,
            auth_token: "test-token".to_string(),
            // Port 0 is never a listening HTTP service.
            mail_port: 0,
            sms_port: 0,
            otp_vault: vault.path(),
        });

        let (status, body) = request("GET", web.port, "/api/mail/api/status", None);
        assert_eq!(status, 502);
        let error = body["error"].as_str().expect("error message");
        assert!(
            error.contains("local service is unreachable"),
            "unexpected proxy error: {error}"
        );
    }
}
