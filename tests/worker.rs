use std::{
    io::{BufRead, BufReader, Write},
    process::{Command, Stdio},
    time::{Duration, Instant},
};
#[test]
fn worker_rejects_invalid_invitation_and_exits_on_eof() {
    let dir = tempfile::tempdir().unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_thodtalk"))
        .arg("--session-worker")
        .env("THODHUB_DATA_DIR", dir.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let profile = thodtalk::storage::Profile {
        name: "Тест".into(),
        network_allowed: true,
        ..Default::default()
    };
    let mut input = child.stdin.take().unwrap();
    writeln!(
        input,
        "{}",
        serde_json::json!({"Start":{"hosting":false,"profile":profile,"code":"invalid"}})
    )
    .unwrap();
    input.flush().unwrap();
    let mut output = BufReader::new(child.stdout.take().unwrap());
    let mut line = String::new();
    output.read_line(&mut line).unwrap();
    let event: serde_json::Value = serde_json::from_str(&line).unwrap();
    assert!(event.get("Fatal").is_some());
    drop(input);
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if child.try_wait().unwrap().is_some() {
            break;
        }
        if Instant::now() > deadline {
            child.kill().unwrap();
            panic!("Worker survived stdin EOF");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
#[ignore = "Starts an active session through public relays"]
fn active_worker_exits_on_shutdown_and_parent_pipe_loss() {
    for graceful in [true, false] {
        let dir = tempfile::tempdir().unwrap();
        let mut command = Command::new(env!("CARGO_BIN_EXE_thodtalk"));
        command
            .arg("--session-worker")
            .env("THODHUB_DATA_DIR", dir.path())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000);
        }
        let mut child = command.spawn().unwrap();
        let mut input = child.stdin.take().unwrap();
        let output = child.stdout.take().unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut reader = BufReader::new(output);
            let mut line = String::new();
            let _ = reader.read_line(&mut line);
            let _ = tx.send(line);
            // Keep stdout open so this test specifically exercises stdin/Shutdown.
            let mut line = String::new();
            while reader.read_line(&mut line).is_ok_and(|n| n > 0) {
                line.clear();
            }
        });
        let profile = thodtalk::storage::Profile {
            name: "Lifecycle".into(),
            network_allowed: true,
            ..Default::default()
        };
        writeln!(
            input,
            "{}",
            serde_json::json!({"Start":{"hosting":true,"profile":profile,"code":""}})
        )
        .unwrap();
        input.flush().unwrap();
        let state = rx.recv_timeout(Duration::from_secs(20));
        if state.is_err() {
            let _ = child.kill();
            let _ = child.wait();
        }
        assert!(state.unwrap().contains("State"));
        if graceful {
            writeln!(input, "\"Shutdown\"").unwrap();
            input.flush().unwrap();
        }
        drop(input);
        let deadline = Instant::now() + Duration::from_secs(3);
        while child.try_wait().unwrap().is_none() {
            if Instant::now() > deadline {
                let _ = child.kill();
                let _ = child.wait();
                panic!("Active worker survived shutdown (graceful={graceful})");
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}
