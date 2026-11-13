use anyhow::{Result, ensure};
use std::{net::{TcpStream, SocketAddr, Shutdown}, sync::mpsc::{self, Receiver, SyncSender}, thread, time::{Duration, Instant}};
use crate::wire::{read_packet, write_packet, Packet};

/// A bounded connection. A stalled remote cannot block the UI thread.
pub struct Connection {
    outgoing: Option<SyncSender<Packet>>,
    pub incoming: Receiver<Result<Packet>>,
    socket: TcpStream,
}

impl Connection {
    pub fn connect(address: SocketAddr) -> Result<Self> {
        Self::from_stream(TcpStream::connect_timeout(&address, Duration::from_secs(8))?)
    }
    pub fn from_stream(socket: TcpStream) -> Result<Self> {
        socket.set_nonblocking(false)?;
        socket.set_nodelay(true)?;
        socket.set_read_timeout(Some(Duration::from_secs(60)))?;
        socket.set_write_timeout(Some(Duration::from_secs(8)))?;
        let mut read = socket.try_clone()?;
        let mut write = socket.try_clone()?;
        let (incoming_tx, incoming) = mpsc::sync_channel(32);
        let (outgoing, outgoing_rx) = mpsc::sync_channel::<Packet>(32);
        thread::spawn(move || {
            let mut window = Instant::now(); let mut count = 0;
            loop {
                let packet = read_packet(&mut read);
                let failed = packet.is_err();
                if window.elapsed() >= Duration::from_secs(1) {window = Instant::now(); count = 0;}
                count += 1;
                if count > 100 { let _ = incoming_tx.try_send(Err(anyhow::anyhow!("Превышена частота пакетов"))); break; }
                if matches!(packet, Ok(Packet::Ping)) {continue;}
                if incoming_tx.try_send(packet).is_err() || failed {break;}
            }
            let _ = read.shutdown(Shutdown::Both);
        });
        thread::spawn(move || {
            loop {
                let packet = match outgoing_rx.recv_timeout(Duration::from_secs(15)) {
                    Ok(packet) => packet,
                    Err(mpsc::RecvTimeoutError::Timeout) => Packet::Ping,
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                };
                if write_packet(&mut write, &packet).is_err() {break;}
            }
            let _ = write.shutdown(Shutdown::Both);
        });
        Ok(Self { outgoing: Some(outgoing), incoming, socket })
    }
    pub fn send(&self, packet: Packet) -> Result<()> {
        ensure!(self.outgoing.is_some(), "Соединение закрыто");
        self.outgoing.as_ref().unwrap().try_send(packet).map_err(|_| anyhow::anyhow!("Соединение закрыто или очередь переполнена"))
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        self.outgoing.take();
        let _ = self.socket.shutdown(Shutdown::Both);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::Identity;
    use std::net::TcpListener;
    #[test]
    fn two_clients_exchange_mls_over_real_tcp() {
        let alice = Identity::new(b"alice").unwrap();
        let bob = Identity::new(b"bob").unwrap();
        let mut group_a = alice.create_group().unwrap();
        let (_, welcome) = alice.add(&mut group_a, &bob.key_package().unwrap()).unwrap();
        let mut group_b = bob.join(&welcome).unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let client = Connection::connect(listener.local_addr().unwrap()).unwrap();
        let (stream, _) = listener.accept().unwrap();
        let server = Connection::from_stream(stream).unwrap();
        client.send(Packet::Content {data: alice.encrypt(&mut group_a, "Привет, друг!".as_bytes()).unwrap()}).unwrap();
        let packet = server.incoming.recv_timeout(Duration::from_secs(5)).unwrap().unwrap();
        let Packet::Content {data} = packet else {panic!("Unexpected packet");};
        assert_eq!(bob.receive(&mut group_b, &data).unwrap().unwrap(), "Привет, друг!".as_bytes());
        server.send(Packet::Content {data: bob.encrypt(&mut group_b, b"hello back").unwrap()}).unwrap();
        let packet = client.incoming.recv_timeout(Duration::from_secs(5)).unwrap().unwrap();
        let Packet::Content {data} = packet else {panic!("Unexpected packet");};
        assert_eq!(alice.receive(&mut group_a, &data).unwrap().unwrap(), b"hello back");
    }
}
