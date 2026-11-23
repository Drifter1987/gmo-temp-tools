//! Signature checking for carrier webhooks used by the SMS service.
//!
//! The service publishes a webhook URL and must refuse deliveries that the
//! carrier did not sign. What stays in the app is the payload shape and the
//! fields that identify the carrier.
//!
//! - **Twilio** signs with HMAC-SHA1 over the full public URL plus the sorted
//!   form fields, base64 encoded.
//! - **Telnyx** signs `"{timestamp}|{raw body}"` with Ed25519 and sends the
//!   timestamp, which must be recent.
//! - **Generic** carriers sign nothing, so a shared secret is compared in
//!   constant time.

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use ed25519_dalek::{Signature, VerifyingKey};
use hmac::{Hmac, Mac};
use sha1::Sha1;
use std::time::{SystemTime, UNIX_EPOCH};

/// Telnyx rejects deliveries older than five minutes; so do we.
pub const MAX_SKEW_SECS: i64 = 300;

// ---------------------------------------------------------------- form bodies

pub fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(value) = u8::from_str_radix(&input[i + 1..i + 3], 16) {
                out.push(value);
                i += 3;
                continue;
            }
        }
        out.push(if bytes[i] == b'+' { b' ' } else { bytes[i] });
        i += 1;
    }
    String::from_utf8_lossy(&out).to_string()
}

/// Every field of an `application/x-www-form-urlencoded` body, in order.
pub fn form_pairs(body: &str) -> Vec<(String, String)> {
    body.split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| match pair.split_once('=') {
            Some((name, value)) => (percent_decode(name), percent_decode(value)),
            None => (percent_decode(pair), String::new()),
        })
        .collect()
}

pub fn form_value<'a>(pairs: &'a [(String, String)], key: &str) -> Option<&'a str> {
    pairs
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(key))
        .map(|(_, value)| value.as_str())
}

/// Constant-time comparison, so a wrong secret cannot be guessed byte by byte.
pub fn ct_eq(a: &str, b: &str) -> bool {
    let ab = a.as_bytes();
    let bb = b.as_bytes();
    let mut diff = ab.len() ^ bb.len();
    for i in 0..ab.len().max(bb.len()) {
        diff |= usize::from(*ab.get(i).unwrap_or(&0) ^ *bb.get(i).unwrap_or(&0));
    }
    diff == 0
}

// ------------------------------------------------------------------- signed URL

/// Rebuild the URL the carrier signed.
///
/// A proxy may rewrite `Host` or `X-Forwarded-Proto`, and then the URL we
/// reconstruct is not the URL Twilio signed even though the delivery is genuine.
/// `TWILIO_WEBHOOK_URL` therefore wins when set: it is the public URL configured
/// in the carrier console. A value with no path (an origin) keeps the request's
/// path and query; a value with a path supplies its own, and its own query wins.
pub fn expected_url(override_url: &str, proto: &str, host: &str, path: &str, query: &str) -> String {
    if !override_url.trim().is_empty() {
        return resolve_url(override_url.trim(), path, query);
    }
    if host.trim().is_empty() {
        return String::new();
    }
    resolve_url(&format!("{proto}://{host}"), path, query)
}

fn resolve_url(base: &str, path: &str, query: &str) -> String {
    let (before_query, base_query) = match base.split_once('?') {
        Some((url, rest)) => (url, Some(rest)),
        None => (base, None),
    };

    // Trim a trailing slash without eating the "//" of a scheme-only base.
    let trimmed = match before_query.split_once("://") {
        Some((scheme, rest)) => format!("{scheme}://{}", rest.trim_end_matches('/')),
        None => before_query.trim_end_matches('/').to_string(),
    };

    let after_scheme = trimmed
        .split_once("://")
        .map(|(_, rest)| rest)
        .unwrap_or(&trimmed);
    let has_path = after_scheme.contains('/');

    let mut url = if has_path {
        trimmed
    } else {
        format!("{trimmed}{path}")
    };

    match (base_query, query.is_empty()) {
        (Some(own), _) => {
            url.push('?');
            url.push_str(own);
        }
        (None, false) => {
            url.push('?');
            url.push_str(query);
        }
        (None, true) => {}
    }
    url
}

// ---------------------------------------------------------------------- Twilio

/// Twilio's scheme: HMAC-SHA1(auth_token, url + sorted(key + value)), base64.
pub fn twilio_signature(auth_token: &str, url: &str, params: &[(String, String)]) -> String {
    let mut sorted: Vec<&(String, String)> = params.iter().collect();
    sorted.sort_by(|a, b| a.0.cmp(&b.0));

    let mut signed = String::from(url);
    for (name, value) in sorted {
        signed.push_str(name);
        signed.push_str(value);
    }

    let mut mac = match Hmac::<Sha1>::new_from_slice(auth_token.as_bytes()) {
        Ok(mac) => mac,
        Err(_) => return String::new(),
    };
    mac.update(signed.as_bytes());
    BASE64.encode(mac.finalize().into_bytes())
}

pub fn verify_twilio(auth_token: &str, url: &str, params: &[(String, String)], signature: &str) -> bool {
    let expected = twilio_signature(auth_token, url, params);
    !expected.is_empty() && ct_eq(&expected, signature.trim())
}

// ---------------------------------------------------------------------- Telnyx

#[derive(Debug, PartialEq, Eq)]
pub enum TelnyxError {
    MissingKey,
    BadKey,
    MissingSignature,
    Stale,
    Invalid,
}

pub fn telnyx_verifying_key(public_key_b64: &str) -> Result<VerifyingKey, TelnyxError> {
    if public_key_b64.trim().is_empty() {
        return Err(TelnyxError::MissingKey);
    }
    let raw = BASE64
        .decode(public_key_b64.trim())
        .map_err(|_| TelnyxError::BadKey)?;
    let bytes: [u8; 32] = raw.try_into().map_err(|_| TelnyxError::BadKey)?;
    VerifyingKey::from_bytes(&bytes).map_err(|_| TelnyxError::BadKey)
}

pub fn telnyx_timestamp_fresh(timestamp: &str, now_secs: i64) -> bool {
    match timestamp.trim().parse::<i64>() {
        Ok(stamp) => (now_secs - stamp).abs() <= MAX_SKEW_SECS,
        Err(_) => false,
    }
}

/// Telnyx signs `"{timestamp}|{raw body}"` with Ed25519.
pub fn verify_telnyx(
    key: &VerifyingKey,
    signature_b64: &str,
    timestamp: &str,
    body: &str,
    now_secs: i64,
) -> Result<(), TelnyxError> {
    if !telnyx_timestamp_fresh(timestamp, now_secs) {
        return Err(TelnyxError::Stale);
    }
    if signature_b64.trim().is_empty() {
        return Err(TelnyxError::MissingSignature);
    }
    let raw = BASE64
        .decode(signature_b64.trim())
        .map_err(|_| TelnyxError::Invalid)?;
    let signature = Signature::from_slice(&raw).map_err(|_| TelnyxError::Invalid)?;
    let payload = format!("{}|{}", timestamp.trim(), body);
    key.verify_strict(payload.as_bytes(), &signature)
        .map_err(|_| TelnyxError::Invalid)
}

pub fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Produce the signature Telnyx would send for `body`, for local checks and
/// tests. `seed` is the 32-byte Ed25519 secret the public key was made from.
pub fn sign_telnyx(seed: &[u8; 32], timestamp: &str, body: &str) -> String {
    use ed25519_dalek::Signer;
    let key = ed25519_dalek::SigningKey::from_bytes(seed);
    let signed = format!("{timestamp}|{body}");
    BASE64.encode(key.sign(signed.as_bytes()).to_bytes())
}

// ------------------------------------------------------- contract decisions

/// The credentials the app was started with. Empty means that contract is off.
pub struct Credentials<'a> {
    pub twilio_auth_token: &'a str,
    pub telnyx_public_key: &'a str,
    pub generic_secret: &'a str,
}

/// What the request offers as proof of origin.
pub struct Proof<'a> {
    pub url: &'a str,
    pub twilio_signature: Option<&'a str>,
    pub telnyx_signature: Option<&'a str>,
    pub telnyx_timestamp: Option<&'a str>,
    pub generic_secret: Option<&'a str>,
}

/// Why a delivery was refused, with the status and message both apps answer.
#[derive(Debug, PartialEq, Eq)]
pub enum Rejection {
    /// The contract's env var is not set: 503, not 403.
    Unconfigured(&'static str),
    /// The env var is set but unusable, e.g. a corrupt public key.
    BadConfiguration(String),
    MissingTwilioSignature,
    InvalidTwilioSignature,
    MissingTelnyxSignature,
    StaleTelnyxTimestamp,
    InvalidTelnyxSignature,
    BadSecret,
    /// Signature accepted, but the payload is not the message or call we expect.
    /// Carries the app's hint about the fields that payload must contain.
    Unreadable(&'static str),
}

impl Rejection {
    pub fn status(&self) -> u16 {
        match self {
            Rejection::Unconfigured(_) | Rejection::BadConfiguration(_) => 503,
            Rejection::Unreadable(_) => 422,
            _ => 403,
        }
    }

    pub fn message(&self) -> String {
        match self {
            Rejection::Unconfigured(env) => {
                format!("Webhook verification is not configured. Set {env}.")
            }
            Rejection::BadConfiguration(detail) => detail.clone(),
            Rejection::MissingTwilioSignature => "Missing X-Twilio-Signature".to_string(),
            Rejection::InvalidTwilioSignature => "Invalid Twilio signature".to_string(),
            Rejection::MissingTelnyxSignature => "Missing Telnyx signature".to_string(),
            Rejection::StaleTelnyxTimestamp => "Stale Telnyx timestamp".to_string(),
            Rejection::InvalidTelnyxSignature => "Invalid Telnyx signature".to_string(),
            Rejection::BadSecret => "Bad secret".to_string(),
            Rejection::Unreadable(detail) => detail.to_string(),
        }
    }
}

pub fn check_twilio(creds: &Credentials, proof: &Proof, body: &str) -> Result<(), Rejection> {
    if creds.twilio_auth_token.is_empty() {
        return Err(Rejection::Unconfigured("TWILIO_AUTH_TOKEN"));
    }
    let signature = proof.twilio_signature.unwrap_or("");
    if signature.is_empty() {
        return Err(Rejection::MissingTwilioSignature);
    }
    let params = form_pairs(body);
    if verify_twilio(creds.twilio_auth_token, proof.url, &params, signature) {
        Ok(())
    } else {
        Err(Rejection::InvalidTwilioSignature)
    }
}

pub fn check_telnyx(
    creds: &Credentials,
    proof: &Proof,
    body: &str,
    now_secs: i64,
) -> Result<(), Rejection> {
    if creds.telnyx_public_key.trim().is_empty() {
        return Err(Rejection::Unconfigured("TELNYX_PUBLIC_KEY"));
    }
    let key = telnyx_verifying_key(creds.telnyx_public_key).map_err(|err| {
        Rejection::BadConfiguration(format!("TELNYX_PUBLIC_KEY is not usable: {err:?}"))
    })?;

    let signature = proof.telnyx_signature.unwrap_or("");
    let timestamp = proof.telnyx_timestamp.unwrap_or("");
    if signature.is_empty() || timestamp.is_empty() {
        return Err(Rejection::MissingTelnyxSignature);
    }
    match verify_telnyx(&key, signature, timestamp, body, now_secs) {
        Ok(()) => Ok(()),
        Err(TelnyxError::Stale) => Err(Rejection::StaleTelnyxTimestamp),
        Err(_) => Err(Rejection::InvalidTelnyxSignature),
    }
}

/// Both apps use the same variable for the shared-secret contract, so the
/// refusal names one variable instead of one per app.
pub fn check_generic(creds: &Credentials, proof: &Proof) -> Result<(), Rejection> {
    if creds.generic_secret.is_empty() {
        return Err(Rejection::Unconfigured("WEBHOOK_SECRET"));
    }
    if ct_eq(creds.generic_secret, proof.generic_secret.unwrap_or("")) {
        Ok(())
    } else {
        Err(Rejection::BadSecret)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

    const URL: &str = "https://sms.example.com/api/inbound/sms?k=abc";
    const FORM: &str =
        "MessageSid=SM111&AccountSid=AC1&From=%2B15550001111&To=%2B442071838750&Body=Your+code+is+123456";

    #[test]
    fn secret_comparison_rejects_longer_values_with_a_null_tail() {
        let secret = "test-secret";
        let extended = format!("{secret}{}", "\0".repeat(256));
        assert!(!ct_eq(secret, &extended));
        assert!(ct_eq(secret, secret));
    }

    #[test]
    fn twilio_signature_covers_url_and_every_field() {
        let params = form_pairs(FORM);
        let signature = twilio_signature("token", URL, &params);

        assert!(verify_twilio("token", URL, &params, &signature));
        assert!(!verify_twilio("other-token", URL, &params, &signature));
        assert!(!verify_twilio(
            "token",
            "https://sms.example.com/api/inbound/sms",
            &params,
            &signature
        ));

        let mut tampered = params.clone();
        tampered.iter_mut().find(|(k, _)| k == "Body").unwrap().1 = "tampered".into();
        assert!(!verify_twilio("token", URL, &tampered, &signature));
    }

    #[test]
    fn public_url_override_beats_rewritten_host_headers() {
        let rebuilt = expected_url(
            "https://sms.example.com",
            "http",
            "internal-proxy.local",
            "/api/inbound/sms",
            "k=abc",
        );
        assert_eq!(rebuilt, URL);

        let params = form_pairs(FORM);
        let signature = twilio_signature("token", URL, &params);
        assert!(verify_twilio("token", &rebuilt, &params, &signature));

        // Without the override the reconstruction uses the rewritten host, so
        // the same genuine signature no longer matches.
        let rewritten = expected_url("", "http", "internal-proxy.local", "/api/inbound/sms", "k=abc");
        assert!(!verify_twilio("token", &rewritten, &params, &signature));
    }

    #[test]
    fn url_resolution_handles_origin_path_and_query() {
        assert_eq!(
            expected_url("", "https", "sms.example.com", "/api/inbound/sms", "k=1"),
            "https://sms.example.com/api/inbound/sms?k=1"
        );
        assert_eq!(expected_url("", "https", "", "/x", "k=1"), "");
        assert_eq!(
            expected_url(
                "https://public.example.com/",
                "http",
                "127.0.0.1:8782",
                "/api/inbound/sms",
                "k=1"
            ),
            "https://public.example.com/api/inbound/sms?k=1"
        );
        assert_eq!(
            expected_url(
                "https://public.example.com/api/inbound/sms?k=own",
                "http",
                "127.0.0.1:8782",
                "/api/inbound/sms",
                "k=1"
            ),
            "https://public.example.com/api/inbound/sms?k=own"
        );
    }

    #[test]
    fn telnyx_signature_and_freshness() {
        let key = SigningKey::from_bytes(&[7u8; 32]);
        let public = BASE64.encode(key.verifying_key().to_bytes());
        let verifying = telnyx_verifying_key(&public).unwrap();

        let body = r#"{"data":{"event_type":"call.initiated","payload":{"call_control_id":"v3:abc","state":"parked"}}}"#;
        let stamp = "1700000000";
        let signature = BASE64.encode(key.sign(format!("{stamp}|{body}").as_bytes()).to_bytes());

        assert_eq!(
            verify_telnyx(&verifying, &signature, stamp, body, 1_700_000_100),
            Ok(())
        );
        assert_eq!(
            verify_telnyx(&verifying, &signature, stamp, body, 1_700_000_000 + 600),
            Err(TelnyxError::Stale)
        );
        assert_eq!(
            verify_telnyx(
                &verifying,
                &signature,
                stamp,
                &body.replace("parked", "hangup"),
                1_700_000_100
            ),
            Err(TelnyxError::Invalid)
        );
        assert_eq!(
            verify_telnyx(&verifying, "", stamp, body, 1_700_000_100),
            Err(TelnyxError::MissingSignature)
        );
    }

    #[test]
    fn secrets_compare_in_constant_time() {
        assert!(ct_eq("test-secret", "test-secret"));
        assert!(!ct_eq("test-secret", "test-secreT"));
        assert!(!ct_eq("test-secret", "test-secret-longer"));
    }

    #[test]
    fn form_fields_decode_like_a_browser() {
        let pairs = form_pairs("From=%2B15550001111&Body=Your+code+is+771245&Empty=");
        assert_eq!(form_value(&pairs, "from"), Some("+15550001111"));
        assert_eq!(form_value(&pairs, "Body"), Some("Your code is 771245"));
        assert_eq!(form_value(&pairs, "empty"), Some(""));
    }

    fn empty_credentials() -> Credentials<'static> {
        Credentials {
            twilio_auth_token: "",
            telnyx_public_key: "",
            generic_secret: "",
        }
    }

    fn no_proof() -> Proof<'static> {
        Proof {
            url: "https://example.test/api/inbound",
            twilio_signature: None,
            telnyx_signature: None,
            telnyx_timestamp: None,
            generic_secret: None,
        }
    }

    #[test]
    fn unconfigured_contracts_answer_503_not_403() {
        let creds = empty_credentials();
        let proof = no_proof();

        let rejections = [
            check_twilio(&creds, &proof, "From=%2B1"),
            check_telnyx(&creds, &proof, "{}", 1_700_000_000),
            check_generic(&creds, &proof),
        ];
        let expected = ["TWILIO_AUTH_TOKEN", "TELNYX_PUBLIC_KEY", "WEBHOOK_SECRET"];

        for (result, env) in rejections.into_iter().zip(expected) {
            let rejection = result.unwrap_err();
            assert_eq!(rejection.status(), 503);
            assert_eq!(rejection, Rejection::Unconfigured(env));
            assert!(rejection.message().contains(env));
        }

        // One name for both apps, and the message says which variable to set.
        let generic = check_generic(&creds, &proof).unwrap_err();
        assert_eq!(generic.status(), 503);
        assert_eq!(
            generic.message(),
            "Webhook verification is not configured. Set WEBHOOK_SECRET."
        );
    }

    #[test]
    fn unusable_telnyx_key_is_503() {
        let creds = Credentials {
            twilio_auth_token: "",
            telnyx_public_key: "not-a-valid-key",
            generic_secret: "",
        };
        let rejection = check_telnyx(&creds, &no_proof(), "{}", 1).unwrap_err();
        assert_eq!(rejection.status(), 503);
        assert!(matches!(rejection, Rejection::BadConfiguration(_)));
        assert!(rejection.message().contains("TELNYX_PUBLIC_KEY"));
    }

    #[test]
    fn signatures_are_checked_once_the_contract_is_configured() {
        let creds = Credentials {
            twilio_auth_token: "test-token",
            telnyx_public_key: "6kpsY+KcUgq+9VB7Ey7F+ZVHdq6+vnuSQh7qaRRG0iw=",
            generic_secret: "test-secret",
        };
        let proof = no_proof();

        assert_eq!(
            check_twilio(&creds, &proof, "From=%2B1").unwrap_err(),
            Rejection::MissingTwilioSignature
        );
        assert_eq!(
            check_telnyx(&creds, &proof, "{}", 1_700_000_000).unwrap_err(),
            Rejection::MissingTelnyxSignature
        );
        assert_eq!(
            check_generic(&creds, &proof).unwrap_err(),
            Rejection::BadSecret
        );
    }

    #[test]
    fn statuses_split_503_422_and_403() {
        assert_eq!(Rejection::Unconfigured("X").status(), 503);
        assert_eq!(Rejection::BadConfiguration("bad".into()).status(), 503);
        let unreadable = Rejection::Unreadable("The SMS payload needs To, From and Body");
        assert_eq!(unreadable.status(), 422);
        assert_eq!(unreadable.message(), "The SMS payload needs To, From and Body");
        assert_eq!(Rejection::BadSecret.status(), 403);
        assert_eq!(Rejection::MissingTwilioSignature.status(), 403);
        assert_eq!(Rejection::InvalidTelnyxSignature.status(), 403);
    }
}
