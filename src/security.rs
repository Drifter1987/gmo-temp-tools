//! The per-install secret that gates the loopback web app and its APIs. It is
//! created once under `.run/` and is never printed by the app. The unified app
//! shares one token protocol and one fail-closed behavior across its services.

pub use local_auth::load_or_create_token;
