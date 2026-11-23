//! Local delivery history for provider/domain selection.
//!
//! This is deliberately small: it records what happened to requests this app
//! made. It does not probe providers or claim to know a domain's reputation.

use crate::store::{InboxRow, Store};
use crate::util::{domain_of, now_ms};
use crate::persist;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

const STATE_VERSION: u32 = 1;

/// The observation window for a terminal inbox outcome. An inbox removed
/// before this point was never observed for the full window, so it is not
/// classified as quiet.
const OUTCOME_WINDOW_MS: f64 = 15.0 * 60.0 * 1000.0;

fn legacy_default() -> bool {
    true
}

#[derive(Clone, Serialize, Deserialize)]
struct Entry {
    provider: String,
    domain: String,
    attempts: u64,
    created: u64,
    #[serde(default)]
    delivered: u64,
    #[serde(default)]
    unclassified: u64,
    create_failures: u64,
    #[serde(default)]
    last_create_failure_at: f64,
    #[serde(default)]
    last_create_error: String,
    list_failures: u64,
    last_attempt_at: f64,
    last_success_at: f64,
    last_error: String,
    /// Entries read from the pre-counter format have no reliable delivered
    /// value or domain attribution. Keep them visible, but mark them history.
    #[serde(default = "legacy_default")]
    legacy: bool,
}

impl Entry {
    fn new(provider: &str, domain: &str) -> Self {
        Self {
            provider: provider.to_string(),
            domain: domain.to_string(),
            attempts: 0,
            created: 0,
            delivered: 0,
            unclassified: 0,
            create_failures: 0,
            last_create_failure_at: 0.0,
            last_create_error: String::new(),
            list_failures: 0,
            last_attempt_at: 0.0,
            last_success_at: 0.0,
            last_error: String::new(),
            legacy: false,
        }
    }
}

/// The state of one inbox at the end of its local observation.
///
/// `Open` means the inbox is still active. `Delivered` means at least one
/// external message was observed; that can happen after the 15-minute window,
/// matching the existing lifetime external-mail count. `Quiet` means the inbox
/// was observed for the full window without external mail. `Unknown` means the
/// inbox expired or was deleted before the window closed, so its 15-minute
/// outcome was never observed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutcomeState {
    Open,
    Delivered,
    Quiet,
    Unknown,
}

/// A durable, per-inbox outcome record. It outlives the inbox itself and is
/// the source for the non-legacy domain created/delivered counts.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Outcome {
    pub inbox_id: String,
    pub provider: String,
    pub domain: String,
    pub created_at: f64,
    pub expires_at: f64,
    #[serde(default)]
    pub first_external_message_at: Option<f64>,
    #[serde(default)]
    pub settled_at: Option<f64>,
    pub state: OutcomeState,
}

/// Counts that predate the per-inbox ledger for a provider/domain. They keep
/// historical rows intact while the ledger supplies every count after the
/// migration point.
#[derive(Clone, Serialize, Deserialize)]
struct Baseline {
    provider: String,
    domain: String,
    created: u64,
    delivered: u64,
}

/// List failures that belong to the provider connection rather than to one
/// domain. These are kept separately so an outage or parser change cannot mark
/// every domain row with the same error.
#[derive(Clone, Serialize, Deserialize)]
struct ProviderEntry {
    provider: String,
    list_failures: u64,
    #[serde(default)]
    last_error: String,
    #[serde(default)]
    last_failure_at: f64,
    #[serde(default)]
    last_success_at: f64,
}

impl ProviderEntry {
    fn new(provider: &str) -> Self {
        Self {
            provider: provider.to_string(),
            list_failures: 0,
            last_error: String::new(),
            last_failure_at: 0.0,
            last_success_at: 0.0,
        }
    }
}

/// Provider-wide list status for the UI and `/api/acceptance`. It is not a
/// domain row and does not claim anything about a domain's deliverability.
#[derive(Clone, Default, Serialize)]
pub struct ProviderRow {
    pub provider: String,
    pub list_failures: u64,
    pub create_failures: u64,
    pub last_error: String,
    pub last_create_error: String,
    pub last_failure_at: f64,
    pub last_create_failure_at: f64,
    pub last_success_at: f64,
    pub last_attempt_at: f64,
    /// Live local inboxes for this provider at the time the row was built.
    pub active_inboxes: u64,
    /// Whether `last_error` applies while an inbox is active. A historical
    /// failure count can remain visible without presenting its old message as
    /// the provider's current status.
    pub error_is_current: bool,
}

/// One row for the web UI and the JSON endpoint.
#[derive(Clone, Default, Serialize)]
pub struct Row {
    pub provider: String,
    pub domain: String,
    pub attempts: u64,
    pub created: u64,
    pub delivered: u64,
    pub unclassified: u64,
    pub create_failures: u64,
    pub list_failures: u64,
    pub pending: u64,
    pub quiet: u64,
    pub last_error: String,
    pub legacy: bool,
}

#[derive(Serialize, Deserialize, Default)]
pub struct Health {
    #[serde(default)]
    version: u32,
    #[serde(default)]
    entries: Vec<Entry>,
    #[serde(default)]
    outcomes: Vec<Outcome>,
    #[serde(default)]
    baselines: Vec<Baseline>,
    #[serde(default)]
    provider_failures: Vec<ProviderEntry>,
    /// The first load after the provider ledger was introduced moves old
    /// domain-attributed list failures once. Later domain-specific list errors
    /// must remain domain-specific.
    #[serde(default)]
    provider_failures_migrated: bool,
    #[serde(skip)]
    path: PathBuf,
    #[serde(skip)]
    last_error: Option<String>,
}

impl Health {
    pub fn load(path: impl Into<PathBuf>) -> Result<Self, String> {
        let path = path.into();
        let raw = std::fs::read_to_string(&path).ok();
        let mut backup_warning = None;
        if let Some(raw) = &raw {
            if let Err(err) = backup_v1(&path, raw) {
                backup_warning = Some(err);
            }
        }
        let (health, warning) =
            persist::load_json::<Self>(&path, STATE_VERSION, |health| health.version)?;
        let mut health = health.unwrap_or_default();
        health.path = path;
        health.version = STATE_VERSION;
        health.last_error = warning;
        health.migrate_provider_failures();
        if let Some(err) = backup_warning {
            health.last_error = Some(match health.last_error.take() {
                Some(existing) => format!("{existing}; {err}"),
                None => err,
            });
        }
        Ok(health)
    }

    pub fn save(&mut self) -> Result<(), String> {
        self.version = STATE_VERSION;
        if self.path.as_os_str().is_empty() {
            return Ok(());
        }
        let raw = serde_json::to_vec_pretty(self)
            .map_err(|err| format!("could not serialize acceptance state: {err}"))?;
        match persist::write_with_backup(&self.path, &raw) {
            Ok(()) => {
                self.last_error = None;
                Ok(())
            }
            Err(err) => {
                let message = format!("could not write acceptance state: {err}");
                self.last_error = Some(message.clone());
                Err(message)
            }
        }
    }

    fn persist(&mut self) {
        if let Err(err) = self.save() {
            eprintln!("temp-email: {err}");
        }
    }

    pub fn last_error(&self) -> Option<&str> {
        self.last_error.as_deref()
    }

    fn entry(&mut self, provider: &str, domain: &str) -> &mut Entry {
        if let Some(index) = self.entries.iter().position(|entry| {
            entry.provider == provider && entry.domain == domain && !entry.legacy
        }) {
            return &mut self.entries[index];
        }
        self.entries.push(Entry::new(provider, domain));
        self.entries.last_mut().expect("entry was just pushed")
    }

    pub fn attempt(&mut self, provider: &str, domain: &str) {
        let entry = self.entry(provider, domain);
        entry.attempts += 1;
        entry.last_attempt_at = now_ms();
    }

    pub fn created(&mut self, provider: &str, domain: &str) {
        let entry = self.entry(provider, domain);
        entry.created += 1;
        entry.last_success_at = now_ms();
        entry.last_error.clear();
        entry.last_create_error.clear();
        self.persist();
    }

    pub fn external_delivered(&mut self, provider: &str, domain: &str) {
        let entry = self.entry(provider, domain);
        entry.delivered += 1;
        self.persist();
    }

    /// Return the durable outcome for an inbox, if the inbox has reached the
    /// ledger. Used by tests and by other local readers; the UI keeps using
    /// the compact domain rows.
    #[cfg(test)]
    pub fn outcome(&self, inbox_id: &str) -> Option<&Outcome> {
        self.outcomes.iter().find(|outcome| outcome.inbox_id == inbox_id)
    }

    /// Create the per-inbox ledger entry as soon as an inbox exists. Existing
    /// active inboxes are backfilled by `reconcile_with_store` on startup.
    pub fn start_outcome(&mut self, row: &InboxRow) {
        self.ensure_outcome(row);
        self.ensure_baseline(&row.provider, domain_of(&row.address));
        self.persist();
    }

    /// Mark the first external message once, and make the outcome terminal
    /// delivered. The first timestamp is retained for reconciliation.
    pub fn mark_outcome_delivered(&mut self, row: &InboxRow, at: f64) {
        let index = self.ensure_outcome(row);
        let outcome = &mut self.outcomes[index];
        outcome.first_external_message_at = Some(
            outcome
                .first_external_message_at
                .map_or(at, |previous| previous.min(at)),
        );
        outcome.state = OutcomeState::Delivered;
        self.ensure_baseline(&row.provider, domain_of(&row.address));
        self.persist();
    }

    /// Settle an inbox that is being deleted or has just expired. This is the
    /// only place an inbox gets a terminal quiet/unknown outcome; a delivered
    /// inbox keeps its delivered state.
    pub fn settle_outcome(&mut self, row: &InboxRow, now: f64) {
        let index = self.ensure_outcome(row);
        let first_external = self.outcomes[index]
            .first_external_message_at
            .or(row.first_external_message_at);
        let outcome = &mut self.outcomes[index];
        if first_external.is_some() {
            outcome.first_external_message_at = first_external;
            outcome.state = OutcomeState::Delivered;
        } else if now >= row.created_at + OUTCOME_WINDOW_MS {
            outcome.state = OutcomeState::Quiet;
        } else {
            outcome.state = OutcomeState::Unknown;
        }
        outcome.settled_at = Some(now);
        self.ensure_baseline(&row.provider, domain_of(&row.address));
        self.persist();
    }

    /// Backfill active inboxes and record the pre-ledger counts for every
    /// non-legacy domain row. This is called once, after both the store and
    /// this ledger have loaded.
    pub fn reconcile_with_store(&mut self, store: &Store) {
        for account in &store.accounts {
            self.ensure_outcome(&account.row);
        }
        let domains: Vec<(String, String)> = self
            .entries
            .iter()
            .filter(|entry| !entry.legacy)
            .map(|entry| (entry.provider.clone(), entry.domain.clone()))
            .collect();
        for (provider, domain) in domains {
            self.ensure_baseline(&provider, &domain);
        }
        self.persist();
    }

    pub fn unclassified_message(&mut self, provider: &str, domain: &str) {
        let entry = self.entry(provider, domain);
        entry.unclassified += 1;
        self.persist();
    }

    pub fn create_failed(&mut self, provider: &str, domain: &str, error: &str) {
        let entry = self.entry(provider, domain);
        entry.create_failures += 1;
        entry.last_create_failure_at = now_ms();
        entry.last_create_error = error.to_string();
        entry.last_error = error.to_string();
        self.persist();
    }

    /// A failure tied to the inbox/domain rather than the provider connection.
    /// Provider-wide failures use `provider_list_failed` instead.
    pub fn list_failed(&mut self, provider: &str, domain: &str, error: &str) {
        let entry = self.entry(provider, domain);
        entry.list_failures += 1;
        entry.last_error = error.to_string();
        self.persist();
    }

    /// Record an outage, rate-limit, network, timeout, or parser/deserialization
    /// failure at the provider level. It is not copied into any domain row.
    pub fn provider_list_failed(&mut self, provider: &str, error: &str) {
        let entry = self.provider_entry(provider);
        entry.list_failures += 1;
        entry.last_error = error.to_string();
        entry.last_failure_at = now_ms();
        self.persist();
    }

    /// A successful provider list clears the displayed provider error but
    /// keeps the cumulative failure count as evidence.
    pub fn provider_list_succeeded(&mut self, provider: &str) {
        if let Some(entry) = self
            .provider_failures
            .iter_mut()
            .find(|entry| entry.provider == provider)
        {
            let had_error = !entry.last_error.is_empty();
            entry.last_error.clear();
            entry.last_success_at = now_ms();
            // Routine polls happen every few seconds. Only touch the state file
            // when there is a stale error to clear; otherwise this would write
            // the whole ledger on every successful poll.
            if had_error {
                self.persist();
            }
        }
    }

    /// A successful list makes a previous list error stale. Keep
    /// `list_failures` as cumulative evidence; only the displayed error clears.
    pub fn list_succeeded(&mut self, provider: &str, domain: &str) {
        let entry = self.entry(provider, domain);
        if entry.list_failures > 0 && !entry.last_error.is_empty() {
            entry.last_error.clear();
            self.persist();
        }
    }

    pub fn provider_rows(&self) -> Vec<ProviderRow> {
        let mut rows: Vec<ProviderRow> = self
            .provider_failures
            .iter()
            .map(|entry| ProviderRow {
                provider: entry.provider.clone(),
                list_failures: entry.list_failures,
                create_failures: 0,
                last_error: entry.last_error.clone(),
                last_create_error: String::new(),
                last_failure_at: entry.last_failure_at,
                last_create_failure_at: 0.0,
                last_success_at: entry.last_success_at,
                last_attempt_at: entry.last_failure_at.max(entry.last_success_at),
                active_inboxes: 0,
                error_is_current: false,
            })
            .collect();
        for entry in &self.entries {
            if entry.create_failures == 0 && entry.last_create_error.is_empty() {
                continue;
            }
            let row = match rows.iter_mut().find(|row| row.provider == entry.provider) {
                Some(row) => row,
                None => {
                    rows.push(ProviderRow {
                        provider: entry.provider.clone(),
                        ..ProviderRow::default()
                    });
                    rows.last_mut().expect("provider row was just pushed")
                }
            };
            row.create_failures += entry.create_failures;
            row.last_attempt_at = row
                .last_attempt_at
                .max(entry.last_create_failure_at)
                .max(entry.last_success_at);
            if entry.last_create_failure_at >= row.last_create_failure_at {
                row.last_create_failure_at = entry.last_create_failure_at;
                row.last_create_error = entry.last_create_error.clone();
            }
        }
        rows.sort_by(|a, b| a.provider.cmp(&b.provider));
        rows
    }

    fn provider_entry(&mut self, provider: &str) -> &mut ProviderEntry {
        if let Some(index) = self
            .provider_failures
            .iter()
            .position(|entry| entry.provider == provider)
        {
            return &mut self.provider_failures[index];
        }
        self.provider_failures.push(ProviderEntry::new(provider));
        self.provider_failures
            .last_mut()
            .expect("provider entry was just pushed")
    }

    /// One-time migration for ledgers written before provider-wide failures
    /// existed. Historical failures have no reliable domain meaning, so they
    /// are moved to the provider bucket and the domain rows are cleaned.
    fn migrate_provider_failures(&mut self) {
        if self.provider_failures_migrated {
            return;
        }
        self.provider_failures_migrated = true;
        let moved: Vec<(String, u64, String, f64, f64)> = self
            .entries
            .iter()
            .filter(|entry| entry.list_failures > 0)
            .map(|entry| {
                (
                    entry.provider.clone(),
                    entry.list_failures,
                    if entry.create_failures == 0 {
                        entry.last_error.clone()
                    } else {
                        String::new()
                    },
                    entry.last_attempt_at,
                    entry.last_success_at,
                )
            })
            .collect();
        for (provider, failures, error, failed_at, succeeded_at) in &moved {
            let entry = self.provider_entry(provider);
            entry.list_failures += failures;
            if !error.is_empty() {
                entry.last_error = error.clone();
            }
            entry.last_failure_at = entry.last_failure_at.max(*failed_at);
            entry.last_success_at = entry.last_success_at.max(*succeeded_at);
        }
        for entry in &mut self.entries {
            if entry.list_failures > 0 {
                entry.list_failures = 0;
                if entry.create_failures == 0 {
                    entry.last_error.clear();
                }
            }
        }
        self.persist();
    }

    pub fn rows(&self) -> Vec<Row> {
        let mut rows: Vec<Row> = self
            .entries
            .iter()
            .map(|entry| {
                let (created, delivered) = if entry.legacy {
                    (entry.created, entry.delivered)
                } else {
                    self.derived_counts(&entry.provider, &entry.domain)
                        .unwrap_or((entry.created, entry.delivered))
                };
                Row {
                    provider: entry.provider.clone(),
                    domain: entry.domain.clone(),
                    attempts: entry.attempts,
                    created,
                    delivered,
                    unclassified: entry.unclassified,
                    create_failures: entry.create_failures,
                    list_failures: entry.list_failures,
                    pending: 0,
                    quiet: 0,
                    last_error: entry.last_error.clone(),
                    legacy: entry.legacy,
                }
            })
            .collect();
        rows.sort_by(|a, b| (&a.provider, &a.domain).cmp(&(&b.provider, &b.domain)));
        rows
    }

    fn ensure_outcome(&mut self, row: &InboxRow) -> usize {
        let index = self
            .outcomes
            .iter()
            .position(|outcome| outcome.inbox_id == row.id);
        let index = match index {
            Some(index) => index,
            None => {
                self.outcomes.push(Outcome {
                    inbox_id: row.id.clone(),
                    provider: row.provider.clone(),
                    domain: domain_of(&row.address).to_string(),
                    created_at: row.created_at,
                    expires_at: row.expires_at,
                    first_external_message_at: None,
                    settled_at: None,
                    state: OutcomeState::Open,
                });
                self.outcomes.len() - 1
            }
        };
        if let Some(at) = row.first_external_message_at {
            let outcome = &mut self.outcomes[index];
            outcome.first_external_message_at = Some(
                outcome
                    .first_external_message_at
                    .map_or(at, |previous| previous.min(at)),
            );
            outcome.state = OutcomeState::Delivered;
        }
        index
    }

    fn ensure_baseline(&mut self, provider: &str, domain: &str) {
        if self
            .baselines
            .iter()
            .any(|baseline| baseline.provider == provider && baseline.domain == domain)
        {
            return;
        }
        let Some(entry) = self
            .entries
            .iter()
            .find(|entry| {
                entry.provider == provider && entry.domain == domain && !entry.legacy
            })
        else {
            return;
        };
        let (outcome_created, outcome_delivered) = self.outcome_counts(provider, domain);
        self.baselines.push(Baseline {
            provider: provider.to_string(),
            domain: domain.to_string(),
            created: entry.created.saturating_sub(outcome_created),
            delivered: entry.delivered.saturating_sub(outcome_delivered),
        });
    }

    fn outcome_counts(&self, provider: &str, domain: &str) -> (u64, u64) {
        let mut created = 0;
        let mut delivered = 0;
        for outcome in &self.outcomes {
            if outcome.provider == provider && outcome.domain == domain {
                created += 1;
                if outcome.state == OutcomeState::Delivered {
                    delivered += 1;
                }
            }
        }
        (created, delivered)
    }

    fn derived_counts(&self, provider: &str, domain: &str) -> Option<(u64, u64)> {
        let baseline = self
            .baselines
            .iter()
            .find(|baseline| baseline.provider == provider && baseline.domain == domain);
        let has_outcomes = self
            .outcomes
            .iter()
            .any(|outcome| outcome.provider == provider && outcome.domain == domain);
        if baseline.is_none() && !has_outcomes {
            return None;
        }
        let (created, delivered) = self.outcome_counts(provider, domain);
        let baseline = baseline.cloned().unwrap_or(Baseline {
            provider: provider.to_string(),
            domain: domain.to_string(),
            created: 0,
            delivered: 0,
        });
        Some((baseline.created + created, baseline.delivered + delivered))
    }
}

fn backup_v1(path: &Path, raw: &str) -> Result<(), String> {
    if raw.contains("\"delivered\"") && raw.contains("\"legacy\"") {
        return Ok(());
    }
    let backup = PathBuf::from(format!("{}.v1.bak", path.to_string_lossy()));
    if !backup.exists() {
        persist::write_atomic(&backup, raw.as_bytes())
            .map_err(|err| format!("could not write {}: {err}", backup.display()))?;
    }
    Ok(())
}

impl Drop for Health {
    fn drop(&mut self) {
        self.persist();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Account;

    fn outcome_row(id: &str, domain: &str, created_at: f64, external: bool) -> InboxRow {
        InboxRow {
            id: id.to_string(),
            address: format!("{id}@{domain}"),
            provider: "mail.tm".to_string(),
            created_at,
            expires_at: created_at + 60.0 * 60.0 * 1000.0,
            first_message_at: external.then_some(created_at + 1_000.0),
            first_external_message_at: external.then_some(created_at + 1_000.0),
            first_unclassified_message_at: None,
        }
    }

    fn outcome_account(row: InboxRow) -> Account {
        Account {
            row,
            password: String::new(),
            token: String::new(),
            remote_id: String::new(),
        }
    }

    #[test]
    fn records_and_sorts_provider_domains() {
        let path = std::env::temp_dir().join(format!(
            "gmo-temp-mail-health-{}.json",
            std::process::id()
        ));
        let mut health = Health::load(&path).expect("health");
        health.attempt("mail.tm", "mail.tm");
        health.created("mail.tm", "mail.tm");
        health.external_delivered("mail.tm", "mail.tm");
        health.unclassified_message("mail.tm", "mail.tm");
        health.create_failed("catchmail", "catchmail.io", "blocked");
        health.provider_list_failed("catchmail", "timeout");

        let rows = health.rows();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].provider, "catchmail");
        assert_eq!(rows[0].create_failures, 1);
        assert_eq!(rows[0].list_failures, 0);
        let provider = &health.provider_rows()[0];
        assert_eq!(provider.provider, "catchmail");
        assert_eq!(provider.list_failures, 1);
        assert_eq!(provider.last_error, "timeout");
        assert_eq!(provider.create_failures, 1);
        assert_eq!(provider.last_create_error, "blocked");
        assert_eq!(rows[1].provider, "mail.tm");
        assert_eq!(rows[1].created, 1);
        assert_eq!(rows[1].delivered, 1);
        assert_eq!(rows[1].unclassified, 1);
        assert!(!rows[1].legacy);

        drop(health);
        assert!(path.exists());
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn provider_list_success_clears_the_error_but_keeps_failure_count() {
        let path = std::env::temp_dir().join(format!(
            "gmo-temp-mail-health-list-success-{}.json",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        let mut health = Health::load(&path).expect("health");
        health.provider_list_failed("guerrilla", "parser error one");
        health.provider_list_failed("guerrilla", "parser error two");
        let provider = &health.provider_rows()[0];
        assert_eq!(provider.last_error, "parser error two");
        assert_eq!(provider.list_failures, 2);
        assert!(health.rows().is_empty(), "provider failures are not domain rows");

        health.provider_list_succeeded("guerrilla");
        let provider = &health.provider_rows()[0];
        assert_eq!(provider.last_error, "");
        assert_eq!(provider.list_failures, 2);

        drop(health);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn marks_v1_entries_as_legacy_and_keeps_a_backup() {
        let path = std::env::temp_dir().join(format!(
            "gmo-temp-mail-health-legacy-{}.json",
            std::process::id()
        ));
        let backup = std::path::PathBuf::from(format!("{}.v1.bak", path.to_string_lossy()));
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(&backup);
        std::fs::write(
            &path,
            r#"{"entries":[{"provider":"mail.tm","domain":"mail.tm","attempts":1,"created":0,"create_failures":0,"list_failures":0,"last_attempt_at":0.0,"last_success_at":0.0,"last_error":""}]}"#,
        )
        .unwrap();

        let mut health = Health::load(&path).expect("health");
        let rows = health.rows();
        assert_eq!(rows.len(), 1);
        assert!(rows[0].legacy);
        assert_eq!(rows[0].delivered, 0);
        assert_eq!(rows[0].unclassified, 0);
        assert!(backup.exists());

        health.created("mail.tm", "mail.tm");
        let rows = health.rows();
        assert_eq!(rows.len(), 2, "a new write must not merge into legacy history");
        assert!(rows.iter().any(|row| row.legacy));
        assert!(rows
            .iter()
            .any(|row| !row.legacy && row.created == 1));

        drop(health);
        let _ = std::fs::remove_file(path);
        let _ = std::fs::remove_file(backup);
    }

    #[test]
    fn migrates_historical_domain_list_failures_to_the_provider_once() {
        let path = std::env::temp_dir().join(format!(
            "gmo-temp-mail-health-provider-migration-{}.json",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        std::fs::write(
            &path,
            r#"{"entries":[
                {"provider":"guerrilla","domain":"guerrillamail.com","attempts":1,"created":0,"delivered":0,"unclassified":0,"create_failures":0,"list_failures":2,"last_attempt_at":10.0,"last_success_at":0.0,"last_error":"parser error one"},
                {"provider":"guerrilla","domain":"guerrillamailblock.com","attempts":1,"created":0,"delivered":0,"unclassified":0,"create_failures":0,"list_failures":3,"last_attempt_at":20.0,"last_success_at":0.0,"last_error":"rate limiting"}
            ]}"#,
        )
        .unwrap();

        let mut health = Health::load(&path).expect("health");
        let rows = health.rows();
        assert!(rows.iter().all(|row| row.list_failures == 0));
        assert!(rows.iter().all(|row| row.last_error.is_empty()));
        let provider = &health.provider_rows()[0];
        assert_eq!(provider.provider, "guerrilla");
        assert_eq!(provider.list_failures, 5);
        assert_eq!(provider.last_error, "rate limiting");

        health.list_failed("guerrilla", "guerrillamailblock.com", "mailbox expired");
        drop(health);

        let reloaded = Health::load(&path).expect("health");
        let provider = &reloaded.provider_rows()[0];
        assert_eq!(provider.list_failures, 5, "migration must not run twice");
        let row = reloaded
            .rows()
            .into_iter()
            .find(|row| row.domain == "guerrillamailblock.com" && !row.legacy)
            .expect("domain row");
        assert_eq!(row.list_failures, 1, "later domain errors stay domain-specific");
        assert_eq!(row.last_error, "mailbox expired");

        drop(reloaded);
        let backup = std::path::PathBuf::from(format!("{}.v1.bak", path.to_string_lossy()));
        let _ = std::fs::remove_file(path);
        let _ = std::fs::remove_file(backup);
    }

    #[test]
    fn terminal_outcomes_survive_reload_and_drive_domain_counts() {
        let path = std::env::temp_dir().join(format!(
            "gmo-temp-mail-health-outcomes-{}.json",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        let now = 1_000_000_000.0;
        let domain = "outcomes.test";
        let delivered = outcome_row("delivered", domain, now - 20.0 * 60_000.0, false);
        let quiet = outcome_row("quiet", domain, now - 20.0 * 60_000.0, false);
        let early = outcome_row("early", domain, now - 5.0 * 60_000.0, false);

        let mut health = Health::load(&path).expect("health");
        health.created("mail.tm", domain);
        health.start_outcome(&delivered);
        health.external_delivered("mail.tm", domain);
        health.mark_outcome_delivered(&delivered, now - 19.0 * 60_000.0);
        health.settle_outcome(&delivered, now);

        health.created("mail.tm", domain);
        health.start_outcome(&quiet);
        health.settle_outcome(&quiet, now);

        health.created("mail.tm", domain);
        health.start_outcome(&early);
        health.settle_outcome(&early, now);

        let row = health
            .rows()
            .into_iter()
            .find(|row| row.domain == domain)
            .expect("outcome domain");
        assert_eq!(row.created, 3);
        assert_eq!(row.delivered, 1);
        assert_eq!(
            health.outcome("delivered").map(|outcome| outcome.state),
            Some(OutcomeState::Delivered)
        );
        assert_eq!(
            health.outcome("quiet").map(|outcome| outcome.state),
            Some(OutcomeState::Quiet)
        );
        assert_eq!(
            health.outcome("early").map(|outcome| outcome.state),
            Some(OutcomeState::Unknown)
        );
        drop(health);

        let reloaded = Health::load(&path).expect("health");
        let row = reloaded
            .rows()
            .into_iter()
            .find(|row| row.domain == domain)
            .expect("reloaded outcome domain");
        assert_eq!(row.created, 3);
        assert_eq!(row.delivered, 1);
        assert_eq!(
            reloaded.outcome("quiet").map(|outcome| outcome.state),
            Some(OutcomeState::Quiet)
        );
        assert!(reloaded
            .outcome("early")
            .and_then(|outcome| outcome.settled_at)
            .is_some());
        drop(reloaded);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn reconciliation_backfills_active_inboxes_without_double_counting() {
        let health_path = std::env::temp_dir().join(format!(
            "gmo-temp-mail-health-reconcile-{}.json",
            std::process::id()
        ));
        let store_path = std::env::temp_dir().join(format!(
            "gmo-temp-mail-store-reconcile-{}.json",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&health_path);
        let _ = std::fs::remove_file(&store_path);
        let now = 1_000_000_000.0;
        let domain = "reconcile.test";
        let one = outcome_row("one", domain, now, true);
        let two = outcome_row("two", domain, now, false);
        let mut store = Store::load(&store_path).expect("store");
        store.push(outcome_account(one.clone()));
        store.push(outcome_account(two.clone()));

        let mut health = Health::load(&health_path).expect("health");
        health.created("mail.tm", domain);
        health.created("mail.tm", domain);
        health.external_delivered("mail.tm", domain);
        health.reconcile_with_store(&store);
        let row = health
            .rows()
            .into_iter()
            .find(|row| row.domain == domain)
            .expect("reconciled domain");
        assert_eq!(row.created, 2);
        assert_eq!(row.delivered, 1);
        assert_eq!(
            health.outcome("one").map(|outcome| outcome.state),
            Some(OutcomeState::Delivered)
        );

        let three = outcome_row("three", domain, now, false);
        health.created("mail.tm", domain);
        health.start_outcome(&three);
        let row = health
            .rows()
            .into_iter()
            .find(|row| row.domain == domain)
            .expect("post-reconcile domain");
        assert_eq!(row.created, 3);
        assert_eq!(row.delivered, 1);

        drop(health);
        let _ = std::fs::remove_file(health_path);
        let _ = std::fs::remove_file(store_path);
    }
}
