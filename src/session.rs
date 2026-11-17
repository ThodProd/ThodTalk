use crate::{
    crypto::Identity,
    history::{FileMeta, History, Message},
    internet::{InternetGuest, InternetHost, PairEvent},
    storage::{Profile, Store},
    transport::Connection,
    wire::Packet,
};
use anyhow::{Context, Result, ensure};
use base64::{Engine, engine::general_purpose::STANDARD};
use openmls::prelude::*;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    sync::mpsc,
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(Serialize, Deserialize)]
pub enum Payload {
    Heartbeat {
        nonce: u64,
    },
    Probe {
        nonce: u64,
    },
    Profile {
        name: String,
        avatar: String,
    },
    Image {
        id: uuid::Uuid,
        channel: String,
        #[serde(with = "crate::wire::binary")]
        data: Vec<u8>,
        time: u64,
        name: String,
    },
    Chat {
        id: uuid::Uuid,
        channel: String,
        text: String,
        time: u64,
        name: String,
    },
    ChatEdit {
        id: uuid::Uuid,
        text: String,
    },
    ChatDelete {
        id: uuid::Uuid,
    },
    VoiceFrame {
        channel: String,
        #[serde(with = "crate::wire::binary")]
        data: Vec<u8>,
        sequence: u64,
    },
    Voice {
        channel: String,
        data: Vec<u8>,
    },
    VoiceState {
        channel: Option<String>,
        muted: bool,
        #[serde(default)]
        deafened: bool,
    },
    Screen {
        channel: String,
        width: u32,
        height: u32,
        #[serde(with = "crate::wire::binary")]
        data: Vec<u8>,
    },
    /// Announced when someone starts or stops sharing a screen in a voice channel.
    ScreenState {
        channel: Option<String>,
    },
    /// Sent by a viewer to the owner of the stream they joined or left.
    ScreenWatch {
        target: Vec<u8>,
        channel: String,
        watching: bool,
    },
    FileStart {
        id: uuid::Uuid,
        channel: String,
        file_name: String,
        size: u64,
        time: u64,
        name: String,
    },
    FileChunk {
        id: uuid::Uuid,
        #[serde(with = "crate::wire::binary")]
        data: Vec<u8>,
    },
}
#[derive(Clone, Serialize, Deserialize, Hash)]
pub struct Member {
    pub name: String,
    pub avatar: Vec<u8>,
    pub voice: Option<String>,
    pub muted: bool,
    #[serde(default)]
    pub deafened: bool,
    /// Voice channel in which this member is currently sharing their screen.
    #[serde(default)]
    pub screen: Option<String>,
}
const FILE_CHUNK: usize = 150 * 1024;
struct PendingFile {
    id: uuid::Uuid,
    channel: String,
    file_name: String,
    data: Vec<u8>,
    offset: usize,
}
struct IncomingFile {
    author: Vec<u8>,
    name: String,
    channel: String,
    file_name: String,
    size: usize,
    time: u64,
    data: Vec<u8>,
    last: std::time::Instant,
}
enum Link {
    Host(InternetHost),
    Guest(InternetGuest),
}
pub struct Session {
    link: Link,
    identity: Identity,
    group: Option<MlsGroup>,
    store: Store,
    owner: Vec<u8>,
    pub room: uuid::Uuid,
    pub members: BTreeMap<Vec<u8>, Member>,
    pub history: History,
    pub status: String,
    pub invite: Option<mpsc::Receiver<PairEvent>>,
    pub code: String,
    pub closed: bool,
    pub profile: Profile,
    pub voice_in: Vec<(Vec<u8>, String, Vec<u8>)>,
    /// Latest screen frame: author, channel, width, height, JPEG bytes.
    pub screen_in: Option<(Vec<u8>, String, u32, u32, Vec<u8>)>,
    /// Viewer join/leave notices addressed to this client's own stream.
    pub screen_watch_in: Vec<(Vec<u8>, String, bool)>,
    screen_rate: BTreeMap<Vec<u8>, std::time::Instant>,
    last_screen_announce: std::time::Instant,
    file_out: Option<PendingFile>,
    file_queue: std::collections::VecDeque<PendingFile>,
    files_in: BTreeMap<uuid::Uuid, IncomingFile>,
    last_file_sweep: std::time::Instant,
    connections: BTreeMap<u64, LeafNodeIndex>,
    banned: std::collections::BTreeSet<Vec<u8>>,
    last_owner: std::time::Instant,
    last_probe: std::time::Instant,
    probe_sent: Option<std::time::Instant>,
    pub ping_ms: Option<u32>,
    nonce: u64,
    confirmed: bool,
    voice_sequence: u64,
    received_voice: BTreeMap<Vec<u8>, u64>,
}
impl Session {
    pub fn host(store: Store, profile: Profile) -> Result<Self> {
        Self::host_with(store, profile, InternetHost::start)
    }
    fn host_with(
        store: Store,
        profile: Profile,
        start: impl FnOnce() -> Result<InternetHost>,
    ) -> Result<Self> {
        ensure!(
            profile.network_allowed,
            "Разрешите подключение к сети в профиле"
        );
        let identity = Identity::load_for_session(&store, profile.id.as_bytes())?;
        let room = store
            .load::<uuid::Uuid>("owned-room")?
            .unwrap_or_else(uuid::Uuid::new_v4);
        store.save("owned-room", &room)?;
        let group = identity.create_group()?;
        identity.save(&store)?;
        let link = Link::Host(start()?);
        let history = History::open(&store, room)?;
        let owner = identity.public_key();
        let mut result = Self {
            link,
            identity,
            group: Some(group),
            store,
            owner,
            room,
            members: BTreeMap::new(),
            history,
            status: "Проверяем маршрут для подключения друзей…".into(),
            invite: None,
            code: String::new(),
            closed: false,
            profile,
            voice_in: Vec::new(),
            screen_in: None,
            screen_watch_in: Vec::new(),
            screen_rate: BTreeMap::new(),
            last_screen_announce: std::time::Instant::now(),
            file_out: None,
            file_queue: std::collections::VecDeque::new(),
            files_in: BTreeMap::new(),
            last_file_sweep: std::time::Instant::now(),
            connections: BTreeMap::new(),
            banned: Default::default(),
            last_owner: std::time::Instant::now(),
            last_probe: std::time::Instant::now(),
            probe_sent: None,
            ping_ms: Some(0),
            nonce: u64::from_le_bytes(uuid::Uuid::new_v4().as_bytes()[..8].try_into().unwrap()),
            confirmed: false,
            voice_sequence: 0,
            received_voice: BTreeMap::new(),
        };
        result.update_own_member();
        result.new_invite();
        Ok(result)
    }
    pub fn join(store: Store, profile: Profile, code: &str) -> Result<Self> {
        ensure!(
            profile.network_allowed,
            "Разрешите подключение к сети в профиле"
        );
        Self::from_guest(store, profile, InternetGuest::join(code)?)
    }
    fn from_guest(store: Store, profile: Profile, guest: InternetGuest) -> Result<Self> {
        let identity = Identity::load_for_session(&store, profile.id.as_bytes())?;
        let room = guest.invitation.room;
        let owner = guest.invitation.owner_key.clone();
        let package = identity.key_package()?;
        identity.save(&store)?;
        guest.connection.send(Packet::Join { package })?;
        let history = History::open(&store, room)?;
        let mut result = Self {
            link: Link::Guest(guest),
            identity,
            group: None,
            store,
            owner,
            room,
            members: BTreeMap::new(),
            history,
            status: "Входим в защищённую группу…".into(),
            invite: None,
            code: String::new(),
            closed: false,
            profile,
            voice_in: Vec::new(),
            screen_in: None,
            screen_watch_in: Vec::new(),
            screen_rate: BTreeMap::new(),
            last_screen_announce: std::time::Instant::now(),
            file_out: None,
            file_queue: std::collections::VecDeque::new(),
            files_in: BTreeMap::new(),
            last_file_sweep: std::time::Instant::now(),
            connections: BTreeMap::new(),
            banned: Default::default(),
            last_owner: std::time::Instant::now(),
            last_probe: std::time::Instant::now(),
            probe_sent: None,
            ping_ms: None,
            nonce: u64::from_le_bytes(uuid::Uuid::new_v4().as_bytes()[..8].try_into().unwrap()),
            confirmed: false,
            voice_sequence: 0,
            received_voice: BTreeMap::new(),
        };
        result.update_own_member();
        Ok(result)
    }
    fn connection(&self) -> &Connection {
        match &self.link {
            Link::Host(host) => &host.relay.owner,
            Link::Guest(guest) => &guest.connection,
        }
    }
    pub fn hosting(&self) -> bool {
        matches!(self.link, Link::Host(_))
    }
    pub fn ready(&self) -> bool {
        self.group.is_some() && (self.hosting() || self.confirmed)
    }
    pub fn own_key(&self) -> Vec<u8> {
        self.identity.public_key()
    }
    pub fn set_closed(&mut self, closed: bool) {
        if let Link::Host(host) = &self.link {
            host.set_closed(closed);
            self.closed = closed;
            self.status = if closed {
                "Вход закрыт. Текущие участники остаются подключены"
            } else {
                "Вход открыт"
            }
            .into();
        }
    }
    /// Graceful server shutdown: notify all members and close
    pub fn shutdown(&mut self) -> Result<()> {
        ensure!(self.hosting(), "Только хозяин может выключить сервер");
        self.set_closed(true);
        // Send goodbye message to all members
        let keys_to_kick: Vec<_> = self
            .members
            .keys()
            .filter(|k| *k != &self.owner)
            .cloned()
            .collect();
        for key in keys_to_kick {
            let _ = self.kick(&key);
        }
        self.status = "Сервер завершает работу".into();
        Ok(())
    }
    pub fn ban(&mut self, key: &[u8]) -> Result<()> {
        ensure!(
            self.hosting() && key != self.owner,
            "Только владелец может заблокировать участника"
        );
        self.banned.insert(key.to_vec());
        self.kick(key)
    }
    pub fn kick(&mut self, key: &[u8]) -> Result<()> {
        ensure!(self.hosting(), "Исключать участников может только владелец");
        ensure!(key != self.owner, "Нельзя исключить владельца");
        let group = self.group.as_mut().context("Нет группы")?;
        let member = group
            .members()
            .find(|m| m.signature_key == key)
            .context("Участник уже вышел")?;
        let connection = self
            .connections
            .iter()
            .find_map(|(id, index)| (*index == member.index).then_some(*id))
            .context("Соединение не найдено")?;
        let (commit, _, _) = group.remove_members(
            &self.identity.provider,
            &self.identity.signer,
            &[member.index],
        )?;
        group.merge_pending_commit(&self.identity.provider)?;
        use ::tls_codec::Serialize;
        let data = commit.tls_serialize_detached()?;
        self.identity.save(&self.store)?;
        self.connection().send(Packet::Disconnect { connection })?;
        self.connection().send(Packet::Commit { data })?;
        self.connections.remove(&connection);
        self.members.remove(key);
        self.received_voice.remove(key);
        self.screen_rate.remove(key);
        Ok(())
    }
    pub fn new_invite(&mut self) {
        if self.invite.is_none() {
            if let Link::Host(host) = &self.link {
                self.code.clear();
                self.invite = Some(host.invite(self.owner.clone(), self.room));
            }
        }
    }
    fn update_own_member(&mut self) {
        let key = self.own_key();
        let member = self.members.entry(key).or_insert(Member {
            name: String::new(),
            avatar: Vec::new(),
            voice: None,
            muted: false,
            deafened: false,
            screen: None,
        });
        member.name = self.profile.name.clone();
        member.avatar = self.profile.avatar.clone();
    }
    pub fn update_profile(&mut self, profile: Profile) -> Result<()> {
        self.profile = profile;
        self.update_own_member();
        self.send_profile()
    }
    fn send_profile(&mut self) -> Result<()> {
        self.send(Payload::Profile {
            name: self.profile.name.clone(),
            avatar: STANDARD.encode(&self.profile.avatar),
        })?;
        let member = self.members.get(&self.own_key()).cloned();
        if let Some(member) = member {
            self.send(Payload::VoiceState {
                channel: member.voice,
                muted: member.muted,
                deafened: member.deafened,
            })?;
        }
        Ok(())
    }
    pub fn send(&mut self, payload: Payload) -> Result<()> {
        let payload = match payload {
            Payload::Voice { channel, data } => {
                self.voice_sequence += 1;
                Payload::VoiceFrame {
                    channel,
                    data,
                    sequence: self.voice_sequence,
                }
            }
            p => p,
        };
        let group = self.group.as_mut().context("Группа ещё не готова")?;
        let data = self
            .identity
            .encrypt(group, &serde_json::to_vec(&payload)?)?;
        // Persist the sending ratchet before ciphertext becomes visible to the network.
        if !matches!(
            payload,
            Payload::VoiceFrame { .. }
                | Payload::Heartbeat { .. }
                | Payload::Probe { .. }
                | Payload::Screen { .. }
                | Payload::FileChunk { .. }
        ) {
            self.identity.save(&self.store)?;
        }
        self.connection()
            .send(if matches!(payload, Payload::VoiceFrame { .. }) {
                Packet::Voice { data }
            } else {
                Packet::Content { data }
            })?;
        if let Payload::VoiceState {
            channel,
            muted,
            deafened,
        } = payload
        {
            if let Some(member) = self.members.get_mut(&self.identity.public_key()) {
                member.voice = channel;
                member.muted = muted;
                member.deafened = deafened;
            }
        }
        Ok(())
    }
    pub fn image(&mut self, channel: &str, path: &std::path::Path) -> Result<()> {
        let data = crate::attachments::prepare(path)?;
        let id = uuid::Uuid::new_v4();
        let time = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis() as u64;
        crate::attachments::save(&self.store, id, &data)?;
        self.send(Payload::Image {
            id,
            channel: channel.into(),
            data,
            time,
            name: self.profile.name.clone(),
        })?;
        self.history.append(
            &self.store,
            Message {
                id,
                channel: channel.into(),
                author_key: self.own_key(),
                display_name: self.profile.name.clone(),
                text: "Изображение".into(),
                timestamp_ms: time,
                image: Some(id),
                edited: false,
                file: None,
            },
        )?;
        Ok(())
    }
    pub fn screen(&mut self, channel: &str, width: u32, height: u32, data: Vec<u8>) -> Result<()> {
        ensure!(valid_channel(channel), "Неизвестный голосовой канал");
        ensure!(
            (2..=4096).contains(&width) && (2..=4096).contains(&height),
            "Некорректный размер кадра"
        );
        ensure!(
            !data.is_empty() && data.len() <= 170 * 1024,
            "Кадр экрана слишком большой"
        );
        self.send(Payload::Screen {
            channel: channel.into(),
            width,
            height,
            data,
        })
    }
    /// Announce that the local user started or stopped sharing. This is what
    /// makes the stream clickable for everyone else in the voice channel.
    pub fn set_screen(&mut self, channel: Option<&str>) -> Result<()> {
        if let Some(channel) = channel {
            ensure!(valid_channel(channel), "Неизвестный голосовой канал");
        }
        let own = self.own_key();
        if let Some(member) = self.members.get_mut(&own) {
            member.screen = channel.map(str::to_string);
        }
        self.send(Payload::ScreenState {
            channel: channel.map(str::to_string),
        })?;
        self.last_screen_announce = std::time::Instant::now();
        Ok(())
    }
    /// Share a file of any extension. Chunks are streamed by `pump_file` so a
    /// large transfer never blocks the network loop or voice packets.
    pub fn file(&mut self, channel: &str, path: &std::path::Path) -> Result<()> {
        ensure!(valid_channel(channel), "Неизвестный канал");
        let (data, file_name, size) = crate::attachments::read_file(path)?;
        ensure!(
            self.file_queue.len() < 5,
            "Слишком много файлов в очереди — дождитесь отправки"
        );
        let id = uuid::Uuid::new_v4();
        crate::attachments::save_bytes(&self.store, id, &data)?;
        let time = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis() as u64;
        self.history.append(
            &self.store,
            Message {
                id,
                channel: channel.into(),
                author_key: self.own_key(),
                display_name: self.profile.name.clone(),
                text: "Файл".into(),
                timestamp_ms: time,
                image: None,
                file: Some(FileMeta {
                    name: file_name.clone(),
                    size,
                }),
                edited: false,
            },
        )?;
        self.file_queue.push_back(PendingFile {
            id,
            channel: channel.into(),
            file_name,
            data,
            offset: 0,
        });
        Ok(())
    }
    fn pump_file(&mut self) {
        if self.file_out.is_none() {
            let Some(file) = self.file_queue.pop_front() else {
                return;
            };
            let payload = Payload::FileStart {
                id: file.id,
                channel: file.channel.clone(),
                file_name: file.file_name.clone(),
                size: file.data.len() as u64,
                time: SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.as_millis() as u64)
                    .unwrap_or(0),
                name: self.profile.name.clone(),
            };
            if self.send(payload).is_ok() {
                self.file_out = Some(file);
            } else {
                self.file_queue.push_front(file);
                return;
            }
        }
        let Some(file) = &self.file_out else {
            return;
        };
        let end = (file.offset + FILE_CHUNK).min(file.data.len());
        let (id, chunk, done) = (
            file.id,
            file.data[file.offset..end].to_vec(),
            end == file.data.len(),
        );
        if self.send(Payload::FileChunk { id, data: chunk }).is_ok() {
            if let Some(file) = &mut self.file_out {
                file.offset = end;
                if done {
                    self.file_out = None;
                }
            }
        }
    }
    pub fn chat(&mut self, channel: &str, text: &str) -> Result<()> {
        ensure!(
            text.len() <= 16 * 1024 && !text.trim().is_empty(),
            "Сообщение пустое или слишком большое"
        );
        let id = uuid::Uuid::new_v4();
        let time = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis() as u64;
        self.send(Payload::Chat {
            id,
            channel: channel.into(),
            text: text.into(),
            time,
            name: self.profile.name.clone(),
        })?;
        self.history.append(
            &self.store,
            Message {
                id,
                author_key: self.own_key(),
                display_name: self.profile.name.clone(),
                text: text.into(),
                timestamp_ms: time,
                image: None,
                channel: channel.into(),
                edited: false,
                file: None,
            },
        )?;
        Ok(())
    }
    /// Edit one of the local user's own messages and broadcast the change.
    pub fn edit_chat(&mut self, id: uuid::Uuid, text: &str) -> Result<()> {
        ensure!(
            text.len() <= 16 * 1024 && !text.trim().is_empty(),
            "Сообщение пустое или слишком большое"
        );
        let own = self.own_key();
        let target = self
            .history
            .messages
            .iter()
            .find(|m| m.id == id)
            .context("Сообщение не найдено")?;
        ensure!(
            target.author_key == own,
            "Можно редактировать только свои сообщения"
        );
        ensure!(
            target.image.is_none() && target.file.is_none(),
            "Вложение нельзя отредактировать"
        );
        if target.text == text {
            return Ok(());
        }
        self.send(Payload::ChatEdit {
            id,
            text: text.into(),
        })?;
        self.history.edit(&self.store, id, &own, text)?;
        Ok(())
    }
    /// Delete a message locally, or for everyone when the author or the owner asks.
    pub fn delete_chat(&mut self, id: uuid::Uuid, for_everyone: bool) -> Result<()> {
        let Some(target) = self.history.messages.iter().find(|m| m.id == id) else {
            return Ok(());
        };
        let own = self.own_key();
        let authority = self.hosting() || own == self.owner;
        let attachment = (target.image.is_some() || target.file.is_some()).then_some(id);
        if for_everyone {
            ensure!(
                target.author_key == own || authority,
                "Удалять чужие сообщения может только владелец сервера"
            );
            self.send(Payload::ChatDelete { id })?;
        }
        self.history.remove(&self.store, id)?;
        if for_everyone {
            if let Some(attachment) = attachment {
                let _ = crate::attachments::remove(&self.store, attachment);
            }
        }
        Ok(())
    }
    pub fn poll(&mut self) -> Result<()> {
        if !self.hosting() {
            ensure!(
                self.last_owner.elapsed().as_secs() < 15,
                "Сервер больше не отвечает. Сессия закрыта"
            );
            if self.group.is_some() && self.last_probe.elapsed().as_secs() >= 3 {
                self.send(Payload::Probe { nonce: self.nonce })?;
                self.probe_sent = Some(std::time::Instant::now());
                self.last_probe = std::time::Instant::now();
            }
        }
        if let Some(rx) = &self.invite {
            if let Ok(event) = rx.try_recv() {
                match event {
                    PairEvent::Error(error) => anyhow::bail!("{error}"),
                    PairEvent::Code(code) => {
                        self.code = code;
                        self.status =
                            "Один код для всех друзей. После перезапуска сервера код изменится"
                                .into();
                    }
                }
            }
        }
        for _ in 0..64 {
            let packet = match self.connection().incoming.try_recv() {
                Ok(result) => result?,
                Err(mpsc::TryRecvError::Empty) => break,
                Err(_) => anyhow::bail!("Соединение закрыто"),
            };
            match packet {
                Packet::Pending {
                    connection,
                    package,
                } if self.hosting() => {
                    let key = self.identity.package_key(&package);
                    if self.closed
                        || key.as_ref().is_err()
                        || key
                            .as_ref()
                            .is_ok_and(|k| self.banned.contains(k) || self.members.contains_key(k))
                    {
                        self.connection().send(Packet::Disconnect { connection })?;
                        continue;
                    }
                    let group = self.group.as_mut().context("Нет группы")?;
                    if group.members().count() >= 16 {
                        self.connection().send(Packet::Disconnect { connection })?;
                        continue;
                    }
                    let old: Vec<_> = group.members().map(|m| m.index).collect();
                    let (commit, welcome) = match self.identity.add(group, &package) {
                        Ok(value) => value,
                        Err(_) => {
                            self.connection().send(Packet::Disconnect { connection })?;
                            continue;
                        }
                    };
                    if let Some(member) = group.members().find(|m| !old.contains(&m.index)) {
                        self.connections.insert(connection, member.index);
                    }
                    self.identity.save(&self.store)?;
                    self.connection().send(Packet::Commit { data: commit })?;
                    self.connection().send(Packet::Welcome {
                        recipient: connection,
                        data: welcome,
                    })?;
                    self.send_profile()?;
                }
                Packet::Welcome { data, .. } if self.group.is_none() => {
                    let group = self.identity.join(&data)?;
                    ensure!(
                        group
                            .member_at(LeafNodeIndex::new(0))
                            .is_some_and(|m| m.signature_key == self.owner),
                        "Ключ владельца не совпадает с приглашением"
                    );
                    self.group = Some(group);
                    self.identity.save(&self.store)?;
                    self.status = "Проверяем доступность владельца…".into();
                    self.send(Payload::Probe { nonce: self.nonce })?;
                    self.send_profile()?;
                }
                Packet::Commit { data } if !self.hosting() => {
                    let group = self.group.as_mut().context("Нет группы")?;
                    self.identity
                        .receive_owner_commit(group, &data, &self.owner)?;
                    let keys: Vec<_> = group.members().map(|m| m.signature_key).collect();
                    self.members.retain(|key, _| keys.contains(key));
                    self.received_voice.retain(|key, _| keys.contains(key));
                    self.identity.save(&self.store)?;
                    self.send_profile()?;
                }
                Packet::Voice { .. } if self.group.is_none() => {
                    // Voice is carried by QUIC datagrams and can arrive before
                    // the reliable Welcome that installs the MLS group. Drop
                    // it instead of closing the whole session.
                    continue;
                }
                Packet::Content { data } | Packet::Voice { data } => {
                    let group = self.group.as_mut().context("Нет группы")?;
                    let (author, bytes) = match self.identity.receive_content(group, &data) {
                        Ok(value) => value,
                        Err(_) => {
                            self.status = "Отклонён недействительный зашифрованный пакет".into();
                            continue;
                        }
                    };
                    let payload: Payload = match serde_json::from_slice(&bytes) {
                        Ok(value) => value,
                        Err(_) => {
                            self.status = "Отклонено сообщение неизвестного формата".into();
                            continue;
                        }
                    };
                    if !matches!(
                        payload,
                        Payload::VoiceFrame { .. }
                            | Payload::Heartbeat { .. }
                            | Payload::Probe { .. }
                            | Payload::Screen { .. }
                            | Payload::FileChunk { .. }
                    ) {
                        self.identity.save(&self.store)?;
                    }
                    match payload {
                        Payload::Probe { nonce } => {
                            if self.hosting() {
                                self.send(Payload::Heartbeat { nonce })?;
                            }
                        }
                        Payload::Heartbeat { nonce } => {
                            if author == self.owner && nonce == self.nonce {
                                self.last_owner = std::time::Instant::now();
                                self.confirmed = true;
                                self.status = "Подключено · сквозное шифрование MLS".into();
                                if let Some(sent) = self.probe_sent.take() {
                                    self.ping_ms = Some(
                                        sent.elapsed().as_millis().min(u32::MAX as u128) as u32,
                                    );
                                }
                            }
                        }
                        Payload::Image {
                            id,
                            channel,
                            data,
                            time,
                            name,
                        } => {
                            if !valid_channel(&channel)
                                || name.chars().count() > 40
                                || name.chars().any(char::is_control)
                            {
                                continue;
                            }
                            if crate::attachments::save(&self.store, id, &data).is_err() {
                                continue;
                            }
                            self.history.append(
                                &self.store,
                                Message {
                                    id,
                                    author_key: author,
                                    display_name: name,
                                    channel,
                                    text: "Изображение".into(),
                                    timestamp_ms: time,
                                    image: Some(id),
                                    edited: false,
                                    file: None,
                                },
                            )?;
                        }
                        Payload::Profile { name, avatar } => {
                            if name.trim().is_empty()
                                || name.chars().count() > 40
                                || name.chars().any(char::is_control)
                                || avatar.len() > 174_764
                            {
                                continue;
                            }
                            let Ok(avatar) = STANDARD.decode(avatar) else {
                                continue;
                            };
                            let entry = self.members.entry(author).or_insert(Member {
                                name: String::new(),
                                avatar: Vec::new(),
                                voice: None,
                                muted: false,
                                deafened: false,
                                screen: None,
                            });
                            entry.name = name;
                            entry.avatar = avatar;
                        }
                        Payload::Chat {
                            id,
                            channel,
                            text,
                            time,
                            name,
                        } => {
                            if !valid_channel(&channel)
                                || text.trim().is_empty()
                                || text.len() > 16 * 1024
                                || name.chars().count() > 40
                                || name.chars().any(char::is_control)
                            {
                                continue;
                            }
                            self.history.append(
                                &self.store,
                                Message {
                                    id,
                                    author_key: author,
                                    display_name: name,
                                    text,
                                    timestamp_ms: time,
                                    image: None,
                                    channel,
                                    edited: false,
                                    file: None,
                                },
                            )?;
                        }
                        Payload::ChatEdit { id, text } => {
                            if text.len() > 16 * 1024 || text.trim().is_empty() {
                                continue;
                            }
                            let Some(target) = self.history.messages.iter().find(|m| m.id == id)
                            else {
                                continue;
                            };
                            if target.author_key != author
                                || target.image.is_some()
                                || target.file.is_some()
                            {
                                continue;
                            }
                            self.history.edit(&self.store, id, &author, &text)?;
                        }
                        Payload::ChatDelete { id } => {
                            let Some(target) = self.history.messages.iter().find(|m| m.id == id)
                            else {
                                continue;
                            };
                            // Authors may delete their own messages; the owner may delete any.
                            if target.author_key != author && author != self.owner {
                                continue;
                            }
                            let has_attachment = target.image.is_some() || target.file.is_some();
                            self.history.remove(&self.store, id)?;
                            if has_attachment {
                                let _ = crate::attachments::remove(&self.store, id);
                            }
                        }
                        Payload::Voice { .. } => {}
                        Payload::VoiceFrame {
                            channel,
                            data,
                            sequence,
                        } => {
                            if data.len() > 4000 || !valid_channel(&channel) {
                                continue;
                            }
                            if self
                                .received_voice
                                .get(&author)
                                .is_some_and(|last| sequence <= *last)
                            {
                                continue;
                            }
                            self.received_voice.insert(author.clone(), sequence);
                            if self.voice_in.len() < 64 {
                                self.voice_in.push((author, channel, data));
                            }
                        }
                        Payload::VoiceState {
                            channel,
                            muted,
                            deafened,
                        } => {
                            if channel.as_deref().is_some_and(|c| !valid_channel(c)) {
                                continue;
                            }
                            if let Some(m) = self.members.get_mut(&author) {
                                m.voice = channel;
                                m.muted = muted;
                                m.deafened = deafened;
                            }
                        }
                        Payload::Screen {
                            channel,
                            width,
                            height,
                            data,
                        } => {
                            if !valid_channel(&channel)
                                || !(2..=4096).contains(&width)
                                || !(2..=4096).contains(&height)
                                || data.len() < 4
                                || data.len() > 170 * 1024
                                // JPEG magic: anything else is not a frame we can show.
                                || data[0] != 0xFF
                                || data[1] != 0xD8
                            {
                                continue;
                            }
                            if self.screen_rate.get(&author).is_some_and(|last| {
                                last.elapsed() < std::time::Duration::from_millis(90)
                            }) {
                                continue;
                            }
                            self.screen_rate
                                .insert(author.clone(), std::time::Instant::now());
                            self.screen_in = Some((author, channel, width, height, data));
                        }
                        Payload::ScreenState { channel } => {
                            if channel
                                .as_deref()
                                .is_some_and(|channel| !valid_channel(channel))
                            {
                                continue;
                            }
                            if let Some(member) = self.members.get_mut(&author) {
                                member.screen = channel.clone();
                            }
                            if channel.is_none()
                                && self
                                    .screen_in
                                    .as_ref()
                                    .is_some_and(|(frame_author, ..)| frame_author == &author)
                            {
                                self.screen_in = None;
                            }
                        }
                        Payload::ScreenWatch {
                            target,
                            channel,
                            watching,
                        } => {
                            if target != self.own_key()
                                || author == target
                                || !valid_channel(&channel)
                            {
                                continue;
                            }
                            let own = self.own_key();
                            let sharing = self
                                .members
                                .get(&own)
                                .and_then(|member| member.screen.as_deref())
                                == Some(channel.as_str());
                            if !sharing || self.screen_watch_in.len() >= 64 {
                                continue;
                            }
                            self.screen_watch_in.push((author, channel, watching));
                        }
                        Payload::FileStart {
                            id,
                            channel,
                            file_name,
                            size,
                            time,
                            name,
                        } => {
                            if !valid_channel(&channel)
                                || size == 0
                                || size > crate::attachments::MAX_FILE as u64
                                || !crate::attachments::valid_file_name(&file_name)
                                || name.trim().is_empty()
                                || name.chars().count() > 40
                                || name.chars().any(char::is_control)
                                || self.files_in.contains_key(&id)
                                || self.files_in.len() >= 8
                            {
                                continue;
                            }
                            self.files_in.insert(
                                id,
                                IncomingFile {
                                    author,
                                    name,
                                    channel,
                                    file_name,
                                    size: size as usize,
                                    time,
                                    data: Vec::with_capacity(size as usize),
                                    last: std::time::Instant::now(),
                                },
                            );
                        }
                        Payload::FileChunk { id, data } => {
                            if data.is_empty() || data.len() > FILE_CHUNK {
                                continue;
                            }
                            let Some(file) = self.files_in.get_mut(&id) else {
                                continue;
                            };
                            if file.author != author || file.data.len() + data.len() > file.size {
                                self.files_in.remove(&id);
                                continue;
                            }
                            file.data.extend_from_slice(&data);
                            file.last = std::time::Instant::now();
                            if file.data.len() < file.size {
                                continue;
                            }
                            let file = self.files_in.remove(&id).unwrap();
                            if crate::attachments::save_bytes(&self.store, id, &file.data).is_err()
                            {
                                continue;
                            }
                            let _ = self.history.append(
                                &self.store,
                                Message {
                                    id,
                                    author_key: file.author,
                                    display_name: file.name,
                                    channel: file.channel,
                                    text: "Файл".into(),
                                    timestamp_ms: file.time,
                                    image: None,
                                    file: Some(FileMeta {
                                        name: file.file_name,
                                        size: file.size as u64,
                                    }),
                                    edited: false,
                                },
                            );
                        }
                    }
                }
                Packet::Disconnect { connection } if self.hosting() => {
                    if let Some(index) = self.connections.remove(&connection) {
                        let group = self.group.as_mut().context("Нет группы")?;
                        if let Some(member) = group.member_at(index) {
                            self.members.remove(&member.signature_key);
                            self.received_voice.remove(&member.signature_key);
                            self.screen_rate.remove(&member.signature_key);
                        }
                        let (commit, _, _) = group.remove_members(
                            &self.identity.provider,
                            &self.identity.signer,
                            &[index],
                        )?;
                        group.merge_pending_commit(&self.identity.provider)?;
                        use ::tls_codec::Serialize;
                        let data = commit.tls_serialize_detached()?;
                        self.identity.save(&self.store)?;
                        self.connection().send(Packet::Commit { data })?;
                    }
                }
                Packet::Ping => {}
                _ => anyhow::bail!("Неожиданный пакет протокола"),
            }
        }
        if self.last_file_sweep.elapsed() > std::time::Duration::from_secs(10) {
            self.last_file_sweep = std::time::Instant::now();
            self.files_in
                .retain(|_, file| file.last.elapsed() < std::time::Duration::from_secs(120));
        }
        // Re-announce the active stream periodically so people who join later
        // also see the clickable stream card.
        if self.last_screen_announce.elapsed() > std::time::Duration::from_secs(2) {
            let own = self.own_key();
            if let Some(channel) = self
                .members
                .get(&own)
                .and_then(|member| member.screen.clone())
            {
                let _ = self.send(Payload::ScreenState {
                    channel: Some(channel),
                });
                self.last_screen_announce = std::time::Instant::now();
            }
        }
        self.pump_file();
        Ok(())
    }
}
pub fn valid_channel(channel: &str) -> bool {
    matches!(channel, "Общий" | "Игры" | "Музыка")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};
    fn pump(
        host: &mut Session,
        guests: &mut [Session],
        condition: impl Fn(&Session, &[Session]) -> bool,
    ) {
        let until = Instant::now() + Duration::from_secs(8);
        loop {
            host.poll().unwrap();
            for guest in guests.iter_mut() {
                guest.poll().unwrap();
            }
            if condition(host, guests) {
                return;
            }
            assert!(
                Instant::now() < until,
                "Состояния участников не синхронизировались"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    #[test]
    fn file_transfer_reaches_guest_in_chunks() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("guest");
        let mut host = Session::host_with(
            Store::open(dir.path().join("host")).unwrap(),
            Profile {
                name: "Хозяин".into(),
                ..Default::default()
            },
            InternetHost::local,
        )
        .unwrap();
        let guest = |h: &Session| {
            let Link::Host(network) = &h.link else {
                unreachable!()
            };
            Session::from_guest(
                Store::open(root.clone()).unwrap(),
                Profile {
                    name: "Друг".into(),
                    ..Default::default()
                },
                network.local_guest().unwrap(),
            )
            .unwrap()
        };
        let mut guests = vec![guest(&host)];
        pump(&mut host, &mut guests, |h, gs| {
            h.members.len() == 2 && gs[0].ready()
        });

        let payload: Vec<u8> = (0..600_000u32).map(|i| (i % 251) as u8).collect();
        let path = dir.path().join("notes.bin");
        std::fs::write(&path, &payload).unwrap();
        host.file("Общий", &path).unwrap();
        pump(&mut host, &mut guests, |_, gs| {
            gs[0].history.messages.iter().any(|m| m.file.is_some())
        });
        let message = guests[0]
            .history
            .messages
            .iter()
            .find(|m| m.file.is_some())
            .unwrap();
        let meta = message.file.as_ref().unwrap();
        assert_eq!(meta.name, "notes.bin");
        assert_eq!(meta.size, payload.len() as u64);
        let saved = crate::attachments::load_bytes(&guests[0].store, message.id)
            .unwrap()
            .unwrap();
        assert_eq!(saved, payload);
    }

    #[test]
    fn screen_frame_reaches_guest() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("guest");
        let mut host = Session::host_with(
            Store::open(dir.path().join("host")).unwrap(),
            Profile {
                name: "Хозяин".into(),
                ..Default::default()
            },
            InternetHost::local,
        )
        .unwrap();
        let guest = |h: &Session| {
            let Link::Host(network) = &h.link else {
                unreachable!()
            };
            Session::from_guest(
                Store::open(root.clone()).unwrap(),
                Profile {
                    name: "Друг".into(),
                    ..Default::default()
                },
                network.local_guest().unwrap(),
            )
            .unwrap()
        };
        let mut guests = vec![guest(&host)];
        pump(&mut host, &mut guests, |h, gs| {
            h.members.len() == 2 && gs[0].ready()
        });

        let frame = vec![0xFF, 0xD8, 0xFF, 0xE0, 1, 2, 3, 4];
        host.set_screen(Some("Общий")).unwrap();
        pump(&mut host, &mut guests, |h, gs| {
            gs[0]
                .members
                .get(&h.own_key())
                .is_some_and(|member| member.screen.as_deref() == Some("Общий"))
        });
        let host_key = host.own_key();
        let guest_key = guests[0].own_key();
        guests[0]
            .send(Payload::ScreenWatch {
                target: host_key.clone(),
                channel: "Общий".into(),
                watching: true,
            })
            .unwrap();
        pump(&mut host, &mut guests, |h, _| {
            h.screen_watch_in.iter().any(|(viewer, channel, watching)| {
                viewer == &guest_key && channel == "Общий" && *watching
            })
        });
        assert!(
            host.screen_watch_in
                .iter()
                .any(|(_, channel, watching)| { channel == "Общий" && *watching })
        );
        host.set_screen(None).unwrap();
        pump(&mut host, &mut guests, |_, gs| {
            gs[0].members.values().all(|member| member.screen.is_none())
        });
        host.screen("Общий", 320, 180, frame.clone()).unwrap();
        pump(&mut host, &mut guests, |_, gs| gs[0].screen_in.is_some());
        let (author, channel, width, height, data) = guests[0].screen_in.clone().unwrap();
        assert_eq!(author, host.own_key());
        assert_eq!(channel, "Общий");
        assert_eq!((width, height), (320, 180));
        assert_eq!(data, frame);
        assert!(host.screen("Общий", 0, 10, frame).is_err());
    }

    #[test]
    fn ban_rejects_same_identity_and_owner_exit_never_confirms_pending_join() {
        let dir = tempfile::tempdir().unwrap();
        let profile = Profile {
            name: "Друг".into(),
            ..Default::default()
        };
        let root = dir.path().join("guest");
        let mut host = Session::host_with(
            Store::open(dir.path().join("host")).unwrap(),
            Profile {
                name: "Хозяин".into(),
                ..Default::default()
            },
            InternetHost::local,
        )
        .unwrap();
        let make_guest = |h: &Session| {
            let Link::Host(network) = &h.link else {
                unreachable!()
            };
            Session::from_guest(
                Store::open(root.clone()).unwrap(),
                profile.clone(),
                network.local_guest().unwrap(),
            )
            .unwrap()
        };
        let mut guests = vec![make_guest(&host)];
        pump(&mut host, &mut guests, |h, gs| {
            h.members.len() == 2 && gs[0].ready()
        });
        let image_path = dir.path().join("image.png");
        image::RgbImage::from_pixel(16, 16, image::Rgb([12, 40, 90]))
            .save(&image_path)
            .unwrap();
        guests[0].image("Общий", &image_path).unwrap();
        pump(&mut host, &mut guests, |h, _| h.history.messages.len() == 1);
        let image_id = host.history.messages[0].image.unwrap();
        assert!(
            host.store
                .load::<Vec<u8>>(&format!("image-{image_id}"))
                .unwrap()
                .is_some()
        );
        host.ban(&guests[0].own_key()).unwrap();
        guests.clear();
        let mut rejected = make_guest(&host);
        let until = Instant::now() + Duration::from_secs(3);
        loop {
            host.poll().unwrap();
            if rejected.poll().is_err() {
                break;
            }
            assert!(!rejected.ready());
            assert!(Instant::now() < until);
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(host.members.len(), 1);
        drop(rejected);
        // New session clears the ban, but ending it while the new guest is pending must not admit a ghost owner.
        drop(host);
        let host = Session::host_with(
            Store::open(dir.path().join("host")).unwrap(),
            Profile {
                name: "Хозяин".into(),
                ..Default::default()
            },
            InternetHost::local,
        )
        .unwrap();
        let mut pending = make_guest(&host);
        assert!(!pending.ready());
        drop(host);
        let until = Instant::now() + Duration::from_secs(3);
        loop {
            if pending.poll().is_err() {
                break;
            }
            assert!(!pending.ready());
            assert!(Instant::now() < until);
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    #[test]
    fn three_clients_chat_voice_kick_restart_and_history() {
        let dir = tempfile::tempdir().unwrap();
        let profile = |name: &str| Profile {
            name: name.into(),
            network_allowed: true,
            ..Default::default()
        };
        let root = dir.path().join("host");
        let owner = profile("Владелец");
        let mut host = Session::host_with(
            Store::open(root.clone()).unwrap(),
            owner.clone(),
            InternetHost::local,
        )
        .unwrap();
        // Local protocol tests must never manufacture an internet invitation.
        assert!(host.code.is_empty());
        let room = host.room;
        let mut guests = Vec::new();
        for name in ["А", "Б"] {
            let Link::Host(network) = &host.link else {
                unreachable!()
            };
            guests.push(
                Session::from_guest(
                    Store::open(dir.path().join(name)).unwrap(),
                    profile(name),
                    network.local_guest().unwrap(),
                )
                .unwrap(),
            );
        }
        pump(&mut host, &mut guests, |h, gs| {
            h.members.len() == 3 && gs.iter().all(|g| g.members.len() == 3)
        });
        guests[0].chat("Общий", "Сообщение друзьям").unwrap();
        pump(&mut host, &mut guests, |h, gs| {
            h.history.messages.len() == 1 && gs[1].history.messages.len() == 1
        });
        let mut encoder =
            opus::Encoder::new(48_000, opus::Channels::Mono, opus::Application::Voip).unwrap();
        let samples: Vec<i16> = (0..960)
            .map(|n| ((n as f32 * 0.0576).sin() * 10000.0) as i16)
            .collect();
        let mut data = vec![0; 4000];
        let n = encoder.encode(&samples, &mut data).unwrap();
        data.truncate(n);
        guests[0]
            .send(Payload::Voice {
                channel: "Общий".into(),
                data,
            })
            .unwrap();
        pump(&mut host, &mut guests, |h, gs| {
            !h.voice_in.is_empty() && !gs[1].voice_in.is_empty()
        });
        let mut decoder = opus::Decoder::new(48_000, opus::Channels::Mono).unwrap();
        let mut decoded = [0; 960];
        assert_eq!(
            decoder
                .decode(&guests[1].voice_in[0].2, &mut decoded, false)
                .unwrap(),
            960
        );
        assert!(decoded.iter().any(|s| *s != 0));
        guests[0]
            .send(Payload::Profile {
                name: "\n".into(),
                avatar: String::new(),
            })
            .unwrap();
        guests[0]
            .send(Payload::Voice {
                channel: "неизвестный".into(),
                data: vec![1],
            })
            .unwrap();
        guests[0].chat("Игры", "После неверных пакетов").unwrap();
        pump(&mut host, &mut guests, |h, _| h.history.messages.len() == 2);
        host.set_closed(true);
        let key = guests[0].own_key();
        host.kick(&key).unwrap();
        guests.remove(0);
        pump(&mut host, &mut guests, |h, gs| {
            h.members.len() == 2 && gs[0].members.len() == 2
        });
        guests[0].chat("Общий", "Оставшийся участник").unwrap();
        pump(&mut host, &mut guests, |h, _| h.history.messages.len() == 3);
        assert!(host.code.is_empty());
        drop(guests);
        drop(host);
        let next =
            Session::host_with(Store::open(root).unwrap(), owner, InternetHost::local).unwrap();
        assert!(next.code.is_empty());
        assert_eq!(next.room, room);
        assert_eq!(next.history.messages.len(), 3);
    }
}
