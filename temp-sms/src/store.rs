use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Deserialize, Serialize, Clone)]
pub struct NumberRow {
    pub id: String,
    pub owner_id: String,
    pub e164: String,
    pub country: String,
    pub provider: String,
    pub created_at: f64,
    pub expires_at: f64,
}

#[derive(Deserialize, Serialize, Clone)]
pub struct SmsRow {
    pub id: String,
    /// The carrier's own message id, used to ignore retried deliveries.
    #[serde(default)]
    pub sid: String,
    pub number_id: String,
    pub sender: String,
    pub body: String,
    pub code: String,
    pub received_at: f64,
    pub seen: f64,
}

/// A card, plus the country slug needed to re-read that number's page.
#[derive(Serialize, Deserialize, Clone)]
pub struct Card {
    pub row: NumberRow,
    pub slug: String,
}

pub struct Store {
    pub cards: Vec<Card>,
    pub seen: Vec<String>,
    /// Messages delivered by a signed webhook, kept locally so the UI can show
    /// them next to anything read from the public page.
    pub inbound: Vec<SmsRow>,
    path: PathBuf,
}

#[derive(Serialize, Deserialize, Default)]
struct File {
    cards: Vec<Card>,
    #[serde(default)]
    seen: Vec<String>,
    #[serde(default)]
    inbound: Vec<SmsRow>,
}

impl Store {
    pub fn load(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let file = std::fs::read_to_string(&path)
            .ok()
            .and_then(|raw| serde_json::from_str::<File>(&raw).ok())
            .unwrap_or_default();
        Self {
            cards: file.cards,
            seen: file.seen,
            inbound: file.inbound,
            path,
        }
    }

    pub fn save(&self) {
        if let Some(dir) = Path::new(&self.path).parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let file = File {
            cards: self.cards.clone(),
            seen: self.seen.clone(),
            inbound: self.inbound.clone(),
        };
        if let Ok(raw) = serde_json::to_string_pretty(&file) {
            let _ = std::fs::write(&self.path, raw);
        }
    }

    pub fn push(&mut self, card: Card) {
        self.cards.push(card);
        self.save();
    }

    /// Keep one card per number: adding a number already on the list returns
    /// the existing card instead of appending a duplicate.
    pub fn push_unique(&mut self, card: Card) -> Card {
        let digits: String = card
            .row
            .e164
            .chars()
            .filter(|ch| ch.is_ascii_digit())
            .collect();
        if let Some(existing) = self.find_by_digits(&digits) {
            return existing;
        }
        self.push(card.clone());
        card
    }

    pub fn find(&self, id: &str) -> Option<Card> {
        self.cards.iter().find(|c| c.row.id == id).cloned()
    }

    pub fn find_by_digits(&self, digits: &str) -> Option<Card> {
        self.cards.iter().find(|c| {
            c.row
                .e164
                .chars()
                .filter(|ch| ch.is_ascii_digit())
                .eq(digits.chars())
        }).cloned()
    }

    /// Store one webhook message. Returns false when the provider retried an id
    /// we already hold, so the caller can answer 200 without duplicating it.
    /// De-duplication reads the messages themselves, so removing a card also
    /// forgets its ids and a later delivery of that message is stored again.
    pub fn push_message(&mut self, row: SmsRow) -> bool {
        if !row.sid.is_empty() && self.inbound.iter().any(|held| held.sid == row.sid) {
            return false;
        }
        self.inbound.push(row);
        self.save();
        true
    }

    pub fn messages_for(&self, card_id: &str) -> Vec<SmsRow> {
        self.inbound
            .iter()
            .filter(|row| row.number_id == card_id)
            .cloned()
            .collect()
    }

    pub fn find_message(&self, id: &str) -> Option<SmsRow> {
        self.inbound.iter().find(|row| row.id == id).cloned()
    }

    pub fn remove(&mut self, id: &str) {
        self.cards.retain(|c| c.row.id != id);
        self.inbound.retain(|row| row.number_id != id);
        self.save();
    }

    pub fn mark_seen(&mut self, id: &str) {
        if !self.seen.iter().any(|s| s == id) {
            self.seen.push(id.to_string());
            self.save();
        }
    }

    /// Expiry deletes the card and its messages; nothing is left behind.
    pub fn purge(&mut self, now: f64) -> usize {
        let before = self.cards.len();
        self.cards.retain(|c| c.row.expires_at > now);
        let removed = before - self.cards.len();
        let alive: Vec<String> = self.cards.iter().map(|c| c.row.id.clone()).collect();
        let kept = self.inbound.len();
        self.inbound.retain(|row| alive.contains(&row.number_id));
        if removed > 0 || kept != self.inbound.len() {
            self.save();
        }
        removed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn card(id: &str) -> Card {
        Card {
            row: NumberRow {
                id: id.to_string(),
                owner_id: "local".to_string(),
                e164: "+15550001111".to_string(),
                country: "us".to_string(),
                provider: "receive-sms.io".to_string(),
                created_at: 0.0,
                expires_at: 1.0,
            },
            slug: "us".to_string(),
        }
    }

    #[test]
    fn adding_the_same_number_reuses_the_existing_card() {
        let path = std::env::temp_dir()
            .join(format!("gmo-sms-dedupe-{}.json", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let mut store = Store::load(&path);
        let first = store.push_unique(card("first"));
        let second = store.push_unique(card("second"));
        assert_eq!(first.row.id, "first");
        assert_eq!(second.row.id, "first");
        assert_eq!(store.cards.len(), 1);
        let _ = std::fs::remove_file(path);
    }

    fn sms(id: &str, number_id: &str, sid: &str) -> SmsRow {
        SmsRow {
            id: id.to_string(),
            sid: sid.to_string(),
            number_id: number_id.to_string(),
            sender: "+15550001111".to_string(),
            body: "Your code is 771245".to_string(),
            code: "771245".to_string(),
            received_at: 10.0,
            seen: 0.0,
        }
    }

    #[test]
    fn expiry_and_removal_prune_messages_and_retries() {
        let path = std::env::temp_dir()
            .join(format!("gmo-sms-prune-{}.json", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let mut store = Store::load(&path);
        store.push(card("n1"));

        assert!(store.push_message(sms("m1", "n1", "SM1")));
        assert!(!store.push_message(sms("m1-retry", "n1", "SM1")));
        assert_eq!(store.messages_for("n1").len(), 1);

        assert_eq!(store.purge(2.0), 1);
        assert!(store.inbound.is_empty());

        store.push(card("n2"));
        assert!(store.push_message(sms("m2", "n2", "SM2")));
        store.remove("n2");
        assert!(store.inbound.is_empty());
        let _ = std::fs::remove_file(path);
    }
}
