//! LAN discovery sends public endpoint addresses only. QUIC still authenticates the
//! endpoint key from the invitation; the invitation secret is never broadcast.
use iroh::{Endpoint, EndpointAddr, EndpointId};
use std::{
    net::{Ipv4Addr, SocketAddrV4, UdpSocket},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
const PORT: u16 = 42317;
const GROUP: Ipv4Addr = Ipv4Addr::new(239, 255, 77, 77);
pub struct Discovery {
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}
impl Discovery {
    pub fn start(endpoint: Endpoint) -> anyhow::Result<Self> {
        let socket = socket2::Socket::new(
            socket2::Domain::IPV4,
            socket2::Type::DGRAM,
            Some(socket2::Protocol::UDP),
        )?;
        socket.set_reuse_address(true)?;
        socket.bind(&SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, PORT).into())?;
        let socket: UdpSocket = socket.into();
        let _ = socket.join_multicast_v4(&GROUP, &Ipv4Addr::UNSPECIFIED);
        socket.set_read_timeout(Some(Duration::from_millis(200)))?;
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let thread = std::thread::spawn(move || {
            let mut buffer = [0; 64];
            let mut start = Instant::now();
            let mut requests = 0;
            while !flag.load(Ordering::Relaxed) {
                let Ok((length, peer)) = socket.recv_from(&mut buffer) else {
                    continue;
                };
                if start.elapsed() > Duration::from_secs(1) {
                    start = Instant::now();
                    requests = 0;
                }
                requests += 1;
                if requests > 25
                    || length != 36
                    || &buffer[..4] != b"TTD2"
                    || &buffer[4..36] != endpoint.id().as_bytes()
                {
                    continue;
                }
                if let Ok(bytes) = serde_json::to_vec(&endpoint.addr()) {
                    if bytes.len() < 4096 {
                        let _ = socket.send_to(&bytes, peer);
                    }
                }
            }
        });
        Ok(Self {
            stop,
            thread: Some(thread),
        })
    }
}
impl Drop for Discovery {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
pub fn find(id: EndpointId) -> Option<EndpointAddr> {
    let socket = UdpSocket::bind("0.0.0.0:0").ok()?;
    socket
        .set_read_timeout(Some(Duration::from_millis(100)))
        .ok()?;
    let _ = socket.set_broadcast(true);
    let mut query = b"TTD2".to_vec();
    query.extend_from_slice(id.as_bytes());
    for target in [GROUP, Ipv4Addr::BROADCAST, Ipv4Addr::LOCALHOST] {
        let _ = socket.send_to(&query, SocketAddrV4::new(target, PORT));
    }
    let deadline = Instant::now() + Duration::from_millis(350);
    let mut buffer = [0; 4096];
    while Instant::now() < deadline {
        if let Ok((len, _)) = socket.recv_from(&mut buffer) {
            if let Ok(address) = serde_json::from_slice::<EndpointAddr>(&buffer[..len]) {
                if address.id == id {
                    return Some(address);
                }
            }
        }
    }
    None
}
