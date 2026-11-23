//! SMS polling and webhook observation that survive the window being hidden.
//!
//! Signed webhook deliveries add rows to `store.inbound`; public page messages
//! live in the scrape cache. A fast observer notices webhook rows, and a slower
//! pass refreshes every live number page. Both paths share one key set, so the
//! same text cannot be notified twice. Existing messages are primed on the
//! first observation instead of producing a backlog of toasts.

use crate::{message_url, now_ms, scrape, store, Shared};
use std::collections::HashSet;
use std::time::{Duration, Instant};

const OBSERVE_SECONDS: u64 = 1;
const SCRAPE_SECONDS: u64 = 20;

fn key(card_id: &str, sender: &str, body: &str, received_at: f64) -> String {
    format!("{card_id}\u{1f}{sender}\u{1f}{:.0}\u{1f}{body}", received_at)
}

pub(crate) fn start(shared: Shared) {
    std::thread::spawn(move || {
        let mut known: HashSet<String> = HashSet::new();
        let mut primed_cards: HashSet<String> = HashSet::new();
        let mut primed_inbound = false;
        let mut next_scrape = Instant::now();

        loop {
            let cards = {
                let mut state = shared.lock().unwrap();
                state.store.purge(crate::now_ms());
                state.store.cards.clone()
            };

            let inbound: Vec<(store::Card, store::SmsRow)> = {
                let state = shared.lock().unwrap();
                cards
                    .iter()
                    .flat_map(|card| {
                        state
                            .store
                            .messages_for(&card.row.id)
                            .into_iter()
                            .map(move |row| (card.clone(), row))
                    })
                    .collect()
            };
            for (card, row) in inbound {
                let message_key = key(&card.row.id, &row.sender, &row.body, row.received_at);
                let is_new = known.insert(message_key);
                if is_new && primed_inbound && crate::notifications_enabled() {
                    desktop_notifications::notify(
                        &format!("GMO TempTools · New text on {}", card.row.e164),
                        &format!("{}: {}", row.sender, row.body),
                    );
                }
            }
            primed_inbound = true;

            if Instant::now() >= next_scrape {
                for card in &cards {
                    let first_scrape = !primed_cards.contains(&card.row.id);
                    // Fetch before taking the shared lock: a slow public page
                    // must not hold up webhook storage or the local API.
                    let messages = if card.slug.is_empty() {
                        Vec::new()
                    } else {
                        {
                            let mut state = shared.lock().unwrap();
                            state.health.source.attempt();
                        }
                        let (html, retries) = match crate::fetch_with_retry(&message_url(card)) {
                            Ok(result) => result,
                            Err(error) => {
                                let mut state = shared.lock().unwrap();
                                state.health.source.failed(&error);
                                continue;
                            }
                        };
                        {
                            let mut state = shared.lock().unwrap();
                            state.health.source.succeeded(retries);
                        }
                        scrape::messages(&html)
                    };
                    {
                        let mut state = shared.lock().unwrap();
                        state
                            .cache
                            .insert(card.row.id.clone(), (now_ms(), messages.clone()));
                    }
                    for message in &messages {
                        let message_key = key(
                            &card.row.id,
                            &message.sender,
                            &message.body,
                            message.received_at,
                        );
                        let is_new = known.insert(message_key);
                        if is_new && !first_scrape && crate::notifications_enabled() {
                            desktop_notifications::notify(
                                &format!("GMO TempTools · New text on {}", card.row.e164),
                                &format!("{}: {}", message.sender, message.body),
                            );
                        }
                    }
                    primed_cards.insert(card.row.id.clone());
                }
                next_scrape = Instant::now() + Duration::from_secs(SCRAPE_SECONDS);
            }

            std::thread::sleep(Duration::from_secs(OBSERVE_SECONDS));
        }
    });
}
