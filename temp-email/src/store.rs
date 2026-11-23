use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::persist;

const STATE_VERSION: u32 = 1;

#[derive(Deserialize, Serialize, Clone)]
pub struct InboxRow {
    pub id: String,
    pub address: String,
    pub provider: String,
    pub created_at: f64,
    pub expires_at: f64,
    #[serde(default)]
    pub first_message_at: Option<f64>,
    /// First message that is not a known provider welcome/system message.
    #[serde(default)]
    pub first_external_message_at: Option<f64>,
    /// First provider-origin message we could not classify safely.
    #[serde(default)]
    pub first_unclassified_message_at: Option<f64>,
}

/// One inbox on the upstream provider, plus the credentials needed to read it.
#[derive(Serialize, Deserialize, Clone)]
pub struct Account {
    pub row: InboxRow,
    pub password: String,
    pub token: String,
    pub remote_id: String,
}

pub struct Store {
    pub accounts: Vec<Account>,
    path: PathBuf,
    last_error: Option<String>,
}

#[derive(Serialize, Deserialize, Default)]
struct File {
    #[serde(default)]
    version: u32,
    accounts: Vec<Account>,
}

impl Store {
    pub fn load(path: impl Into<PathBuf>) -> Result<Self, String> {
        let path = path.into();
        let (file, warning) = persist::load_json::<File>(&path, STATE_VERSION, |file| file.version)?;
        let mut store = Self {
            accounts: file.map(|file| file.accounts).unwrap_or_default(),
            path,
            last_error: warning,
        };
        // Migrate the on-disk file to the current version even when there are
        // no inboxes yet. This also retains the prior file as the known-good
        // backup on the first start after an upgrade.
        if store.path.exists() {
            store.persist();
        }
        Ok(store)
    }

    pub fn save(&mut self) -> Result<(), String> {
        let file = File {
            version: STATE_VERSION,
            accounts: self.accounts.clone(),
        };
        let raw = serde_json::to_vec_pretty(&file)
            .map_err(|err| format!("could not serialize inbox state: {err}"))?;
        match persist::write_with_backup(&self.path, &raw) {
            Ok(()) => {
                self.last_error = None;
                Ok(())
            }
            Err(err) => {
                let message = format!("could not write inbox state: {err}");
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

    pub fn push(&mut self, account: Account) {
        self.accounts.push(account);
        self.persist();
    }

    pub fn find(&self, id: &str) -> Option<Account> {
        self.accounts.iter().find(|a| a.row.id == id).cloned()
    }

    pub fn remove(&mut self, id: &str) {
        self.accounts.retain(|a| a.row.id != id);
        self.persist();
    }

    pub fn mark_delivered(&mut self, id: &str, at: f64) {
        if let Some(account) = self.accounts.iter_mut().find(|a| a.row.id == id) {
            if account.row.first_message_at.is_none() {
                account.row.first_message_at = Some(at);
                self.persist();
            }
        }
    }

    /// Mark the first external message once. Returns true only for the
    /// transition, so a poll cannot increment the domain count twice.
    pub fn mark_external_delivered(&mut self, id: &str, at: f64) -> bool {
        if let Some(account) = self.accounts.iter_mut().find(|a| a.row.id == id) {
            if account.row.first_external_message_at.is_none() {
                account.row.first_external_message_at = Some(at);
                if account.row.first_message_at.is_none() {
                    account.row.first_message_at = Some(at);
                }
                self.persist();
                return true;
            }
        }
        false
    }

    /// Record the first unclassified provider-origin message once.
    pub fn mark_unclassified(&mut self, id: &str, at: f64) -> bool {
        if let Some(account) = self.accounts.iter_mut().find(|a| a.row.id == id) {
            if account.row.first_unclassified_message_at.is_none() {
                account.row.first_unclassified_message_at = Some(at);
                if account.row.first_message_at.is_none() {
                    account.row.first_message_at = Some(at);
                }
                self.persist();
                return true;
            }
        }
        false
    }

    /// Snapshot expired inboxes without removing them, so callers can persist
    /// the durable outcome before the store row is saved away.
    pub fn expired(&self, now: f64) -> Vec<Account> {
        self.accounts
            .iter()
            .filter(|account| account.row.expires_at <= now)
            .cloned()
            .collect()
    }

    /// Expired inboxes are removed here, not merely hidden. Callers that need
    /// a terminal ledger outcome must settle it before calling this method.
    pub fn purge(&mut self, now: f64) -> Vec<Account> {
        let mut expired = Vec::new();
        let mut kept = Vec::new();
        for account in self.accounts.drain(..) {
            if account.row.expires_at <= now {
                expired.push(account);
            } else {
                kept.push(account);
            }
        }
        self.accounts = kept;
        if !expired.is_empty() {
            self.persist();
        }
        expired
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn account(id: &str, expires_at: f64) -> Account {
        Account {
            row: InboxRow {
                id: id.to_string(),
                address: format!("{id}@example.test"),
                provider: "mail.tm".to_string(),
                created_at: 10.0,
                expires_at,
                first_message_at: None,
                first_external_message_at: None,
                first_unclassified_message_at: None,
            },
            password: "pw".to_string(),
            token: "token".to_string(),
            remote_id: id.to_string(),
        }
    }

    #[test]
    fn stores_finds_marks_and_purges_inboxes() {
        let path = std::env::temp_dir().join(format!(
            "gmo-email-store-{}.json",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        let mut store = Store::load(&path).expect("store");
        store.push(account("one", 1_000.0));
        store.push(account("two", 2_000.0));

        let reloaded = Store::load(&path).expect("store");
        assert_eq!(reloaded.accounts.len(), 2);
        assert_eq!(reloaded.find("one").unwrap().row.address, "one@example.test");

        store.mark_delivered("one", 42.0);
        store.mark_delivered("one", 99.0);
        assert_eq!(store.find("one").unwrap().row.first_message_at, Some(42.0));
        assert!(store.mark_external_delivered("one", 50.0));
        assert!(!store.mark_external_delivered("one", 99.0));
        assert_eq!(
            store.find("one").unwrap().row.first_external_message_at,
            Some(50.0)
        );
        assert_eq!(store.find("one").unwrap().row.first_message_at, Some(42.0));
        assert!(store.mark_unclassified("one", 55.0));
        assert!(!store.mark_unclassified("one", 56.0));
        assert_eq!(
            store.find("one").unwrap().row.first_unclassified_message_at,
            Some(55.0)
        );

        let expired = store.purge(1_500.0);
        assert_eq!(expired.len(), 1);
        assert_eq!(expired[0].row.id, "one");
        assert!(store.find("one").is_none());
        assert!(store.find("two").is_some());
        let reloaded = Store::load(&path).expect("store");
        assert_eq!(reloaded.accounts.len(), 1);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(persist::backup_path(&path));
    }

    #[test]
    fn saves_a_version_and_reports_a_write_failure() {
        let directory = std::env::temp_dir().join(format!(
            "gmo-email-store-version-{}",
            std::process::id()
        ));
        let path = directory.join("state.json");
        let _ = std::fs::remove_dir_all(&directory);
        let mut store = Store::load(&path).expect("store");
        store.push(account("one", 1_000.0));

        let saved: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(saved["version"], 1);

        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();
        assert!(store.save().is_err());
        assert!(store
            .last_error()
            .is_some_and(|error| error.contains("could not write inbox state")));

        let _ = std::fs::remove_dir_all(directory);
    }
}
