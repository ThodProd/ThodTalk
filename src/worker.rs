//! Native session worker over inherited anonymous pipes. No listener or command-line secret.
use crate::{
    history::Message,
    session::{Member, Payload, Session},
    storage::{Profile, Store},
};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    hash::{Hash, Hasher},
    io::{BufRead, BufReader, Write},
    process::{Child, Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

const MAX_LINE: usize = 16 * 1024 * 1024;
#[derive(Serialize, Deserialize)]
enum Request {
    Start {
        hosting: bool,
        profile: Profile,
        code: String,
    },
    Chat {
        channel: String,
        text: String,
    },
    Payload(Payload),
    Profile(Profile),
    Image {
        channel: String,
        path: std::path::PathBuf,
    },
    Kick(Vec<u8>),
    Ban(Vec<u8>),
    Closed(bool),
}
#[derive(Serialize, Deserialize)]
enum Event {
    State(Snapshot),
    Voice(Vec<(Vec<u8>, String, Vec<u8>)>),
    Error(String),
    Fatal(String),
}
#[derive(Serialize, Deserialize)]
struct Snapshot {
    key: Vec<u8>,
    hosting: bool,
    ready: bool,
    members: Vec<(Vec<u8>, Member)>,
    history: Vec<Message>,
    status: String,
    code: String,
    closed: bool,
}
pub struct ViewHistory {
    pub messages: Vec<Message>,
}
pub struct SessionProxy {
    child: Child,
    requests: Option<mpsc::SyncSender<Request>>,
    events: mpsc::Receiver<Result<Event>>,
    key: Vec<u8>,
    hosting: bool,
    ready: bool,
    pub members: BTreeMap<Vec<u8>, Member>,
    pub history: ViewHistory,
    pub status: String,
    pub code: String,
    pub closed: bool,
    pub invite: Option<()>,
    pub voice_in: Vec<(Vec<u8>, String, Vec<u8>)>,
}
fn read<T: serde::de::DeserializeOwned>(reader: &mut impl BufRead) -> Result<T> {
    let mut line = Vec::new();
    loop {
        let buf = reader.fill_buf()?;
        ensure!(!buf.is_empty(), "Рабочий процесс завершён");
        let length = buf
            .iter()
            .position(|b| *b == b'\n')
            .map_or(buf.len(), |n| n + 1);
        ensure!(
            line.len() + length <= MAX_LINE,
            "Превышен размер сообщения рабочего процесса"
        );
        let done = buf[length - 1] == b'\n';
        line.extend_from_slice(&buf[..length]);
        reader.consume(length);
        if done {
            return Ok(serde_json::from_slice(&line)?);
        }
    }
}
fn write(value: &impl Serialize, writer: &mut impl Write) -> Result<()> {
    let bytes = serde_json::to_vec(value)?;
    ensure!(
        bytes.len() < MAX_LINE,
        "Слишком большое сообщение рабочего процесса"
    );
    writer.write_all(&bytes)?;
    writer.write_all(b"\n")?;
    writer.flush()?;
    Ok(())
}
impl SessionProxy {
    pub fn host(store: Store, profile: Profile) -> Result<Self> {
        Self::start(store, profile, true, "")
    }
    pub fn join(store: Store, profile: Profile, code: &str) -> Result<Self> {
        Self::start(store, profile, false, code)
    }
    fn start(store: Store, profile: Profile, hosting: bool, code: &str) -> Result<Self> {
        Self::connect(
            store,
            profile,
            hosting,
            code,
            Arc::new(AtomicBool::new(false)),
        )
    }
    pub fn connect(
        store: Store,
        profile: Profile,
        hosting: bool,
        code: &str,
        cancel: Arc<AtomicBool>,
    ) -> Result<Self> {
        let mut command = Command::new(std::env::current_exe()?);
        command
            .arg("--session-worker")
            .env("THODHUB_DATA_DIR", &store.root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000);
        }
        let mut child = command
            .spawn()
            .context("Не удалось запустить сетевой процесс")?;
        let mut input = child.stdin.take().unwrap();
        let output = child.stdout.take().unwrap();
        let (tx, rx) = mpsc::sync_channel(128);
        let (etx, erx) = mpsc::sync_channel(8);
        std::thread::spawn(move || {
            while let Ok(request) = rx.recv() {
                if write(&request, &mut input).is_err() {
                    break;
                }
            }
        });
        std::thread::spawn(move || {
            let mut reader = BufReader::new(output);
            loop {
                let event = read::<Event>(&mut reader);
                let failed = event.is_err();
                if etx.send(event).is_err() || failed {
                    break;
                }
            }
        });
        let mut result = Self {
            child,
            requests: Some(tx),
            events: erx,
            key: Vec::new(),
            hosting,
            ready: false,
            members: BTreeMap::new(),
            history: ViewHistory {
                messages: Vec::new(),
            },
            status: String::new(),
            code: String::new(),
            closed: false,
            invite: None,
            voice_in: Vec::new(),
        };
        result.request(Request::Start {
            hosting,
            profile,
            code: code.into(),
        })?;
        let deadline = Instant::now() + Duration::from_secs(75);
        loop {
            ensure!(!cancel.load(Ordering::Relaxed), "Подключение отменено");
            ensure!(Instant::now() < deadline, "Рабочий процесс не ответил");
            match result.events.recv_timeout(Duration::from_millis(100)) {
                Ok(event) => {
                    result.apply(event?)?;
                    if result.ready {
                        break;
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(_) => anyhow::bail!("Рабочий процесс завершён"),
            }
        }
        Ok(result)
    }
    fn request(&self, request: Request) -> Result<()> {
        self.requests
            .as_ref()
            .context("Соединение закрыто")?
            .try_send(request)
            .map_err(|_| anyhow::anyhow!("Рабочий процесс занят или завершён"))
    }
    fn apply(&mut self, event: Event) -> Result<()> {
        match event {
            Event::State(s) => {
                self.key = s.key;
                self.hosting = s.hosting;
                self.ready = s.ready;
                self.members = s.members.into_iter().collect();
                self.history.messages = s.history;
                self.status = s.status;
                self.code = s.code;
                self.closed = s.closed;
                self.invite = self.hosting.then_some(());
            }
            Event::Voice(frames) => {
                for frame in frames {
                    if self.voice_in.len() < 64 {
                        self.voice_in.push(frame);
                    }
                }
            }
            Event::Error(error) => self.status = error,
            Event::Fatal(error) => anyhow::bail!("{error}"),
        }
        Ok(())
    }
    pub fn poll(&mut self) -> Result<()> {
        for _ in 0..64 {
            match self.events.try_recv() {
                Ok(event) => self.apply(event?)?,
                Err(mpsc::TryRecvError::Empty) => break,
                Err(_) => anyhow::bail!("Рабочий процесс завершён"),
            }
        }
        Ok(())
    }
    pub fn hosting(&self) -> bool {
        self.hosting
    }
    pub fn ready(&self) -> bool {
        self.ready
    }
    pub fn own_key(&self) -> Vec<u8> {
        self.key.clone()
    }
    pub fn set_closed(&mut self, value: bool) {
        if let Err(e) = self.request(Request::Closed(value)) {
            self.status = e.to_string();
        }
    }
    pub fn ban(&mut self, key: &[u8]) -> Result<()> {
        self.request(Request::Ban(key.into()))
    }
    pub fn image(&mut self, channel: &str, path: std::path::PathBuf) -> Result<()> {
        self.request(Request::Image {
            channel: channel.into(),
            path,
        })
    }
    pub fn kick(&mut self, key: &[u8]) -> Result<()> {
        self.request(Request::Kick(key.into()))
    }
    pub fn send(&mut self, payload: Payload) -> Result<()> {
        self.request(Request::Payload(payload))
    }
    pub fn chat(&mut self, channel: &str, text: &str) -> Result<()> {
        self.request(Request::Chat {
            channel: channel.into(),
            text: text.into(),
        })
    }
    pub fn update_profile(&mut self, profile: Profile) -> Result<()> {
        self.request(Request::Profile(profile))
    }
}
impl Drop for SessionProxy {
    fn drop(&mut self) {
        self.requests.take();
        // All durable data is flushed during operations. Kill only this owned child.
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
fn snapshot(session: &Session) -> Snapshot {
    Snapshot {
        key: session.own_key(),
        hosting: session.hosting(),
        ready: session.ready(),
        members: session
            .members
            .iter()
            .map(|(k, m)| (k.clone(), m.clone()))
            .collect(),
        history: session
            .history
            .messages
            .iter()
            .rev()
            .take(300)
            .cloned()
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect(),
        status: session.status.clone(),
        code: session.code.clone(),
        closed: session.closed,
    }
}
fn revision(session: &Session) -> u64 {
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    session.members.hash(&mut hash);
    session.history.messages.len().hash(&mut hash);
    session.status.hash(&mut hash);
    session.code.hash(&mut hash);
    session.closed.hash(&mut hash);
    session.ready().hash(&mut hash);
    hash.finish()
}
pub fn run() -> Result<()> {
    let (tx, rx) = mpsc::sync_channel::<Request>(128);
    std::thread::spawn(move || {
        let mut input = BufReader::new(std::io::stdin());
        loop {
            match read(&mut input) {
                Ok(request) => {
                    if tx.send(request).is_err() {
                        return;
                    }
                }
                Err(_) => std::process::exit(0),
            }
        }
    });
    let Request::Start {
        hosting,
        profile,
        code,
    } = rx.recv()?
    else {
        anyhow::bail!("Ожидался запуск сессии")
    };
    let store = Store::open(Store::default_path()?)?;
    let mut output = std::io::stdout().lock();
    let mut session = match if hosting {
        Session::host(store, profile)
    } else {
        Session::join(store, profile, &code)
    } {
        Ok(session) => session,
        Err(error) => {
            write(&Event::Fatal(format!("{error:#}")), &mut output)?;
            return Ok(());
        }
    };
    session.poll()?;
    write(&Event::State(snapshot(&session)), &mut output)?;
    let mut last = revision(&session);
    loop {
        for _ in 0..64 {
            let Ok(request) = rx.try_recv() else {
                break;
            };
            let result = match request {
                Request::Chat { channel, text } => session.chat(&channel, &text),
                Request::Payload(payload) => session.send(payload),
                Request::Profile(profile) => session.update_profile(profile),
                Request::Image { channel, path } => session.image(&channel, &path),
                Request::Kick(key) => session.kick(&key),
                Request::Ban(key) => session.ban(&key),
                Request::Closed(value) => {
                    session.set_closed(value);
                    Ok(())
                }
                Request::Start { .. } => Err(anyhow::anyhow!("Сессия уже запущена")),
            };
            if let Err(error) = result {
                write(&Event::Error(error.to_string()), &mut output)?;
            }
        }
        if let Err(error) = session.poll() {
            write(&Event::Fatal(error.to_string()), &mut output)?;
            break;
        }
        if !session.voice_in.is_empty() {
            write(
                &Event::Voice(std::mem::take(&mut session.voice_in)),
                &mut output,
            )?;
        }
        let next = revision(&session);
        if next != last {
            write(&Event::State(snapshot(&session)), &mut output)?;
            last = next;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    Ok(())
}
