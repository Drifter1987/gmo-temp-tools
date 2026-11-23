//! Small request guard shared by the loopback services. The token is never
//! inferred from a cookie here: a browser calling a service directly must send
//! the explicit bearer header, so a cross-site request cannot ride an ambient
//! credential. Inbound carrier routes are left to their signature verifiers.

use fs2::FileExt;
use rand::RngCore;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use tiny_http::Request;

pub const TOKEN_HEADER: &str = "X-TempTools-Token";

/// The fixed loopback origin of the unified browser app. Service ports use
/// this for page redirects; it never comes from a request Host header.
pub fn web_app_origin() -> String {
    let port = std::env::var("TEMPTOOLS_WEB_PORT")
        .ok()
        .and_then(|value| value.trim().parse::<u16>().ok())
        .filter(|value| *value != 0)
        .unwrap_or(8780);
    format!("http://127.0.0.1:{port}/")
}

/// Send a browser that reached a service port to the authenticated web app.
/// This is a page-only redirect: callers must leave `/api/...` requests to the
/// token guard, and no local data is returned here.
pub fn redirect_to_web(request: tiny_http::Request) {
    let mut response = tiny_http::Response::from_string("").with_status_code(302);
    if let Ok(header) = tiny_http::Header::from_bytes(&b"Location"[..], web_app_origin().as_bytes()) {
        response.add_header(header);
    }
    if let Ok(header) = tiny_http::Header::from_bytes(&b"Cache-Control"[..], &b"no-store"[..]) {
        response.add_header(header);
    }
    let _ = request.respond(response);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GuardFailure {
    Unauthorized,
    Forbidden,
}

impl GuardFailure {
    pub fn status(self) -> u16 {
        match self {
            Self::Unauthorized => 401,
            Self::Forbidden => 403,
        }
    }

    pub fn message(self) -> &'static str {
        match self {
            Self::Unauthorized => "A per-install token is required",
            Self::Forbidden => "Request origin is not allowed",
        }
    }
}

#[derive(Clone)]
pub struct ApiGuard {
    token: String,
    origins: Vec<String>,
}

impl ApiGuard {
    /// Every shipped entry point supplies a non-empty per-install token.
    pub fn new(token: String, origins: Vec<String>) -> Self {
        Self { token, origins }
    }

    pub fn authorize(&self, request: &Request, path: &str) -> Result<(), GuardFailure> {
        if is_inbound(path) {
            return Ok(());
        }
        let expected = self.token.as_str();
        match token_from_request(request) {
            Some(actual) if constant_time_eq(expected, actual) => {}
            _ => return Err(GuardFailure::Unauthorized),
        }
        if !origin_allowed(request, &self.origins) {
            return Err(GuardFailure::Forbidden);
        }
        Ok(())
    }
}

/// The per-install secret lives beside the app's `.run` directory.
pub fn token_file(root: &Path) -> PathBuf {
    root.join(".run").join("auth-token")
}

fn lock_token_file(root: &Path) -> Result<std::fs::File, String> {
    let dir = root.join(".run");
    fs::create_dir_all(&dir)
        .map_err(|error| format!("cannot create the token directory: {error}"))?;
    let path = dir.join("auth-token.lock");
    let file = fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .open(&path)
        .map_err(|error| format!("cannot open the token lock: {error}"))?;
    file.lock_exclusive()
        .map_err(|error| format!("cannot lock the install token: {error}"))?;
    Ok(file)
}

/// Read the install token, or create a new random one when the file is absent
/// or empty. `TEMPTOOLS_AUTH_TOKEN` wins when explicitly exported.
pub fn load_or_create_token(root: &Path) -> Result<String, String> {
    if let Ok(value) = std::env::var("TEMPTOOLS_AUTH_TOKEN") {
        let value = value.trim().to_string();
        if !value.is_empty() {
            return Ok(value);
        }
    }

    load_or_create_token_file(root)
}

fn load_or_create_token_file(root: &Path) -> Result<String, String> {
    // Serialize first-run creation across processes. Without this, two
    // launches can both decide the file is missing and one can replace the
    // other's token.
    let _token_lock = lock_token_file(root)?;
    let path = token_file(root);
    if let Some(value) = read_token_file(&path)? {
        return Ok(value);
    }

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("cannot create the token directory: {error}"))?;
    }
    let mut bytes = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    let token: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    match create_token_file(&path, &token) {
        Ok(()) => Ok(token),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            // Another entry point won the first-run race. Reuse its token
            // instead of overwriting it with this process's token.
            if let Some(value) = read_token_file(&path)? {
                return Ok(value);
            }
            // An empty file is replaced so every entry point converges on a
            // usable token.
            fs::remove_file(&path)
                .map_err(|error| format!("cannot replace the empty install token: {error}"))?;
            create_token_file(&path, &token)
                .map_err(|error| format!("cannot write the install token: {error}"))?;
            Ok(token)
        }
        Err(error) => Err(format!("cannot write the install token: {error}")),
    }
}

fn read_token_file(path: &Path) -> Result<Option<String>, String> {
    match fs::read_to_string(path) {
        Ok(value) => {
            let value = value.trim().to_string();
            Ok((!value.is_empty()).then_some(value))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("cannot read the install token: {error}")),
    }
}

fn create_token_file(path: &Path, token: &str) -> std::io::Result<()> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    writeln!(file, "{token}")?;
    file.sync_all()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

/// The app root is the folder beside the executable. The installer puts the
/// executable in the user's local app folder, so state never depends on the
/// directory a shortcut happened to use.
pub fn install_root() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(PathBuf::from))
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
}

#[cfg(test)]
mod token_tests {
    use super::*;

    fn temp_root(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "gmo-local-auth-{name}-{}",
            std::process::id()
        ))
    }

    #[test]
    fn install_token_is_created_once_and_reused() {
        let root = temp_root("token");
        let _ = fs::remove_dir_all(&root);
        let first = load_or_create_token_file(&root).expect("token");
        assert_eq!(first.len(), 64);
        assert!(first.chars().all(|ch| ch.is_ascii_hexdigit()));
        let second = load_or_create_token_file(&root).expect("reloaded token");
        assert_eq!(first, second);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn empty_token_file_is_replaced() {
        let root = temp_root("empty");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join(".run")).expect("token directory");
        fs::write(token_file(&root), "\n").expect("empty token");
        let token = load_or_create_token_file(&root).expect("replacement token");
        assert_eq!(token.len(), 64);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn unreadable_token_file_fails_closed() {
        let root = temp_root("unreadable");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join(".run")).expect("token directory");
        let path = token_file(&root);
        let invalid = [0xff, 0xfe, 0x00];
        fs::write(&path, invalid).expect("invalid token file");
        let error = load_or_create_token_file(&root).expect_err("invalid token must fail");
        assert!(error.contains("cannot read the install token"));
        assert_eq!(fs::read(&path).expect("token file still exists"), invalid);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn concurrent_first_run_converges_on_one_token() {
        let root = std::sync::Arc::new(temp_root("concurrent"));
        let _ = fs::remove_dir_all(root.as_ref());
        let mut workers = Vec::new();
        for _ in 0..8 {
            let root = root.clone();
            workers.push(std::thread::spawn(move || {
                load_or_create_token_file(&root).expect("load token")
            }));
        }
        let tokens: Vec<String> = workers
            .into_iter()
            .map(|worker| worker.join().expect("token worker"))
            .collect();
        assert!(tokens.iter().all(|token| token == &tokens[0]));
        assert_eq!(tokens[0].len(), 64);
        let _ = fs::remove_dir_all(root.as_ref());
    }
}

pub fn is_inbound(path: &str) -> bool {
    path == "/api/inbound" || path.starts_with("/api/inbound/")
}

/// Static assets carry no user data. Everything else (including the
/// server-rendered service pages) is gated by the mandatory install token.
/// The loopback origins a local UI may use. Both spellings are accepted so a
/// user who opens localhost is not treated as a cross-site caller.
pub fn origins(ports: &[u16]) -> Vec<String> {
    let mut values = Vec::new();
    for port in ports {
        values.push(format!("http://127.0.0.1:{port}"));
        values.push(format!("http://localhost:{port}"));
    }
    values
}

pub fn header<'a>(request: &'a Request, name: &str) -> Option<&'a str> {
    request
        .headers()
        .iter()
        .find(|header| header.field.as_str().as_str().eq_ignore_ascii_case(name))
        .map(|header| header.value.as_str())
}

pub fn token_from_request(request: &Request) -> Option<&str> {
    if let Some(authorization) = header(request, "authorization") {
        if let Some((scheme, value)) = authorization.split_once(' ') {
            if scheme.eq_ignore_ascii_case("bearer") && !value.trim().is_empty() {
                return Some(value.trim());
            }
        }
    }
    header(request, TOKEN_HEADER).filter(|value| !value.trim().is_empty())
}

pub fn origin_allowed(request: &Request, allowed: &[String]) -> bool {
    match header(request, "origin") {
        None => true,
        Some(origin) => allowed.iter().any(|candidate| candidate == origin),
    }
}

pub fn host_allowed(request: &Request, ports: &[u16]) -> bool {
    let Some(host) = header(request, "host") else {
        return false;
    };
    ports.iter().any(|port| {
        host.eq_ignore_ascii_case(&format!("127.0.0.1:{port}"))
            || host.eq_ignore_ascii_case(&format!("localhost:{port}"))
    })
}

pub fn constant_time_eq(left: &str, right: &str) -> bool {
    let left = left.as_bytes();
    let right = right.as_bytes();
    let mut difference = left.len() ^ right.len();
    let length = left.len().max(right.len());
    for index in 0..length {
        let a = left.get(index).copied().unwrap_or(0);
        let b = right.get(index).copied().unwrap_or(0);
        difference |= usize::from(a ^ b);
    }
    difference == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_comparison_checks_every_byte() {
        assert!(constant_time_eq("same", "same"));
        assert!(!constant_time_eq("same", "samf"));
        assert!(!constant_time_eq("short", "longer"));
    }

    #[test]
    fn inbound_routes_are_the_only_public_api_paths() {
        assert!(is_inbound("/api/inbound/sms"));
        assert!(!is_inbound("/api/numbers"));
    }
}
