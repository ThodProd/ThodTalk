use serde_json::{Value, json};
use std::{
    io::{BufRead, BufReader, Write},
    process::{Child, ChildStdin, Command, Stdio},
    sync::mpsc,
    time::{Duration, Instant},
};
struct Client {
    child: Child,
    input: ChildStdin,
    events: mpsc::Receiver<Value>,
    state: Value,
    voices: usize,
}
impl Client {
    fn start(path: &std::path::Path, name: &str, hosting: bool, code: &str) -> Self {
        let executable = std::env::var_os("THODTALK_TEST_EXE")
            .unwrap_or_else(|| env!("CARGO_BIN_EXE_thodtalk").into());
        let mut command = Command::new(executable);
        command
            .arg("--session-worker")
            .env("THODHUB_DATA_DIR", path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000);
        }
        let mut child = command.spawn().unwrap();
        let input = child.stdin.take().unwrap();
        let output = child.stdout.take().unwrap();
        let (tx, events) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(output).lines() {
                let Ok(line) = line else {
                    break;
                };
                let Ok(value) = serde_json::from_str(&line) else {
                    break;
                };
                if tx.send(value).is_err() {
                    break;
                }
            }
        });
        let mut client = Self {
            child,
            input,
            events,
            state: Value::Null,
            voices: 0,
        };
        let profile = thodtalk::storage::Profile {
            name: name.into(),
            network_allowed: true,
            ..Default::default()
        };
        client.send(json!({"Start":{"hosting":hosting,"profile":profile,"code":code}}));
        client
    }
    fn send(&mut self, v: Value) {
        writeln!(self.input, "{v}").unwrap();
        self.input.flush().unwrap();
    }
    fn poll(&mut self) {
        while let Ok(v) = self.events.try_recv() {
            assert!(v.get("Fatal").is_none(), "{}", v["Fatal"]);
            assert!(v.get("Error").is_none(), "{}", v["Error"]);
            if let Some(s) = v.get("State") {
                self.state = s.clone();
            }
            if let Some(frames) = v.get("Voice").and_then(Value::as_array) {
                self.voices += frames.len();
            }
        }
    }
    fn members(&self) -> usize {
        self.state["members"].as_array().map_or(0, Vec::len)
    }
    fn messages(&self) -> usize {
        self.state["history"].as_array().map_or(0, Vec::len)
    }
}
impl Drop for Client {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
fn wait(clients: &mut [Client], seconds: u64, done: impl Fn(&[Client]) -> bool) {
    let end = Instant::now() + Duration::from_secs(seconds);
    loop {
        for c in clients.iter_mut() {
            c.poll();
        }
        if done(clients) {
            return;
        }
        assert!(
            Instant::now() < end,
            "Истёк срок ожидания состояния клиентов"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}
#[test]
#[ignore = "Проверка публичного маршрута тремя процессами готового EXE, без LAN discovery"]
fn packaged_workers_public_chat_voice_and_kick() {
    let dir = tempfile::tempdir().unwrap();
    let mut clients = vec![Client::start(
        &dir.path().join("host"),
        "Владелец",
        true,
        "",
    )];
    wait(&mut clients, 30, |cs| {
        cs[0].state["code"].as_str().is_some_and(|s| !s.is_empty())
    });
    let code = clients[0].state["code"].as_str().unwrap().to_owned();
    let connecting = Instant::now();
    clients.push(Client::start(&dir.path().join("a"), "Друг А", false, &code));
    clients.push(Client::start(&dir.path().join("b"), "Друг Б", false, &code));
    wait(&mut clients, 35, |cs| cs.iter().all(|c| c.members() == 3));
    eprintln!("Two guests connected in {:?}", connecting.elapsed());
    assert_eq!(clients[0].state["hosting"], true);
    assert_eq!(clients[1].state["hosting"], false);
    assert_eq!(clients[2].state["hosting"], false);
    clients[1].send(json!({"Chat":{"channel":"Общий","text":"Проверка публичного маршрута"}}));
    wait(&mut clients, 15, |cs| cs.iter().all(|c| c.messages() == 1));
    let mut encoder =
        opus::Encoder::new(48_000, opus::Channels::Mono, opus::Application::Voip).unwrap();
    let samples: Vec<i16> = (0..960)
        .map(|n| ((n as f32 * 0.0576).sin() * 10000.0) as i16)
        .collect();
    let mut packet = vec![0; 4000];
    let size = encoder.encode(&samples, &mut packet).unwrap();
    packet.truncate(size);
    clients[1].send(json!({"Payload":{"Voice":{"channel":"Общий","data":packet.clone()}}}));
    wait(&mut clients, 15, |cs| cs[0].voices > 0 && cs[2].voices > 0);
    clients[0].send(json!({"Payload":{"Voice":{"channel":"Общий","data":packet}}}));
    wait(&mut clients, 15, |cs| cs[1].voices > 0 && cs[2].voices > 1);
    clients[0].send(json!({"Closed":true}));
    let key = clients[1].state["key"].clone();
    clients[0].send(json!({"Kick":key}));
    clients.remove(1);
    wait(&mut clients, 15, |cs| cs.iter().all(|c| c.members() == 2));
    clients[1].send(json!({"Chat":{"channel":"Игры","text":"После кика"}}));
    wait(&mut clients, 15, |cs| cs.iter().all(|c| c.messages() == 2));
    assert_eq!(clients[0].state["code"].as_str().unwrap(), code);
    assert_eq!(clients[0].state["closed"], true);
}

#[test]
#[ignore = "Public relay: repeated joins with the same persisted identity"]
fn public_rejoin_after_shutdown_and_worker_restart() {
    let dir = tempfile::tempdir().unwrap();
    let mut clients = vec![Client::start(&dir.path().join("host"), "Host", true, "")];
    wait(&mut clients, 35, |cs| {
        cs[0].state["code"].as_str().is_some_and(|s| !s.is_empty())
    });
    let code = clients[0].state["code"].as_str().unwrap().to_owned();
    let mut key = Value::Null;
    for cycle in 0..4 {
        eprintln!("Rejoin cycle {cycle}");
        clients.push(Client::start(
            &dir.path().join("guest"),
            "Guest",
            false,
            &code,
        ));
        wait(&mut clients, 35, |cs| cs.iter().all(|c| c.members() == 2));
        if cycle == 0 {
            key = clients[1].state["key"].clone();
        }
        assert_eq!(
            clients[1].state["key"], key,
            "Must reconnect with the same identity"
        );
        clients[1].send(json!({"Chat":{"channel":"Общий","text":format!("Rejoin {cycle}")}}));
        wait(&mut clients, 10, |cs| cs[0].messages() == cycle + 1);
        if cycle % 2 == 0 {
            clients[1].send(json!("Shutdown"));
            let deadline = Instant::now() + Duration::from_secs(3);
            while clients[1].child.try_wait().unwrap().is_none() {
                assert!(Instant::now() < deadline, "Worker did not shut down");
                std::thread::sleep(Duration::from_millis(20));
            }
        }
        clients.pop(); // Odd cycles emulate an abrupt process restart.
        eprintln!("Waiting for departed member cleanup after cycle {cycle}");
        wait(&mut clients, 20, |cs| cs[0].members() == 1);
        assert_eq!(clients[0].state["code"], code);
    }
}
