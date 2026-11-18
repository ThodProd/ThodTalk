use crate::storage::Store;
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Clone, Serialize, Deserialize, Debug, PartialEq, Hash)]
pub struct FileMeta {
    pub name: String,
    pub size: u64,
}

#[derive(Clone, Serialize, Deserialize, Debug, PartialEq)]
pub struct Message {
    #[serde(default = "default_channel")]
    pub channel: String,
    pub id: uuid::Uuid,
    pub author_key: Vec<u8>,
    pub display_name: String,
    pub text: String,
    pub timestamp_ms: u64,
    #[serde(default)]
    pub image: Option<uuid::Uuid>,
    /// Any non-image attachment. Stored under the same key as `image`.
    #[serde(default)]
    pub file: Option<FileMeta>,
    #[serde(default)]
    pub edited: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_to: Option<uuid::Uuid>,
}
fn default_channel() -> String {
    "Общий".into()
}

pub struct History {
    pub messages: Vec<Message>,
    key: String,
    revision: u64,
}
impl History {
    pub fn saved_rooms(store: &Store) -> Result<Vec<uuid::Uuid>> {
        let mut rooms = Vec::new();
        for entry in std::fs::read_dir(&store.root)? {
            let entry = entry?;
            if let Some(room) = entry
                .file_name()
                .to_str()
                .and_then(|name| name.strip_prefix("history-"))
                .and_then(|name| name.strip_suffix(".bin"))
                .and_then(|name| uuid::Uuid::parse_str(name).ok())
                .filter(|id| entry.file_name() == format!("history-{id}.bin").as_str())
            {
                rooms.push(room);
            }
        }
        Ok(rooms)
    }

    /// Archive cleanup runs with the session worker stopped. A file referenced
    /// by another room remains available in that room.
    pub fn clear_saved(store: &Store, room: Option<uuid::Uuid>) -> Result<()> {
        let mut candidates = BTreeSet::new();
        let mut retained = BTreeSet::new();
        let mut all_retained_read = true;
        let mut failure = None;
        for id in Self::saved_rooms(store)? {
            let history = Self::open(store, id);
            let clear = room.is_none_or(|room| room == id);
            let removed = clear
                && match std::fs::remove_file(store.root.join(format!("history-{id}.bin"))) {
                    Ok(()) => true,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => true,
                    Err(error) => {
                        failure.get_or_insert(error);
                        false
                    }
                };
            match history {
                Ok(history) => {
                    let references = history.messages.iter().filter_map(|message| {
                        message
                            .image
                            .or_else(|| message.file.as_ref().map(|_| message.id))
                    });
                    if removed {
                        candidates.extend(references);
                    } else {
                        retained.extend(references);
                    }
                }
                Err(_) if !removed => all_retained_read = false,
                Err(_) => {}
            }
        }
        if all_retained_read {
            if room.is_none() && failure.is_none() {
                for entry in std::fs::read_dir(&store.root)? {
                    let entry = entry?;
                    if let Some(id) = entry
                        .file_name()
                        .to_str()
                        .and_then(|name| name.strip_prefix("image-"))
                        .and_then(|name| name.strip_suffix(".bin"))
                        .and_then(|name| uuid::Uuid::parse_str(name).ok())
                    {
                        candidates.insert(id);
                    }
                }
            }
            for &id in candidates.difference(&retained) {
                let path = store.root.join(format!("image-{id}.bin"));
                if let Err(error) = std::fs::remove_file(path)
                    && error.kind() != std::io::ErrorKind::NotFound
                {
                    failure.get_or_insert(error);
                }
            }
        }
        match failure {
            Some(error) => Err(error.into()),
            None => Ok(()),
        }
    }

    pub fn open(store: &Store, room: uuid::Uuid) -> Result<Self> {
        let key = format!("history-{room}");
        Ok(Self {
            messages: store.load(&key)?.unwrap_or_default(),
            key,
            revision: 0,
        })
    }
    pub fn revision(&self) -> u64 {
        self.revision
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
        self.revision = self.revision.wrapping_add(1);
        Ok(true)
    }
    /// Change the text of a message owned by `author`.
    pub fn edit(
        &mut self,
        store: &Store,
        id: uuid::Uuid,
        author: &[u8],
        text: &str,
    ) -> Result<bool> {
        ensure!(
            text.len() <= 16 * 1024 && !text.trim().is_empty(),
            "Сообщение пустое или слишком большое"
        );
        let Some(index) = self.messages.iter().position(|m| m.id == id) else {
            return Ok(false);
        };
        ensure!(
            self.messages[index].author_key == author,
            "Можно редактировать только свои сообщения"
        );
        if self.messages[index].image.is_some() || self.messages[index].file.is_some() {
            return Ok(false);
        }
        let previous = self.messages[index].text.clone();
        let was_edited = self.messages[index].edited;
        self.messages[index].text = text.into();
        self.messages[index].edited = true;
        if let Err(error) = store.save(&self.key, &self.messages) {
            self.messages[index].text = previous;
            self.messages[index].edited = was_edited;
            return Err(error);
        }
        self.revision = self.revision.wrapping_add(1);
        Ok(true)
    }
    /// Remove a message from the local history.
    pub fn remove(&mut self, store: &Store, id: uuid::Uuid) -> Result<bool> {
        let Some(index) = self.messages.iter().position(|m| m.id == id) else {
            return Ok(false);
        };
        let removed = self.messages.remove(index);
        if let Err(error) = store.save(&self.key, &self.messages) {
            self.messages.insert(index, removed);
            return Err(error);
        }
        self.revision = self.revision.wrapping_add(1);
        Ok(true)
    }
    pub fn author(&self, id: uuid::Uuid) -> Option<&[u8]> {
        self.messages
            .iter()
            .find(|m| m.id == id)
            .map(|m| m.author_key.as_slice())
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
            image: None,
            file: None,
            edited: false,
            reply_to: None,
        };
        assert!(history.append(&store, message.clone()).unwrap());
        assert!(!history.append(&store, message.clone()).unwrap());
        assert_eq!(History::open(&store, room).unwrap().messages, vec![message]);
    }

    #[test]
    fn clearing_archive_preserves_other_rooms_and_profile() {
        let temp = tempfile::tempdir().unwrap();
        let store = Store::open(temp.path().to_owned()).unwrap();
        let first = uuid::Uuid::new_v4();
        let second = uuid::Uuid::new_v4();
        let shared = uuid::Uuid::new_v4();
        let unique = uuid::Uuid::new_v4();
        store.save("profile", &"test profile").unwrap();
        for id in [shared, unique] {
            crate::attachments::save_bytes(&store, id, b"attachment").unwrap();
        }
        for (room, id) in [(first, shared), (first, unique), (second, shared)] {
            History::open(&store, room)
                .unwrap()
                .append(
                    &store,
                    Message {
                        id,
                        channel: "Общий".into(),
                        author_key: vec![1; 32],
                        display_name: "Друг".into(),
                        text: "Вложение".into(),
                        timestamp_ms: 1,
                        image: Some(id),
                        file: None,
                        edited: false,
                        reply_to: None,
                    },
                )
                .unwrap();
        }
        History::clear_saved(&store, Some(first)).unwrap();
        assert_eq!(History::saved_rooms(&store).unwrap(), vec![second]);
        assert!(
            crate::attachments::load_bytes(&store, shared)
                .unwrap()
                .is_some()
        );
        assert!(
            crate::attachments::load_bytes(&store, unique)
                .unwrap()
                .is_none()
        );
        History::clear_saved(&store, None).unwrap();
        assert!(History::saved_rooms(&store).unwrap().is_empty());
        assert!(
            crate::attachments::load_bytes(&store, shared)
                .unwrap()
                .is_none()
        );
        assert_eq!(
            store.load::<String>("profile").unwrap().as_deref(),
            Some("test profile")
        );
    }
}
