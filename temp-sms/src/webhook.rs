//! Which carrier sent this text delivery, and what does its payload say?
//!
//! The signing rules live in the `webhook-sign` crate; this module only knows
//! the SMS payload shapes and which fields mark a Twilio post.

pub use webhook_sign::{
    check_generic, check_telnyx, check_twilio, expected_url, form_pairs, form_value, now_secs,
    Credentials, Proof, Rejection,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    Twilio,
    Telnyx,
    Generic,
}

/// Which contract does this request speak?
///
/// An explicit `/api/inbound/<provider>` path wins; otherwise the payload
/// decides. A form carrying Twilio's own ids is Twilio even when it also
/// carries `?k=`.
pub fn detect(path_provider: Option<&str>, body: &str) -> Provider {
    if let Some(name) = path_provider {
        match name.to_ascii_lowercase().as_str() {
            "twilio" => return Provider::Twilio,
            "telnyx" => return Provider::Telnyx,
            // /api/inbound/sms is the documented alias for /api/inbound: the
            // segment names the app, not the carrier, so keep detecting from
            // the payload instead of forcing the generic contract.
            "sms" | "" => {}
            _ => return Provider::Generic,
        }
    }

    if body.trim_start().starts_with('{') {
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(body) {
            if value.get("data").is_some() {
                return Provider::Telnyx;
            }
        }
        return Provider::Generic;
    }

    let pairs = form_pairs(body);
    let looks_twilio = ["SmsSid", "MessageSid", "AccountSid", "SmsStatus"]
        .iter()
        .any(|key| form_value(&pairs, key).is_some());
    if looks_twilio {
        Provider::Twilio
    } else {
        Provider::Generic
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct InboundMessage {
    pub sid: String,
    pub from: String,
    pub to: String,
    pub body: String,
}

pub fn parse_twilio(body: &str) -> Option<InboundMessage> {
    let pairs = form_pairs(body);
    let to = form_value(&pairs, "To")?;
    Some(InboundMessage {
        sid: form_value(&pairs, "SmsSid")
            .or_else(|| form_value(&pairs, "MessageSid"))
            .unwrap_or("")
            .to_string(),
        from: form_value(&pairs, "From").unwrap_or("unknown").to_string(),
        to: to.to_string(),
        body: form_value(&pairs, "Body").unwrap_or("").to_string(),
    })
}

pub fn parse_telnyx(body: &str) -> Option<InboundMessage> {
    let value: serde_json::Value = serde_json::from_str(body).ok()?;
    let payload = value.get("data")?.get("payload")?;
    let from = payload
        .get("from")
        .and_then(|f| f.get("phone_number"))
        .and_then(|v| v.as_str())?;
    let to = payload
        .get("to")
        .and_then(|t| t.get(0))
        .and_then(|t| t.get("phone_number"))
        .and_then(|v| v.as_str())?;
    Some(InboundMessage {
        sid: payload
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string(),
        from: from.to_string(),
        to: to.to_string(),
        body: payload
            .get("text")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string(),
    })
}

/// `{from, to, body}` or `{sender, number, message}` — either JSON or form shape.
pub fn parse_generic(body: &str) -> Option<InboundMessage> {
    if body.trim_start().starts_with('{') {
        let value: serde_json::Value = serde_json::from_str(body).ok()?;
        let pick = |keys: &[&str]| -> Option<String> {
            keys.iter().find_map(|key| {
                value
                    .get(*key)
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
            })
        };
        let to = pick(&["to", "number", "recipient"])?;
        return Some(InboundMessage {
            sid: pick(&["id", "sid"]).unwrap_or_default(),
            from: pick(&["from", "sender", "source"]).unwrap_or_else(|| "unknown".to_string()),
            to,
            body: pick(&["body", "message", "text"]).unwrap_or_default(),
        });
    }

    let pairs = form_pairs(body);
    let to = form_value(&pairs, "to")
        .or_else(|| form_value(&pairs, "number"))
        .or_else(|| form_value(&pairs, "recipient"))?;
    Some(InboundMessage {
        sid: form_value(&pairs, "id")
            .or_else(|| form_value(&pairs, "sid"))
            .unwrap_or("")
            .to_string(),
        from: form_value(&pairs, "from")
            .or_else(|| form_value(&pairs, "sender"))
            .unwrap_or("unknown")
            .to_string(),
        to: to.to_string(),
        body: form_value(&pairs, "body")
            .or_else(|| form_value(&pairs, "message"))
            .or_else(|| form_value(&pairs, "text"))
            .unwrap_or("")
            .to_string(),
    })
}

pub fn parse(provider: Provider, body: &str) -> Option<InboundMessage> {
    match provider {
        Provider::Twilio => parse_twilio(body),
        Provider::Telnyx => parse_telnyx(body),
        Provider::Generic => parse_generic(body),
    }
}

/// The 422 hint for a signed body that is not a message: name the fields it
/// must carry, so the sender can fix the payload rather than guess.
pub const UNREADABLE: &str = "The SMS payload needs To, From and Body";

/// Check the delivery's proof, then read the message. An unconfigured contract
/// is `Unconfigured` (503), a signed body we cannot read is `Unreadable` (422).
pub fn accept(
    provider: Provider,
    body: &str,
    proof: &Proof,
    creds: &Credentials,
    now: i64,
) -> Result<InboundMessage, Rejection> {
    match provider {
        Provider::Twilio => check_twilio(creds, proof, body)?,
        Provider::Telnyx => check_telnyx(creds, proof, body, now)?,
        Provider::Generic => check_generic(creds, proof)?,
    }
    parse(provider, body).ok_or(Rejection::Unreadable(UNREADABLE))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detection_prefers_payload_identity() {
        let twilio = "SmsSid=SM1&AccountSid=AC1&From=%2B1&To=%2B2&Body=hi";
        assert_eq!(detect(None, twilio), Provider::Twilio);
        assert_eq!(detect(Some("twilio"), twilio), Provider::Twilio);

        // The documented /api/inbound/sms path must not force the generic
        // contract: it is an alias for /api/inbound and still detects.
        assert_eq!(detect(Some("sms"), twilio), Provider::Twilio);
        assert_eq!(detect(Some("SMS"), twilio), Provider::Twilio);
        assert_eq!(
            detect(Some("sms"), r#"{"data":{"event_type":"message.received"}}"#),
            Provider::Telnyx
        );
        assert_eq!(detect(Some("sms"), "from=%2B1&to=%2B2&body=hi"), Provider::Generic);
        assert_eq!(detect(Some("other"), twilio), Provider::Generic);
        assert_eq!(detect(None, "from=%2B1&to=%2B2&body=hi"), Provider::Generic);
        assert_eq!(
            detect(None, r#"{"data":{"event_type":"message.received"}}"#),
            Provider::Telnyx
        );
        assert_eq!(detect(None, r#"{"from":"+1","to":"+2","body":"hi"}"#), Provider::Generic);
    }

    #[test]
    fn parses_all_three_shapes() {
        let twilio = parse_twilio("SmsSid=SM9&From=%2B15550001111&To=%2B15550002222&Body=code+771245").unwrap();
        assert_eq!(twilio.sid, "SM9");
        assert_eq!(twilio.to, "+15550002222");
        assert_eq!(twilio.body, "code 771245");

        let telnyx = parse_telnyx(
            r#"{"data":{"payload":{"id":"msg_2","from":{"phone_number":"+15550003333"},"to":[{"phone_number":"+15550004444"}],"text":"code 552103"}}}"#,
        )
        .unwrap();
        assert_eq!(telnyx.from, "+15550003333");
        assert_eq!(telnyx.to, "+15550004444");
        assert_eq!(telnyx.body, "code 552103");

        let generic = parse_generic(r#"{"from":"+15550005555","to":"+15550006666","body":"code 999111"}"#).unwrap();
        assert_eq!(generic.body, "code 999111");
    }

    const TEST_TELNYX_KEY: &str = "6kpsY+KcUgq+9VB7Ey7F+ZVHdq6+vnuSQh7qaRRG0iw=";

    fn unconfigured() -> Credentials<'static> {
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
    fn unconfigured_contracts_answer_503() {
        let creds = unconfigured();
        let proof = no_proof();
        let now = 1_700_000_000;

        let attempts = [
            (
                accept(
                    Provider::Twilio,
                    "SmsSid=SM1&From=%2B1&To=%2B2&Body=hi",
                    &proof,
                    &creds,
                    now,
                ),
                "TWILIO_AUTH_TOKEN",
            ),
            (
                accept(
                    Provider::Telnyx,
                    r#"{"data":{"payload":{"text":"hi"}}}"#,
                    &proof,
                    &creds,
                    now,
                ),
                "TELNYX_PUBLIC_KEY",
            ),
            (
                accept(Provider::Generic, "from=%2B1&to=%2B2&body=hi", &proof, &creds, now),
                "WEBHOOK_SECRET",
            ),
        ];

        for (result, env) in attempts {
            let rejection = result.unwrap_err();
            assert_eq!(rejection.status(), 503, "{env} should answer 503");
            assert_eq!(rejection, Rejection::Unconfigured(env));
            assert!(rejection.message().contains(env));
        }
    }

    #[test]
    fn signed_but_unreadable_payloads_answer_422() {
        let creds = Credentials {
            twilio_auth_token: "test-token",
            telnyx_public_key: TEST_TELNYX_KEY,
            generic_secret: "test-secret",
        };
        let url = "https://example.test/api/inbound";
        let now = 1_700_000_000;

        // Twilio: the signature is real, but the form carries no To.
        let body = "SmsSid=SM1&From=%2B15550001111";
        let signature = webhook_sign::twilio_signature("test-token", url, &form_pairs(body));
        let proof = Proof {
            url,
            twilio_signature: Some(&signature),
            telnyx_signature: None,
            telnyx_timestamp: None,
            generic_secret: None,
        };
        let rejection = accept(Provider::Twilio, body, &proof, &creds, now).unwrap_err();
        assert_eq!(rejection, Rejection::Unreadable(UNREADABLE));
        assert_eq!(rejection.message(), "The SMS payload needs To, From and Body");
        assert_eq!(rejection.status(), 422);

        // Telnyx: a valid envelope with no message inside it.
        let body = r#"{"data":{"event_type":"message.received"}}"#;
        let stamp = now.to_string();
        let signature = webhook_sign::sign_telnyx(&[7u8; 32], &stamp, body);
        let proof = Proof {
            url,
            twilio_signature: None,
            telnyx_signature: Some(&signature),
            telnyx_timestamp: Some(&stamp),
            generic_secret: None,
        };
        let rejection = accept(Provider::Telnyx, body, &proof, &creds, now).unwrap_err();
        assert_eq!(rejection, Rejection::Unreadable(UNREADABLE));
        assert_eq!(rejection.message(), "The SMS payload needs To, From and Body");
        assert_eq!(rejection.status(), 422);
    }
}
