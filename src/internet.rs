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

const ALPN: &[u8] = b"thodtalk/2";
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
        let endpoint = runtime.block_on(
            Endpoint::builder(presets::N0)
                .alpns(vec![ALPN.to_vec()])
                .bind(),
        )?;
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
                let Ok(slot) = slots.clone().try_acquire_owned() else {incoming.refuse(); continue;};
                let gate = gate.clone(); let info = info.clone();
                tokio::spawn(async move {
                    let _slot = slot;
                    let incoming_result = tokio::time::timeout(Duration::from_secs(60),incoming).await;
                    #[cfg(debug_assertions)] if let Err(error)=&incoming_result {eprintln!("QUIC: рукопожатие: {error}");}
                    #[cfg(debug_assertions)] if let Ok(Err(error))=&incoming_result {eprintln!("QUIC: рукопожатие отклонено: {error}");}
                    let Ok(Ok(connection)) = incoming_result else {return;};
                    let setup = tokio::time::timeout(Duration::from_secs(30),async {
                        ensure!(!gate.load(Ordering::SeqCst),"Вход закрыт");
                        let (mut send,mut recv) = connection.accept_bi().await?;
                        #[cfg(debug_assertions)] eprintln!("QUIC: поток приглашения открыт");
                        let mut supplied = [0u8;16]; recv.read_exact(&mut supplied).await?;
                        ensure!(bool::from(token.ct_eq(&supplied)),"Неверное приглашение");
                        ensure!(!gate.load(Ordering::SeqCst),"Вход закрыт");
                        let invitation = info.read().ok().and_then(|i| i.clone()).context("Сервер запускается")?;
                        let bytes = serde_json::to_vec(&invitation)?;
                        send.write_u32(bytes.len() as u32).await?; send.write_all(&bytes).await?;
                        send.finish()?;
                        // Close the bootstrap stream before opening the framed message stream.
                        let (send,recv) = connection.accept_bi().await?;
                        let tcp = tokio::net::TcpStream::connect(address).await?;
                        Ok::<_,anyhow::Error>((send,recv,tcp))
                    }).await;
                    #[cfg(debug_assertions)]
                    if let Err(error) = &setup {eprintln!("QUIC: ожидание входа: {error}");}
                    #[cfg(debug_assertions)]
                    if let Ok(Err(error)) = &setup {eprintln!("QUIC: вход отклонён: {error:#}");}
                    if let Ok(Ok((mut send,mut recv,tcp))) = setup {
                        let (mut read,mut write) = tcp.into_split();
                        tokio::select! {_ = tokio::io::copy(&mut read,&mut send) => {},_ = tokio::io::copy(&mut recv,&mut write) => {}}
                    }
                    connection.close(0u8.into(),b"closed");
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
        let mut last = None;
        for attempt in 0..3 {
            let runtime = runtime()?;
            let builder = Endpoint::builder(presets::N0);
            let builder = if attempt > 0 {
                builder.clear_ip_transports()
            } else {
                builder
            };
            let endpoint = runtime.block_on(builder.bind())?;
            let local = if std::env::var_os("THODTALK_TEST_PUBLIC_ONLY").is_some() {
                None
            } else {
                crate::lan::find(id)
            };
            if local.is_none() {
                if let Err(error) = runtime.block_on(tokio_wait_online(&endpoint)) {
                    runtime.block_on(endpoint.close());
                    last = Some(error);
                    continue;
                }
            }
            match Self::join_with(runtime, endpoint, local.unwrap_or_else(|| id.into()), token) {
                Ok(guest) => return Ok(guest),
                Err(error) => last = Some(error),
            }
        }
        Err(last.unwrap()).context("Не удалось подключиться после трёх попыток")
    }
    fn join_with(
        runtime: Arc<Runtime>,
        endpoint: Endpoint,
        address: EndpointAddr,
        token: [u8; 16],
    ) -> Result<Self> {
        let (quic, mut send, mut recv, invitation) = runtime.block_on(async {
            tokio::time::timeout(Duration::from_secs(35), async {
                // QUIC authenticates the exact public key embedded in the invitation.
                let quic = loop {
                    match endpoint.connect(address.clone(), ALPN).await {
                        Ok(connection) => break connection,
                        Err(_) => tokio::time::sleep(Duration::from_millis(750)).await,
                    }
                };
                let (mut send, mut recv) = quic.open_bi().await?;
                #[cfg(debug_assertions)]
                eprintln!("QUIC: клиент отправляет приглашение");
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
                let (send, recv) = quic.open_bi().await?;
                Ok::<_, anyhow::Error>((quic, send, recv, invitation))
            })
            .await
            .context("Сервер недоступен. Проверьте код, сеть и открыт ли вход")?
        })?;
        let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
        let connection = Connection::connect(listener.local_addr()?)?;
        let (tcp, _) = listener.accept()?;
        tcp.set_nonblocking(true)?;
        let task = runtime.spawn(async move {
            let Ok(tcp) = tokio::net::TcpStream::from_std(tcp) else {return;};
            let (mut read,mut write) = tcp.into_split();
            tokio::select! {_ = tokio::io::copy(&mut read,&mut send) => {},_ = tokio::io::copy(&mut recv,&mut write) => {}}
            quic.close(0u8.into(),b"closed");
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
    tokio::time::timeout(Duration::from_secs(20), endpoint.online())
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
    format!("TT2:{}", URL_SAFE_NO_PAD.encode(bytes))
}
pub fn parse_code(code: &str) -> Result<(EndpointId, [u8; 16])> {
    let value = code.trim();
    ensure!(
        value.len() == 68 && value.starts_with("TT2:"),
        "Вставьте полный код сервера, начинающийся с TT2:"
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
