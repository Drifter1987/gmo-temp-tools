//! User-facing application information. Keeping the release notes here means
//! the desktop window and the local web app show exactly the same history.

pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");
pub const OWNER: &str = "drifter1987";

pub struct ToolRelease {
    pub tool: &'static str,
    pub version: &'static str,
    pub summary: &'static str,
    /// Newest version first. Each entry is `(version, change)`.
    pub changelog: &'static [(&'static str, &'static str)],
}

pub const TOOL_RELEASES: &[ToolRelease] = &[
    ToolRelease {
        tool: "Mail",
        version: "0.3.0",
        summary: "Disposable addresses, live inbox refresh, provider health and code extraction.",
        changelog: &[
            (
                "0.3.0",
                "Explicit refresh now bypasses the provider cache; the visible inbox refreshes itself and unread counts settle as messages are read.",
            ),
            (
                "0.3.0",
                "Provider health now separates current failures from historical fallback data.",
            ),
            (
                "0.2.0",
                "Moved into the single GMO TempTools window with a matching web tab.",
            ),
            (
                "0.2.0",
                "Added provider health, retries and deterministic fallback across six keyless providers.",
            ),
            (
                "0.2.0",
                "Sanitized HTML mail and automatic verification-code and link extraction.",
            ),
            (
                "0.1.0",
                "Initial disposable-mail inbox, local persistence and provider picker.",
            ),
        ],
    },
    ToolRelease {
        tool: "SMS",
        version: "0.3.0",
        summary: "Public temporary numbers, received texts and signed carrier deliveries.",
        changelog: &[
            (
                "0.3.0",
                "Choose a country and a listed public number instead of accepting a random one; source checks now run on demand.",
            ),
            (
                "0.3.0",
                "The tab is labelled Beta because public inboxes are shared and upstream availability varies.",
            ),
            (
                "0.2.0",
                "Moved into the single GMO TempTools window with a matching web tab.",
            ),
            (
                "0.2.0",
                "Added the cached receive-sms.io source health and last-good fallback.",
            ),
            (
                "0.2.0",
                "Signed Twilio, Telnyx and generic inbound webhooks with retry de-duplication.",
            ),
            (
                "0.1.0",
                "Initial temporary-number list and received-text view.",
            ),
        ],
    },
    ToolRelease {
        tool: "OTP",
        version: "0.3.0",
        summary: "Offline code extraction and a local TOTP authenticator vault.",
        changelog: &[
            (
                "0.3.0",
                "The desktop tab now checks the shared vault on a short interval instead of decrypting it on every repaint.",
            ),
            (
                "0.2.0",
                "Moved into the single GMO TempTools window and the local web app.",
            ),
            (
                "0.2.0",
                "Added DPAPI encryption at rest, atomic writes and a cross-process vault lock.",
            ),
            (
                "0.2.0",
                "Added shared desktop/web reads and one-second TOTP refresh.",
            ),
            (
                "0.1.0",
                "Initial offline verification-code extractor and TOTP generator.",
            ),
        ],
    },
];
