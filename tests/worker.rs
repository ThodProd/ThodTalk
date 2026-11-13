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
