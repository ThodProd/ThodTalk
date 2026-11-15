use std::{
    sync::mpsc,
    time::{Duration, Instant},
};
use thodtalk::{
    session::Session,
    storage::{Profile, Store},
};

fn profile(name: &str) -> Profile {
    Profile {
        name: name.into(),
        network_allowed: true,
        ..Default::default()
    }
}

#[test]
#[ignore = "Uses public rendezvous and relay infrastructure; run explicitly"]
fn shared_code_two_guests_chat_and_kick() {
    if let Ok(path) = std::env::var("THODTALK_TEST_TRACE") {
        let file = std::fs::File::create(path).unwrap();
        let _ = tracing_subscriber::fmt()
            .with_env_filter("iroh=debug,iroh::socket::transports::relay=trace,iroh_relay=trace,noq=debug")
            .with_ansi(false)
            .with_writer(std::sync::Mutex::new(file))
            .try_init();
    }
    let dir = tempfile::tempdir().unwrap();
    let mut host = Session::host(
        Store::open(dir.path().join("host")).unwrap(),
        profile("Владелец"),
    )
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    while host.code.is_empty() && Instant::now() < deadline {
        host.poll().unwrap();
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(!host.code.is_empty(), "No server code: {}", host.status);
    let original_code = host.code.clone();
    let (tx, rx) = mpsc::channel();
    for name in ["Друг А", "Друг Б"] {
        let tx = tx.clone();
        let code = host.code.clone();
        let root = dir.path().join(name);
        std::thread::spawn(move || {
            let result = Session::join(Store::open(root).unwrap(), profile(name), &code);
            let _ = tx.send(result);
        });
    }
    drop(tx);
    let mut guests = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(180);
    while Instant::now() < deadline {
        host.poll().unwrap();
        while let Ok(result) = rx.try_recv() {
            guests.push(result.unwrap());
        }
        for guest in &mut guests {
            guest.poll().unwrap();
        }
        if guests.len() == 2
            && guests.iter().all(|g| g.ready() && g.members.len() == 3)
            && host.members.len() == 3
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(guests.len(), 2);
    assert_eq!(host.members.len(), 3);
    assert_eq!(host.code, original_code);
    guests[0]
        .chat("Общий", "Проверка шифрованной переписки")
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        host.poll().unwrap();
        for guest in &mut guests {
            guest.poll().unwrap();
        }
        if host.history.messages.len() == 1 && guests[1].history.messages.len() == 1 {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(host.history.messages.len(), 1);
    assert_eq!(guests[1].history.messages.len(), 1);
    host.set_closed(true);
    assert!(host.closed);
    let removed = guests[0].own_key();
    host.kick(&removed).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        host.poll().unwrap();
        guests[1].poll().unwrap();
        if guests[1].members.len() == 2 {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(!host.members.contains_key(&removed));
    assert_eq!(guests[1].members.len(), 2);
    guests[1]
        .chat("Игры", "Вход закрыт, но наш чат работает")
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while host.history.messages.len() < 2 && Instant::now() < deadline {
        host.poll().unwrap();
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(host.history.messages.len(), 2);
    assert_eq!(host.code, original_code);
}
