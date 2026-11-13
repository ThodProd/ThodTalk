use crate::storage::Store;
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize, Debug, PartialEq)]
pub struct Message {
    #[serde(default = "default_channel")]
    pub channel: String,
    pub id: uuid::Uuid,
    pub author_key: Vec<u8>,
    pub display_name: String,
    pub text: String,
    pub timestamp_ms: u64,
}
fn default_channel() -> String {
    "Общий".into()
}

pub struct History {
    pub messages: Vec<Message>,
    key: String,
}
impl History {
    pub fn open(store: &Store, room: uuid::Uuid) -> Result<Self> {
        let key = format!("history-{room}");
        Ok(Self {
            messages: store.load(&key)?.unwrap_or_default(),
            key,
        })
    }
    pub fn append(&mut self, store: &Store, message: Message) -> Result<bool> {
        ensure!(
            message.text.len() <= 16 * 1024 && !message.text.trim().is_empty(),
            "Сообщение пустое или слишком большое"
        );
        ensure!(
            message.display_name.chars().count() <= 40,
            "Слишком длинное имя"
        );
        if self.messages.iter().any(|m| m.id == message.id) {
            return Ok(false);
        }
        ensure!(
            self.messages.len() < 10_000,
            "Достигнут предел истории: 10 000 сообщений"
        );
        self.messages.push(message);
        if let Err(error) = store.save(&self.key, &self.messages) {
            self.messages.pop();
            return Err(error);
        }
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn history_persists_and_deduplicates() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path().to_owned()).unwrap();
        let room = uuid::Uuid::new_v4();
        let mut history = History::open(&store, room).unwrap();
        let message = Message {
            channel: "Общий".into(),
            id: uuid::Uuid::new_v4(),
            author_key: vec![1; 32],
            display_name: "Друг".into(),
            text: "Привет".into(),
            timestamp_ms: 1,
        };
        assert!(history.append(&store, message.clone()).unwrap());
        assert!(!history.append(&store, message.clone()).unwrap());
        assert_eq!(History::open(&store, room).unwrap().messages, vec![message]);
    }
}
