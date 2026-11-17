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

const ALPN: &[u8] = b"thodtalk/3";
const ROUTE_ALPN: &[u8] = b"thodtalk/route/1";
#[derive(Clone, Serialize, Deserialize)]
pub struct Invitation {
    pub address: EndpointAddr,
    pub owner_key: Vec<u8>,
    pub room: uuid::Uuid,
}
pub enum PairEvent {
    Code(String),
    Error(String),
}

pub struct InternetHost {
    pub relay: Relay,
    endpoint: Endpoint,
    runtime: Arc<Runtime>,
    invitation: Arc<RwLock<Option<Invitation>>>,
    token: [u8; 16],
    task: tokio::task::JoinHandle<()>,
    closed: Arc<AtomicBool>,
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
    Endpoint::builder(presets::N0).transport_config(
        iroh::endpoint::QuicTransportConfig::builder()
            .max_idle_timeout(Some(
                Duration::from_secs(15)
                    .try_into()
                    .expect("valid idle timeout"),
            ))
            .build(),
    )
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
        // A relay can accept HTTPS and QUIC handshakes yet stall on application
        // data. Race the production relays using a real bidirectional exchange.
        let (tx, rx) = mpsc::channel();
        let done = Arc::new(AtomicBool::new(false));
        for relay in iroh::RelayMode::Default.relay_map().urls::<Vec<_>>() {
            let tx = tx.clone();
            let done = done.clone();
            std::thread::spawn(move || {
                let result = (|| -> Result<Self> {
                    let runtime = runtime()?;
                    let endpoint = runtime.block_on(
                        internet_builder()
                            .relay_mode(iroh::RelayMode::custom([relay.clone()]))
                            .alpns(vec![ALPN.to_vec(), ROUTE_ALPN.to_vec()])
                            .bind(),
                    )?;
                    let host =
                        Self::start_with(Relay::start("127.0.0.1:0".parse()?)?, runtime, endpoint)?;
                    host.runtime.block_on(check_public_route(
                        &host.endpoint,
                        relay.clone(),
                        done.clone(),
                    ))?;
                    Ok(host)
                })()
                .with_context(|| format!("Интернет-маршрут {relay}"));
                if result.is_ok() && done.swap(true, Ordering::SeqCst) {
                    return;
                }
                let _ = tx.send(result);
            });
        }
        drop(tx);
        let mut errors = Vec::new();
        for result in rx {
            match result {
                Ok(host) => return Ok(host),
                Err(error) => errors.push(format!("{error:#}")),
            }
        }
        anyhow::bail!(
            "Не найден работающий интернет-relay. Приглашение не создано. {}",
            errors.join("; ")
        )
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

                    let Ok(Ok(connection)) = incoming_result else {
                        return;
                    };

                    if connection.alpn() == ROUTE_ALPN {
                        let _ = tokio::time::timeout(Duration::from_secs(5), async {
                            let (mut send, mut recv) = connection.accept_bi().await?;
                            ensure!(recv.read_u8().await? == 1, "Неверный запрос маршрута");
                            send.write_all(&[1]).await?;
                            send.finish()?;
                            let _ = send.stopped().await;
                            Ok::<_, anyhow::Error>(())
                        })
                        .await;
                        connection.close(0u8.into(), b"route checked");
                        return;
                    }
                    let setup = tokio::time::timeout(Duration::from_secs(30), async {
                        ensure!(!gate.load(Ordering::SeqCst), "Вход закрыт");
                        let (mut send, mut recv) = connection.accept_bi().await?;

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

                        // Close the bootstrap stream before opening the framed message stream.
                        let (mut send, mut recv) = connection.accept_bi().await?;

                        ensure!(recv.read_u8().await? == 3, "Неверный протокол сессии");
                        ensure!(!gate.load(Ordering::SeqCst), "Сервер закрыл вход");
                        let tcp = tokio::net::TcpStream::connect(address).await?;
                        tcp.set_nodelay(true)?;
                        send.write_all(&[3]).await?;

                        Ok::<_, anyhow::Error>((send, recv, tcp))
                    })
                    .await;

                    if let Ok(Ok((send, recv, tcp))) = setup {
                        let _ = bridge(&connection, send, recv, tcp).await;
                    }
                    connection.close(0u8.into(), b"closed");
                });
            }
        });
        Ok(Self {
            relay,
            endpoint,
            runtime,
            invitation,
            token,
            task,
            closed,
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
        let (tx, rx) = mpsc::channel();
        let endpoint = self.endpoint.clone();
        let self_token = self.token;
        self.runtime.spawn(async move {
            let event = match tokio::time::timeout(Duration::from_secs(40), endpoint.online()).await {
                Ok(()) => match internet_address(&endpoint.addr()) {
                    Ok(address) => PairEvent::Code(encode_address_code(&address, &self_token)),
                    Err(error) => PairEvent::Error(error.to_string()),
                },
                Err(_) => PairEvent::Error("Не удалось подключить сервер к интернет-relay за 40 секунд. Приглашение не создано. Проверьте интернет и повторите запуск сервера.".into()),
            };
            let _ = tx.send(event);
        });
        rx
    }
}
async fn close_endpoint(endpoint: &Endpoint) {
    let _ = tokio::time::timeout(Duration::from_secs(1), endpoint.close()).await;
}

// This health exchange has no LAN/IP transport and cannot pass via loopback.
async fn check_public_route(
    host: &Endpoint,
    relay: iroh::RelayUrl,
    cancel: Arc<AtomicBool>,
) -> Result<()> {
    let probe = Endpoint::builder(presets::Minimal)
        .clear_ip_transports()
        .relay_mode(iroh::RelayMode::custom([relay.clone()]))
        .bind()
        .await?;
    let result = tokio::select! {
        result = tokio::time::timeout(Duration::from_secs(30), async {
            host.online().await;
            let connection = probe.connect(EndpointAddr::new(host.id()).with_relay_url(relay), ROUTE_ALPN).await?;
            let (mut send, mut recv) = connection.open_bi().await?;
            send.write_all(&[1]).await?;
            send.finish()?;
            ensure!(recv.read_u8().await? == 1, "Relay не подтвердил передачу данных");
            connection.close(0u8.into(), b"route checked");
            Ok::<_, anyhow::Error>(())
        }) => result.context("Relay не передаёт данные за 30 секунд").and_then(|result| result),
        _ = async {
            while !cancel.load(Ordering::Relaxed) {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        } => Err(anyhow::anyhow!("Выбран другой интернет-маршрут")),
    };
    let _ = tokio::time::timeout(Duration::from_secs(1), probe.close()).await;
    result
}

// Invitations always carry an internet route, never a snapshot of private LAN IPs.
fn internet_address(address: &EndpointAddr) -> Result<EndpointAddr> {
    let mut public = EndpointAddr::new(address.id);
    for relay in address.relay_urls() {
        public = public.with_relay_url(relay.clone());
    }
    ensure!(
        public.relay_urls().next().is_some(),
        "Интернет-relay ещё не готов; приглашение не создано"
    );
    Ok(public)
}
impl Drop for InternetHost {
    fn drop(&mut self) {
        self.closed.store(true, Ordering::SeqCst);
        *self.invitation.write().unwrap() = None;
        self.task.abort();
        self.runtime.block_on(close_endpoint(&self.endpoint));
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
        let (address, token) = parse_address_code(code)?;
        let (tx, rx) = mpsc::channel();
        let done = Arc::new(AtomicBool::new(false));
        for relay_only in [false, true] {
            let address = address.clone();
            let tx = tx.clone();
            let done = done.clone();
            std::thread::spawn(move || {
                if relay_only {
                    std::thread::sleep(Duration::from_millis(500));
                }
                if done.load(Ordering::Relaxed) {
                    return;
                }
                let result = (|| -> Result<Self> {
                    let runtime = runtime()?;
                    let builder = internet_builder();
                    let relays: Vec<_> = address.relay_urls().cloned().collect();
                    let builder = if relays.is_empty() {
                        builder
                    } else {
                        builder.relay_mode(iroh::RelayMode::custom(relays))
                    };
                    let builder = if relay_only {
                        builder.clear_ip_transports()
                    } else {
                        builder
                    };
                    let endpoint = runtime.block_on(builder.bind())?;
                    Self::join_with_cancel(runtime, endpoint, address, token, done.clone())
                })();
                if result.is_ok() && done.swap(true, Ordering::SeqCst) {
                    return; // Drop and close a second successful route immediately.
                }
                let route = if relay_only {
                    "relay"
                } else {
                    "автоматический маршрут"
                };
                let result = result.context(route);
                let _ = tx.send(result);
            });
        }
        drop(tx);
        let mut errors = Vec::new();
        for result in rx {
            match result {
                Ok(guest) => {
                    done.store(true, Ordering::Relaxed);
                    return Ok(guest);
                }
                Err(e) => errors.push(format!("{e:#}")),
            }
        }
        anyhow::bail!(
            "Не удалось связаться с сервером. Убедитесь, что владелец не перезапускал сервер и код актуален. {}",
            errors.join("; ")
        )
    }

    #[cfg(test)]
    fn join_with(
        runtime: Arc<Runtime>,
        endpoint: Endpoint,
        address: EndpointAddr,
        token: [u8; 16],
    ) -> Result<Self> {
        Self::join_with_cancel(
            runtime,
            endpoint,
            address,
            token,
            Arc::new(AtomicBool::new(false)),
        )
    }

    fn join_with_cancel(
        runtime: Arc<Runtime>,
        endpoint: Endpoint,
        address: EndpointAddr,
        token: [u8; 16],
        cancel: Arc<AtomicBool>,
    ) -> Result<Self> {
        let mut stage = "установка QUIC-соединения";
        let mut last_error = String::new();
        let setup = runtime.block_on(async {
            let dial = tokio::time::timeout(Duration::from_secs(45), async {
                // QUIC authenticates the exact public key embedded in the invitation.
                // Relay selection and DNS publication can finish shortly after
                // the first dial. Keep retrying the same authenticated address
                // instead of turning a transient miss into a hard timeout.
                let quic = loop {
                    match tokio::time::timeout(Duration::from_secs(10), endpoint.connect(address.clone(), ALPN)).await {
                        Ok(Ok(connection)) => break connection,
                        result => {
                            last_error = match result {
                                Ok(Err(error)) => error.to_string(),
                                Err(_) => "маршрут не ответил за 10 секунд".into(),
                                _ => unreachable!(),
                            };

                            tokio::time::sleep(Duration::from_millis(750)).await;
                        }
                    }
                };

                stage = "проверка приглашения сервером";
                let (mut send, mut recv) = quic.open_bi().await?;

                send.write_all(&token)
                    .await
                    .context("Передача приглашения")?;
                send.finish()?;

                let len =
                    recv.read_u32()
                        .await
                        .context("Сервер не подтвердил приглашение")? as usize;
                ensure!(len <= 16384, "Некорректное приглашение");
                let mut bytes = vec![0; len];
                recv.read_exact(&mut bytes).await?;
                let invitation: Invitation = serde_json::from_slice(&bytes)?;

                ensure!(
                    invitation.owner_key.len() == 32,
                    "Некорректный ключ владельца"
                );
                ensure!(
                    recv.read_to_end(0).await?.is_empty(),
                    "Лишние данные приглашения"
                );

                stage = "открытие потока сессии";
                let (mut send, mut recv) = quic.open_bi().await?;
                send.write_all(&[3]).await?;

                ensure!(
                    recv.read_u8().await? == 3,
                    "Сервер не подтвердил запуск сессии"
                );
                Ok::<_, anyhow::Error>((quic, send, recv, invitation))
            });
            tokio::select! {
                result = dial => result.map_err(|_| {
                    let detail = if last_error.is_empty() { String::new() } else { format!(". Последняя ошибка маршрута: {last_error}") };
                    anyhow::anyhow!("Тайм-аут: {stage}{detail}")
                })?,
                _ = async {
                    while !cancel.load(Ordering::Relaxed) {
                        tokio::time::sleep(Duration::from_millis(50)).await;
                    }
                } => anyhow::bail!("Попытка подключения отменена"),
            }
        });
        let (quic, send, recv, invitation) = match setup {
            Ok(value) => value,
            Err(error) => {
                runtime.block_on(close_endpoint(&endpoint));
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
impl Drop for InternetGuest {
    fn drop(&mut self) {
        self.task.abort();
        self.runtime.block_on(close_endpoint(&self.endpoint));
    }
}
#[cfg(test)]
fn encode_code(id: EndpointId, token: &[u8; 16]) -> String {
    let mut bytes = id.as_bytes().to_vec();
    bytes.extend_from_slice(token);
    format!("TT3:{}", URL_SAFE_NO_PAD.encode(bytes))
}
pub fn parse_code(code: &str) -> Result<(EndpointId, [u8; 16])> {
    let (address, token) = parse_address_code(code)?;
    Ok((address.id, token))
}
fn encode_address_code(address: &EndpointAddr, token: &[u8; 16]) -> String {
    let bytes = serde_json::to_vec(&(address, token)).expect("serializable invitation");
    format!("TT4:{}", URL_SAFE_NO_PAD.encode(bytes))
}
fn parse_address_code(code: &str) -> Result<(EndpointAddr, [u8; 16])> {
    // Codes are often copied from a wrapped message and may contain line breaks.
    // Base64 decoders reject a newline anywhere in the payload, so strip all
    // whitespace before validating and decoding.
    let value: String = code.chars().filter(|c| !c.is_whitespace()).collect();
    ensure!(value.len() <= 8192, "Слишком длинный код сервера");
    if let Some(encoded) = value.strip_prefix("TT4:") {
        return Ok(serde_json::from_slice(&URL_SAFE_NO_PAD.decode(encoded)?)?);
    }
    ensure!(
        value.len() == 68 && value.starts_with("TT3:"),
        "Вставьте полный код сервера, начинающийся с TT4: или TT3:"
    );
    let bytes = URL_SAFE_NO_PAD.decode(&value[4..])?;
    ensure!(bytes.len() == 48, "Некорректный код");
    Ok((
        EndpointId::from_bytes(bytes[..32].try_into()?)?.into(),
        bytes[32..].try_into()?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn internet_invitation_rejects_local_only_and_excludes_private_addresses() {
        let rt = runtime().unwrap();
        let ep = rt.block_on(Endpoint::bind(presets::Minimal)).unwrap();
        assert!(internet_address(&ep.addr()).is_err());
        let address = ep
            .addr()
            .with_relay_url("https://relay.example.com".parse().unwrap());
        let public = internet_address(&address).unwrap();
        assert_eq!(public.id, address.id);
        assert_eq!(public.relay_urls().count(), 1);
        assert_eq!(public.ip_addrs().count(), 0);
        rt.block_on(ep.close());
    }

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
    fn routed_code_preserves_addresses_and_legacy_codes() {
        let rt = runtime().unwrap();
        let ep = rt.block_on(Endpoint::bind(presets::Minimal)).unwrap();
        let address = ep
            .addr()
            .with_relay_url("https://relay.example.com".parse().unwrap());
        let code = encode_address_code(&address, &[7; 16]);
        let (decoded, token) = parse_address_code(&code).unwrap();
        assert_eq!(decoded, address);
        assert_eq!(token, [7; 16]);
        // A code copied from a wrapped chat message may contain line breaks.
        let wrapped = format!("{}\r\n{}", &code[..100], &code[100..]);
        assert_eq!(parse_address_code(&wrapped).unwrap().1, [7; 16]);
        assert_eq!(parse_code(&code).unwrap().0, ep.id());
        assert!(parse_address_code("TT4:broken").is_err());
        assert!(parse_address_code(&format!("TT4:{}", "A".repeat(8192))).is_err());
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
