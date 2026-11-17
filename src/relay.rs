//! Delivery-only relay. No profile, message plaintext, or MLS secrets enter here.
use crate::{transport::Connection, wire::Packet};
use anyhow::Result;
use std::{
    collections::BTreeMap,
    net::{SocketAddr, TcpListener},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

pub struct Relay {
    pub owner: Connection,
    pub address: SocketAddr,
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}

struct Peer {
    connection: Connection,
    admitted: bool,
    requested: bool,
    since: std::time::Instant,
    last_activity: std::time::Instant,
}

impl Relay {
    pub fn start(address: SocketAddr) -> Result<Self> {
        // Establish the owner over a separate loopback listener before accepting outsiders.
        let control_listener = TcpListener::bind("127.0.0.1:0")?;
        let owner = Connection::connect(control_listener.local_addr()?)?;
        let (control, _) = control_listener.accept()?;
        let control = Connection::from_stream(control)?;
        let listener = TcpListener::bind(address)?;
        let address = listener.local_addr()?;
        listener.set_nonblocking(true)?;
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        let worker = thread::spawn(move || {
            let mut peers: BTreeMap<u64, Peer> = BTreeMap::new();
            let mut next_id = 1;
            while !stopped.load(Ordering::Relaxed) {
                if let Ok((stream, _)) = listener.accept() {
                    if let Ok(connection) = Connection::from_stream(stream) {
                        peers.insert(
                            next_id,
                            Peer {
                                connection,
                                admitted: false,
                                requested: false,
                                since: std::time::Instant::now(),
                                last_activity: std::time::Instant::now(),
                            },
                        );
                        next_id += 1;
                    }
                }
                let mut failed = Vec::new();
                // Owner traffic must not starve guest probes and voice processing.
                for _ in 0..32 {
                    let packet = match control.incoming.try_recv() {
                        Ok(p) => p,
                        Err(std::sync::mpsc::TryRecvError::Empty) => break,
                        Err(_) => {
                            stopped.store(true, Ordering::Relaxed);
                            break;
                        }
                    };
                    match packet {
                        Ok(Packet::Welcome { recipient, data }) => {
                            if let Some(peer) = peers.get_mut(&recipient) {
                                if peer.requested && !peer.admitted {
                                    peer.admitted = peer
                                        .connection
                                        .send(Packet::Welcome { recipient, data })
                                        .is_ok();
                                }
                            }
                        }
                        Ok(Packet::Disconnect { connection }) => {
                            peers.remove(&connection);
                        }
                        Ok(
                            packet @ (Packet::Commit { .. }
                            | Packet::Content { .. }
                            | Packet::Voice { .. }),
                        ) => {
                            for (&id, peer) in &peers {
                                if peer.admitted && peer.connection.send(packet.clone()).is_err() {
                                    failed.push(id);
                                }
                            }
                        }
                        Err(_) => {
                            stopped.store(true, Ordering::Relaxed);
                            break;
                        }
                        _ => {}
                    }
                }
                let mut forward = Vec::new();
                let mut departed = failed;
                for (&id, peer) in peers.iter_mut() {
                    // Guests send a session probe every three seconds. Do not
                    // leave a dead membership around for QUIC's PTO-dependent
                    // transport timeout after a process/computer disappears.
                    if (peer.admitted && peer.last_activity.elapsed() > Duration::from_secs(15))
                        || (!peer.admitted && peer.since.elapsed() > Duration::from_secs(60))
                    {
                        departed.push(id);
                        continue;
                    }
                    // Bound work per tick so a flooding peer cannot starve everyone.
                    for _ in 0..8 {
                        let packet = match peer.connection.incoming.try_recv() {
                            Ok(p) => p,
                            Err(std::sync::mpsc::TryRecvError::Empty) => break,
                            Err(_) => {
                                departed.push(id);
                                break;
                            }
                        };
                        if packet.is_ok() {
                            peer.last_activity = std::time::Instant::now();
                        }
                        match packet {
                            Ok(Packet::Join { package })
                                if !peer.requested && package.len() < 64 * 1024 =>
                            {
                                peer.requested = true;
                                if control
                                    .send(Packet::Pending {
                                        connection: id,
                                        package,
                                    })
                                    .is_err()
                                {
                                    departed.push(id);
                                }
                            }
                            Ok(packet @ (Packet::Content { .. } | Packet::Voice { .. }))
                                if peer.admitted =>
                            {
                                forward.push((id, packet))
                            }
                            _ => {
                                departed.push(id);
                                break;
                            }
                        }
                    }
                }
                for (sender, packet) in forward {
                    let _ = control.send(packet.clone());
                    for (&id, peer) in &peers {
                        if id != sender
                            && peer.admitted
                            && peer.connection.send(packet.clone()).is_err()
                        {
                            departed.push(id);
                        }
                    }
                }
                departed.sort_unstable();
                departed.dedup();
                for id in departed {
                    peers.remove(&id);
                    let _ = control.send(Packet::Disconnect { connection: id });
                }
                thread::sleep(Duration::from_millis(5));
            }
        });
        Ok(Self {
            owner,
            address,
            stop,
            worker: Some(worker),
        })
    }
}

impl Drop for Relay {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::Identity;
    #[test]
    fn relay_routes_ciphertext_and_owner_approves_membership() {
        let relay = Relay::start("127.0.0.1:0".parse().unwrap()).unwrap();
        let alice = Identity::new(b"alice").unwrap();
        let mut a = alice.create_group().unwrap();
        let bob = Identity::new(b"bob").unwrap();
        let client = Connection::connect(relay.address).unwrap();
        client
            .send(Packet::Join {
                package: bob.key_package().unwrap(),
            })
            .unwrap();
        let pending = relay
            .owner
            .incoming
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .unwrap();
        let Packet::Pending {
            connection,
            package,
        } = pending
        else {
            panic!("Expected join");
        };
        let (_, welcome) = alice.add(&mut a, &package).unwrap();
        relay
            .owner
            .send(Packet::Welcome {
                recipient: connection,
                data: welcome,
            })
            .unwrap();
        let received = client
            .incoming
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .unwrap();
        let Packet::Welcome { data, .. } = received else {
            panic!("Expected welcome");
        };
        let mut b = bob.join(&data).unwrap();
        client
            .send(Packet::Content {
                data: bob.encrypt(&mut b, b"secret").unwrap(),
            })
            .unwrap();
        let received = relay
            .owner
            .incoming
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .unwrap();
        let Packet::Content { data } = received else {
            panic!("Expected content");
        };
        assert_eq!(alice.receive(&mut a, &data).unwrap().unwrap(), b"secret");
    }
}
