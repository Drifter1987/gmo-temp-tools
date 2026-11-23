//! Mail polling that is independent of the visible tab.
//!
//! The desktop window only refreshes the selected inbox when the user asks, so
//! a hidden window would otherwise never learn about a new message. This thread
//! polls every live inbox, feeds the existing delivery ledger, and raises one
//! native notification for each genuinely new provider message id.
//!
//! Inboxes that already existed when the app started are primed on their first
//! successful poll, so their existing mail does not produce a backlog of
//! toasts. An inbox created after startup has no backlog to protect: provider
//! system mail is ignored, and its first real external message is notified.

use crate::{now_ms, providers, store, Shared};
use providers::Brief;
use std::collections::{HashMap, HashSet};
use std::time::Duration;

const POLL_SECONDS: u64 = 20;

/// Per-inbox notification state. `prime_next` contains the inboxes that were
/// present when the poller started and whose first successful list must only
/// populate `known`.
#[derive(Default)]
struct NotificationState {
    prime_next: HashSet<String>,
    known: HashMap<String, HashSet<String>>,
}

impl NotificationState {
    fn for_existing(accounts: &[store::Account]) -> Self {
        let mut state = Self::default();
        for account in accounts {
            state.prime_next.insert(account.row.id.clone());
        }
        state
    }

    fn retain_accounts(&mut self, accounts: &[store::Account]) {
        let live: HashSet<&str> = accounts
            .iter()
            .map(|account| account.row.id.as_str())
            .collect();
        self.prime_next.retain(|id| live.contains(id.as_str()));
        self.known.retain(|id, _| live.contains(id.as_str()));
    }

    fn observe(
        &mut self,
        account: &store::Account,
        messages: &[Brief],
        notify: &mut impl FnMut(&str, &str),
    ) {
        let first_poll = self.prime_next.remove(&account.row.id);
        let known = self.known.entry(account.row.id.clone()).or_default();
        let provider = providers::Provider::from_slug(&account.row.provider);

        for message in messages {
            if !known.insert(message.id.clone()) {
                continue;
            }
            if first_poll {
                continue;
            }
            if provider.is_some_and(|provider| providers::is_system_message(provider, message)) {
                continue;
            }

            let from = message.from.display();
            let subject = if message.subject.trim().is_empty() {
                "(no subject)"
            } else {
                message.subject.trim()
            };
            notify(
                "GMO TempTools · New mail",
                &format!("{from} · {subject}"),
            );
        }
    }
}

pub(crate) fn start(shared: Shared) {
    // Snapshot before spawning so an inbox created just after startup is
    // treated as new (and can notify on its first real message).
    let existing = {
        let state = shared.lock().unwrap();
        state.store.accounts.clone()
    };
    std::thread::spawn(move || {
        let mut notifications = NotificationState::for_existing(&existing);

        loop {
            let accounts = {
                let mut state = shared.lock().unwrap();
                state.purge_expired(now_ms());
                state.store.accounts.clone()
            };
            notifications.retain_accounts(&accounts);

            for account in accounts {
                let messages = match providers::list_with_retry(&account) {
                    Ok(messages) => messages,
                    Err(err) => {
                        let mut state = shared.lock().unwrap();
                        crate::record_list_failure(&mut state, &account, &err);
                        continue;
                    }
                };

                notifications.observe(&account, &messages, &mut |title, body| {
                    if crate::notifications_enabled() {
                        desktop_notifications::notify(title, body);
                    }
                });

                let mut state = shared.lock().unwrap();
                crate::record_list_success(&mut state, &account, &messages);
            }

            std::thread::sleep(Duration::from_secs(POLL_SECONDS));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::Addr;
    use crate::store::{Account, InboxRow};

    fn account(id: &str) -> Account {
        Account {
            row: InboxRow {
                id: id.to_string(),
                address: format!("{id}@guerrillamailblock.com"),
                provider: "guerrilla".to_string(),
                created_at: 1.0,
                expires_at: 2.0,
                first_message_at: None,
                first_external_message_at: None,
                first_unclassified_message_at: None,
            },
            password: String::new(),
            token: String::new(),
            remote_id: String::new(),
        }
    }

    fn message(id: &str, from: &str, subject: &str) -> Brief {
        Brief {
            id: id.to_string(),
            from: Addr {
                address: from.to_string(),
                name: String::new(),
            },
            subject: subject.to_string(),
            intro: String::new(),
            seen: false,
            created_at: String::new(),
            received_at: 1.0,
        }
    }

    fn welcome() -> Brief {
        message(
            "welcome",
            "no-reply@guerrillamail.com",
            "Welcome to Guerrilla Mail",
        )
    }

    fn external(id: &str) -> Brief {
        message(id, "sender@example.test", "Your verification code")
    }

    fn observe(state: &mut NotificationState, account: &Account, messages: &[Brief]) -> Vec<String> {
        let mut notifications = Vec::new();
        state.observe(account, messages, &mut |title, body| {
            notifications.push(format!("{title}: {body}"));
        });
        notifications
    }

    #[test]
    fn existing_inbox_primes_its_first_poll() {
        let account = account("existing");
        let mut state = NotificationState::for_existing(std::slice::from_ref(&account));

        assert!(observe(&mut state, &account, &[external("old"), welcome()]).is_empty());
        assert!(observe(&mut state, &account, &[external("old"), welcome()]).is_empty());
        assert_eq!(observe(&mut state, &account, &[external("new")]).len(), 1);
        assert!(observe(&mut state, &account, &[external("new")]).is_empty());
    }

    #[test]
    fn new_inbox_ignores_welcome_but_notifies_external_mail() {
        let account = account("new");
        let mut state = NotificationState::default();

        let notifications = observe(&mut state, &account, &[welcome(), external("first")]);
        assert_eq!(notifications.len(), 1);
        assert!(notifications[0].contains("sender@example.test"));
        assert!(notifications[0].contains("Your verification code"));
        assert!(observe(&mut state, &account, &[welcome(), external("first")]).is_empty());
    }

    #[test]
    fn welcome_that_arrives_after_an_empty_first_poll_is_ignored() {
        let account = account("delayed-welcome");
        let mut state = NotificationState::default();

        assert!(observe(&mut state, &account, &[]).is_empty());
        assert!(observe(&mut state, &account, &[welcome()]).is_empty());
        assert_eq!(observe(&mut state, &account, &[welcome(), external("later")]).len(), 1);
    }

    #[test]
    fn removed_inboxes_are_pruned() {
        let account = account("removed");
        let mut state = NotificationState::for_existing(std::slice::from_ref(&account));
        observe(&mut state, &account, &[external("one")]);
        assert_eq!(state.known.len(), 1);

        state.retain_accounts(&[]);
        assert!(state.prime_next.is_empty());
        assert!(state.known.is_empty());
    }
}
