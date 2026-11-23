//! Health of the public SMS source and the fallback paths.
//!
//! The public source is a shared website with an unknown rate limit. A failed
//! refresh does not discard the last good thread: the cache is a fallback, and
//! signed carrier webhooks are always an independent path. This state is kept
//! in memory so the native window, the service page and `/api/status` agree.

use serde::Serialize;

use crate::now_ms;

pub const SOURCE_RULE: &str =
    "receive-sms.io public page; numbers are stored in E.164 form and read by digits";

#[derive(Clone, Debug, Serialize)]
pub struct SourceHealth {
    pub provider: String,
    pub status: String,
    pub consecutive_failures: u32,
    pub last_error: String,
    pub last_attempt_at: f64,
    pub last_success_at: f64,
    pub last_failure_at: f64,
    pub last_retry_count: u32,
    pub last_source: String,
}

impl Default for SourceHealth {
    fn default() -> Self {
        Self {
            provider: "receive-sms.io".to_string(),
            status: "unknown".to_string(),
            consecutive_failures: 0,
            last_error: String::new(),
            last_attempt_at: 0.0,
            last_success_at: 0.0,
            last_failure_at: 0.0,
            last_retry_count: 0,
            last_source: "none".to_string(),
        }
    }
}

impl SourceHealth {
    pub fn attempt(&mut self) {
        self.last_attempt_at = now_ms();
        self.last_source = "live".to_string();
    }

    pub fn succeeded(&mut self, retries: u32) {
        self.status = "ok".to_string();
        self.consecutive_failures = 0;
        self.last_success_at = now_ms();
        self.last_retry_count = retries;
        self.last_error.clear();
        self.last_source = "live".to_string();
    }

    pub fn failed(&mut self, error: &str) {
        self.consecutive_failures = self.consecutive_failures.saturating_add(1);
        self.last_failure_at = now_ms();
        self.last_error = error.to_string();
        self.status = if self.last_success_at > 0.0 {
            "degraded".to_string()
        } else {
            "down".to_string()
        };
        self.last_source = "live".to_string();
    }

    /// The request failed, but the last good list is still available.
    pub fn using_cache(&mut self, error: &str) {
        self.failed(error);
        self.status = "degraded".to_string();
        self.last_source = "cache".to_string();
    }

}

#[derive(Clone, Debug, Default)]
pub struct Health {
    pub source: SourceHealth,
}

impl Health {
    pub fn new() -> Self {
        Self {
            source: SourceHealth::default(),
        }
    }

    pub fn snapshot(&self) -> SourceHealth {
        self.source.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn health_moves_from_unknown_to_ok_and_degraded() {
        let mut health = Health::new();
        assert_eq!(health.snapshot().status, "unknown");

        health.source.attempt();
        health.source.succeeded(1);
        let snapshot = health.snapshot();
        assert_eq!(snapshot.status, "ok");
        assert_eq!(snapshot.last_retry_count, 1);
        assert_eq!(snapshot.consecutive_failures, 0);

        health.source.attempt();
        health.source.using_cache("receive-sms.io returned 429");
        let snapshot = health.snapshot();
        assert_eq!(snapshot.status, "degraded");
        assert_eq!(snapshot.last_source, "cache");
        assert_eq!(snapshot.consecutive_failures, 1);
        assert!(snapshot.last_error.contains("429"));

        health.source.attempt();
        health.source.succeeded(0);
        assert_eq!(health.snapshot().status, "ok");
        assert!(health.snapshot().last_error.is_empty());
    }

    #[test]
    fn a_never_successful_source_is_down() {
        let mut health = Health::new();
        health.source.attempt();
        health.source.failed("connection refused");
        let snapshot = health.snapshot();
        assert_eq!(snapshot.status, "down");
        assert_eq!(snapshot.last_source, "live");
    }
}
