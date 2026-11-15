//! Authenticated iroh QUIC rendezvous; MLS remains the end-to-end content layer.
use crate::{relay::Relay, transport::Connection};
use anyhow::{Context, Result, ensure};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use iroh::{Endpoint, EndpointAddr, EndpointId, endpoint::presets};
use serde::{Deserialize, Serialize};
use std::{
    sync::{
        Arc, RwLock,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::Duration,
};
use subtle::ConstantTimeEq;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    runtime::Runtime,
};

fn trace(stage: &str) {
    if std::env::var_os("THODTALK_TEST_TRACE").is_some() {
        eprintln!("Сеть: {stage}");
    }
}
const ALPN: &[u8] = b"thodtalk/3";
#[derive(Clone, Serialize, Deserialize)]
pub struct Invitation {
    pub address: EndpointAddr,
    pub owner_key: Vec<u8>,
    pub room: uuid::Uuid,
}
pub enum PairEvent {
    Code(String),
}

pub struct InternetHost {
    pub relay: Relay,
    endpoint: Endpoint,
    runtime: Arc<Runtime>,
    invitation: Arc<RwLock<Option<Invitation>>>,
    token: [u8; 16],
    task: tokio::task::JoinHandle<()>,
    closed: Arc<AtomicBool>,
    _lan: Option<crate::lan::Discovery>,
}
fn runtime() -> Result<Arc<Runtime>> {
    Ok(Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()?,
    ))
}
fn internet_builder() -> iroh::endpoint::Builder {
    let builder = Endpoint::builder(presets::N0).transport_config(
        iroh::endpoint::QuicTransportConfig::builder()
            .enable_segmentation_offload(false)
            .build(),
    );
    let builder = if std::env::var_os("THODTALK_TEST_RELAY_ONLY").is_some() {
        builder.clear_ip_transports()
    } else {
        builder
    };
    if let Ok(region) = std::env::var("THODTALK_TEST_RELAY_REGION") {
        let name = match region.as_str() {
            "asia" => "aps1-1",
            "europe" => "euc1-1",
            "west" => "usw1-1",
            _ => "use1-1",
        };
        let url: iroh::RelayUrl = format!("https://{name}.relay.n0.iroh.link./")
            .parse()
            .unwrap();
        return builder.relay_mode(iroh::RelayMode::Custom(iroh::RelayMap::from_iter([url])));
    }
    builder
}
impl InternetHost {
    #[cfg(test)]
    pub(crate) fn local() -> Result<Self> {
        let rt = runtime()?;
        let ep = rt.block_on(
            Endpoint::builder(presets::Minimal)
                .alpns(vec![ALPN.to_vec()])
                .bind(),
        )?;
        Self::start_with(Relay::start("127.0.0.1:0".parse()?)?, rt, ep)
    }
    #[cfg(test)]
    pub(crate) fn local_guest(&self) -> Result<InternetGuest> {
        let rt = runtime()?;
        let ep = rt.block_on(Endpoint::bind(presets::Minimal))?;
        InternetGuest::join_with(rt, ep, self.endpoint.addr(), self.token)
    }
    pub fn start() -> Result<Self> {
        let relay = Relay::start("127.0.0.1:0".parse()?)?;
        let runtime = runtime()?;
        let endpoint = runtime.block_on(internet_builder().alpns(vec![ALPN.to_vec()]).bind())?;
        Self::start_with(relay, runtime, endpoint)
    }
    fn start_with(relay: Relay, runtime: Arc<Runtime>, endpoint: Endpoint) -> Result<Self> {
        let token = *uuid::Uuid::new_v4().as_bytes();
        let invitation: Arc<RwLock<Option<Invitation>>> = Arc::default();
        let closed = Arc::new(AtomicBool::new(false));
        let ep = endpoint.clone();
        let info = invitation.clone();
        let address = relay.address;
        let gate = closed.clone();
        let task = runtime.spawn(async move {
            let slots = Arc::new(tokio::sync::Semaphore::new(16));
            while let Some(incoming) = ep.accept().await {
                let Ok(slot) = slots.clone().try_acquire_owned() else {
                    incoming.refuse();
                    continue;
                };
                let gate = gate.clone();
                let info = info.clone();
                tokio::spawn(async move {
                    let _slot = slot;
                    let incoming_result =
                        tokio::time::timeout(Duration::from_secs(60), incoming).await;
                    #[cfg(debug_assertions)]
                    if let Err(error) = &incoming_result {
                        eprintln!("QUIC: рукопожатие: {error}");
                    }
                    #[cfg(debug_assertions)]
                    if let Ok(Err(error)) = &incoming_result {
                        eprintln!("QUIC: рукопожатие отклонено: {error}");
                    }
                    let Ok(Ok(connection)) = incoming_result else {
                        return;
                    };
                    trace("хозяин: QUIC установлен");
                    let setup = tokio::time::timeout(Duration::from_secs(30), async {
                        ensure!(!gate.load(Ordering::SeqCst), "Вход закрыт");
                        let (mut send, mut recv) = connection.accept_bi().await?;
                        trace("хозяин: принят поток");
                        #[cfg(debug_assertions)]
                        eprintln!("QUIC: поток приглашения открыт");
                        let mut supplied = [0u8; 16];
                        recv.read_exact(&mut supplied).await?;
                        ensure!(bool::from(token.ct_eq(&supplied)), "Неверное приглашение");
                        ensure!(!gate.load(Ordering::SeqCst), "Вход закрыт");
                        let invitation = info
                            .read()
                            .ok()
                            .and_then(|i| i.clone())
                            .context("Сервер запускается")?;
                        let bytes = serde_json::to_vec(&invitation)?;
                        send.write_u32(bytes.len() as u32).await?;
                        send.write_all(&bytes).await?;
                        send.finish()?;
                        trace("приглашение отправлено");
                        // Close the bootstrap stream before opening the framed message stream.
                        let (mut send, mut recv) = connection.accept_bi().await?;
                        trace("хозяин: принят поток");
                        ensure!(recv.read_u8().await? == 3, "Неверный протокол сессии");
                        ensure!(!gate.load(Ordering::SeqCst), "Сервер закрыл вход");
                        let tcp = tokio::net::TcpStream::connect(address).await?;
                        tcp.set_nodelay(true)?;
                        send.write_all(&[3]).await?;
                        trace("отправлено подтверждение потока");
                        Ok::<_, anyhow::Error>((send, recv, tcp))
                    })
                    .await;
                    #[cfg(debug_assertions)]
                    if let Err(error) = &setup {
                        eprintln!("QUIC: ожидание входа: {error}");
                    }
                    #[cfg(debug_assertions)]
                    if let Ok(Err(error)) = &setup {
                        eprintln!("QUIC: вход отклонён: {error:#}");
                    }
                    if let Ok(Ok((send, recv, tcp))) = setup {
                        let _ = bridge(&connection, send, recv, tcp).await;
                    }
                    connection.close(0u8.into(), b"closed");
                });
            }
        });
        let lan = crate::lan::Discovery::start(endpoint.clone()).ok();
        Ok(Self {
            relay,
            endpoint,
            runtime,
            invitation,
            token,
            task,
            closed,
            _lan: lan,
        })
    }
    pub fn set_closed(&self, closed: bool) {
        self.closed.store(closed, Ordering::SeqCst);
    }
    pub fn invite(&self, owner_key: Vec<u8>, room: uuid::Uuid) -> mpsc::Receiver<PairEvent> {
        *self.invitation.write().unwrap() = Some(Invitation {
            address: self.endpoint.addr(),
            owner_key,
            room,
        });
        let code = encode_code(self.endpoint.id(), &self.token);
        let (tx, rx) = mpsc::channel();
        let _ = tx.send(PairEvent::Code(code));
        rx
    }
}
impl Drop for InternetHost {
    fn drop(&mut self) {
        self.closed.store(true, Ordering::SeqCst);
        *self.invitation.write().unwrap() = None;
        self.task.abort();
        self.runtime.block_on(self.endpoint.close());
    }
}

pub struct InternetGuest {
    pub connection: Connection,
    pub invitation: Invitation,
    endpoint: Endpoint,
    runtime: Arc<Runtime>,
    task: tokio::task::JoinHandle<()>,
}
impl InternetGuest {
    pub fn join(code: &str) -> Result<Self> {
        let (id, token) = parse_code(code)?;
        let (tx, rx) = mpsc::channel();
        let done = Arc::new(AtomicBool::new(false));
        for relay_only in [false, true] {
            let tx = tx.clone();
            let done = done.clone();
            std::thread::spawn(move || {
                if relay_only {
                    std::thread::sleep(Duration::from_secs(3));
                }
                if done.load(Ordering::Relaxed) {
                    return;
                }
                let result = (|| -> Result<Self> {
                    let runtime = runtime()?;
                    let builder = internet_builder();
                    let builder = if relay_only {
                        builder.clear_ip_transports()
                    } else {
                        builder
                    };
                    let endpoint = runtime.block_on(builder.bind())?;
                    let local =
                        if relay_only || std::env::var_os("THODTALK_TEST_PUBLIC_ONLY").is_some() {
                            None
                        } else {
                            crate::lan::find(id)
                        };
                    if local.is_none() {
                        if let Err(error) = runtime.block_on(tokio_wait_online(&endpoint)) {
                            runtime.block_on(endpoint.close());
                            return Err(error);
                        }
                    }
                    Self::join_with(runtime, endpoint, local.unwrap_or_else(|| id.into()), token)
                })();
                if result.is_ok() {
                    trace(if relay_only {
                        "вход через совместимый relay-маршрут"
                    } else {
                        "вход с автоматическим выбором пути"
                    });
                }
                let _ = tx.send(result);
            });
        }
        drop(tx);
        let mut last = None;
        for result in rx {
            match result {
                Ok(guest) => {
                    done.store(true, Ordering::Relaxed);
                    return Ok(guest);
                }
                Err(e) => last = Some(e),
            }
        }
        Err(last.unwrap_or_else(|| anyhow::anyhow!("Нет доступного маршрута")))
            .context("Сервер не существует, завершил сессию или недоступен")
    }

    fn join_with(
        runtime: Arc<Runtime>,
        endpoint: Endpoint,
        address: EndpointAddr,
        token: [u8; 16],
    ) -> Result<Self> {
        let setup = runtime.block_on(async {
            tokio::time::timeout(Duration::from_secs(20), async {
                // QUIC authenticates the exact public key embedded in the invitation.
                let quic = loop {
                    match endpoint.connect(address.clone(), ALPN).await {
                        Ok(connection) => break connection,
                        Err(_) => tokio::time::sleep(Duration::from_millis(750)).await,
                    }
                };
                trace("гость: QUIC установлен / открытие потока");
                let (mut send, mut recv) = quic.open_bi().await?;
                #[cfg(debug_assertions)]
                eprintln!("QUIC: клиент отправляет приглашение");
                send.write_all(&token)
                    .await
                    .context("Передача приглашения")?;
                send.finish()?;
                trace("приглашение отправлено");
                let len =
                    recv.read_u32()
                        .await
                        .context("Сервер не подтвердил приглашение")? as usize;
                ensure!(len <= 16384, "Некорректное приглашение");
                let mut bytes = vec![0; len];
                recv.read_exact(&mut bytes).await?;
                let invitation: Invitation = serde_json::from_slice(&bytes)?;
                trace("гость: приглашение получено");
                ensure!(
                    invitation.owner_key.len() == 32,
                    "Некорректный ключ владельца"
                );
                ensure!(
                    recv.read_to_end(0).await?.is_empty(),
                    "Лишние данные приглашения"
                );
                trace("гость: QUIC установлен / открытие потока");
                let (mut send, mut recv) = quic.open_bi().await?;
                send.write_all(&[3]).await?;
                trace("отправлено подтверждение потока");
                ensure!(
                    recv.read_u8().await? == 3,
                    "Сервер не подтвердил запуск сессии"
                );
                Ok::<_, anyhow::Error>((quic, send, recv, invitation))
            })
            .await
            .context("Сервер недоступен. Проверьте код, сеть и открыт ли вход")?
        });
        let (quic, send, recv, invitation) = match setup {
            Ok(value) => value,
            Err(error) => {
                runtime.block_on(endpoint.close());
                return Err(error);
            }
        };
        let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
        let connection = Connection::connect(listener.local_addr()?)?;
        let (tcp, _) = listener.accept()?;
        tcp.set_nonblocking(true)?;
        tcp.set_nodelay(true)?;
        let task = runtime.spawn(async move {
            let Ok(tcp) = tokio::net::TcpStream::from_std(tcp) else {
                return;
            };
            let _ = bridge(&quic, send, recv, tcp).await;
            quic.close(0u8.into(), b"closed");
        });
        Ok(Self {
            connection,
            invitation,
            endpoint,
            runtime,
            task,
        })
    }
}
async fn tokio_wait_online(endpoint: &Endpoint) -> Result<()> {
    tokio::time::timeout(Duration::from_secs(8), endpoint.online())
        .await
        .context(
            "Бесплатный узел связи iroh недоступен. Проверьте интернет и повторите запуск сервера",
        )?;
    Ok(())
}
impl Drop for InternetGuest {
    fn drop(&mut self) {
        self.task.abort();
        self.runtime.block_on(self.endpoint.close());
    }
}
fn encode_code(id: EndpointId, token: &[u8; 16]) -> String {
    let mut bytes = id.as_bytes().to_vec();
    bytes.extend_from_slice(token);
    format!("TT3:{}", URL_SAFE_NO_PAD.encode(bytes))
}
pub fn parse_code(code: &str) -> Result<(EndpointId, [u8; 16])> {
    let value = code.trim();
    ensure!(
        value.len() == 68 && value.starts_with("TT3:"),
        "Вставьте полный код сервера, начинающийся с TT3:"
    );
    let bytes = URL_SAFE_NO_PAD.decode(&value[4..])?;
    ensure!(bytes.len() == 48, "Некорректный код");
    Ok((
        EndpointId::from_bytes(bytes[..32].try_into()?)?,
        bytes[32..].try_into()?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn code_is_bound_to_endpoint_and_secret() {
        let rt = runtime().unwrap();
        let ep = rt.block_on(Endpoint::bind(presets::Minimal)).unwrap();
        let code = encode_code(ep.id(), &[19; 16]);
        let (id, token) = parse_code(&code).unwrap();
        assert_eq!(id, ep.id());
        assert_eq!(token, [19; 16]);
        assert!(parse_code(&code[..67]).is_err());
        rt.block_on(ep.close());
    }
    #[test]
    fn authenticated_quic_checks_token_gate_and_cleanup() {
        let rt = runtime().unwrap();
        let ep = rt
            .block_on(
                Endpoint::builder(presets::Minimal)
                    .alpns(vec![ALPN.to_vec()])
                    .bind(),
            )
            .unwrap();
        let host = InternetHost::start_with(
            Relay::start("127.0.0.1:0".parse().unwrap()).unwrap(),
            rt,
            ep,
        )
        .unwrap();
        let _ = host.invite(vec![1; 32], uuid::Uuid::new_v4());
        let address = host.endpoint.addr();
        let connect = |token| {
            let rt = runtime().unwrap();
            let ep = rt.block_on(Endpoint::bind(presets::Minimal)).unwrap();
            InternetGuest::join_with(rt, ep, address.clone(), token)
        };
        assert!(connect([0; 16]).is_err());
        let guest = connect(host.token).unwrap();
        host.set_closed(true);
        assert!(connect(host.token).is_err());
        guest
            .connection
            .send(crate::wire::Packet::Join {
                package: vec![1, 2, 3],
            })
            .unwrap();
        assert!(matches!(
            host.relay
                .owner
                .incoming
                .recv_timeout(Duration::from_secs(5))
                .unwrap()
                .unwrap(),
            crate::wire::Packet::Pending { .. }
        ));
        host.set_closed(false);
        assert!(connect(host.token).is_ok());
    }
}

// Voice has no retransmission backlog. MLS authenticates datagrams exactly as stream content.
async fn bridge(
    connection: &iroh::endpoint::Connection,
    mut send: iroh::endpoint::SendStream,
    mut recv: iroh::endpoint::RecvStream,
    tcp: tokio::net::TcpStream,
) -> Result<()> {
    use crate::wire::{MAX_FRAME, Packet};
    let (mut input, output) = tcp.into_split();
    let output = tokio::sync::Mutex::new(output);
    async fn frame(r: &mut (impl tokio::io::AsyncRead + Unpin)) -> Result<Vec<u8>> {
        let n = r.read_u32().await? as usize;
        ensure!(n > 0 && n <= MAX_FRAME, "Недопустимый пакет");
        let mut b = vec![0; n];
        r.read_exact(&mut b).await?;
        Ok(b)
    }
    let (reliable_tx, mut reliable_rx) = tokio::sync::mpsc::channel::<Vec<u8>>(8);
    let outgoing = async {
        loop {
            let bytes = frame(&mut input).await?;
            if matches!(
                serde_json::from_slice::<Packet>(&bytes)?,
                Packet::Voice { .. }
            ) {
                // Oversized or congestion-dropped voice is never retried on the reliable stream.
                let _ = connection.send_datagram(bytes.into());
            } else {
                reliable_tx
                    .send(bytes)
                    .await
                    .context("Поток сообщений закрыт")?;
            }
        }
        #[allow(unreachable_code)]
        Ok::<(), anyhow::Error>(())
    };
    let reliable_out = async {
        while let Some(bytes) = reliable_rx.recv().await {
            send.write_u32(bytes.len() as u32).await?;
            send.write_all(&bytes).await?;
        }
        Ok::<(), anyhow::Error>(())
    };
    let reliable = async {
        loop {
            let bytes = frame(&mut recv).await?;
            let mut w = output.lock().await;
            w.write_u32(bytes.len() as u32).await?;
            w.write_all(&bytes).await?;
        }
        #[allow(unreachable_code)]
        Ok::<(), anyhow::Error>(())
    };
    let voice = async {
        loop {
            let bytes = connection.read_datagram().await?;
            if bytes.len() > 8192
                || !matches!(
                    serde_json::from_slice::<Packet>(&bytes),
                    Ok(Packet::Voice { .. })
                )
            {
                continue;
            }
            let mut w = output.lock().await;
            w.write_u32(bytes.len() as u32).await?;
            w.write_all(&bytes).await?;
        }
        #[allow(unreachable_code)]
        Ok::<(), anyhow::Error>(())
    };
    tokio::select! { r = outgoing => r, r = reliable_out => r, r = reliable => r, r = voice => r }
}
