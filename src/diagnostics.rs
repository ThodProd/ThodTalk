//! Bounded, in-memory diagnostics. Never records invitations or message payloads.
use eframe::egui;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, VecDeque},
    sync::{
        Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

#[derive(Clone, Serialize, Deserialize)]
pub struct Entry {
    pub section: String,
    pub ok: Option<bool>,
    pub detail: String,
}
#[derive(Default)]
struct State {
    current: BTreeMap<String, (Entry, Option<Instant>)>,
    history: VecDeque<(Entry, Instant)>,
}
static STATE: OnceLock<Mutex<State>> = OnceLock::new();
static WORKER: AtomicBool = AtomicBool::new(false);
static ENABLED: AtomicBool = AtomicBool::new(false);
static STARTED: AtomicBool = AtomicBool::new(false);
pub fn worker() {
    WORKER.store(true, Ordering::Relaxed);
}
pub fn report(section: impl Into<String>, ok: Option<bool>, detail: impl Into<String>) {
    let entry = Entry {
        section: section
            .into()
            .chars()
            .filter(|c| !c.is_control())
            .take(180)
            .collect(),
        ok,
        detail: detail
            .into()
            .chars()
            .filter(|c| !c.is_control())
            .take(1500)
            .collect(),
    };
    if WORKER.load(Ordering::Relaxed) {
        use std::io::Write;
        if let Ok(line) = serde_json::to_string(&entry) {
            let _ = writeln!(std::io::stderr().lock(), "{line}");
        }
    } else {
        ingest(entry);
    }
}
pub fn ingest(entry: Entry) {
    let mut state = STATE.get_or_init(Default::default).lock().unwrap();
    let now = Instant::now();
    let changed = state
        .current
        .get(&entry.section)
        .is_none_or(|(old, _)| old.ok != entry.ok || old.detail != entry.detail);
    if changed {
        state.history.push_back((entry.clone(), now));
        while state.history.len() > 250 {
            state.history.pop_front();
        }
    }
    state
        .current
        .insert(entry.section.clone(), (entry, Some(now)));
    while state.current.len() > 100 {
        let key = state.current.keys().next().cloned().unwrap();
        state.current.remove(&key);
    }
}
pub fn enable(enabled: bool, root: std::path::PathBuf) {
    ENABLED.store(enabled, Ordering::Relaxed);
    if !enabled || STARTED.swap(true, Ordering::Relaxed) {
        return;
    }
    std::thread::spawn(move || {
        let result = (|| -> anyhow::Result<()> {
            let dir = tempfile::tempdir_in(root)?;
            let store = crate::storage::Store::open(dir.path().into())?;
            store.save("diagnostic", &"test")?;
            anyhow::ensure!(
                store.load::<String>("diagnostic")?.as_deref() == Some("test"),
                "Проверка чтения не совпала"
            );
            Ok(())
        })();
        report("Компьютер / хранилище и DPAPI", Some(result.is_ok()), match result { Ok(()) => "Запись, шифрование и чтение временного файла проверены. Отдельная БД не требуется".into(), Err(e) => format!("{e:#}") });
        let tcp = std::net::TcpListener::bind("127.0.0.1:0");
        report(
            "Компьютер / локальный TCP",
            Some(tcp.is_ok()),
            match tcp {
                Ok(_) => "Локальный порт открывается".into(),
                Err(e) => e.to_string(),
            },
        );
        let udp = std::net::UdpSocket::bind("0.0.0.0:0");
        report(
            "Компьютер / UDP",
            Some(udp.is_ok()),
            match udp {
                Ok(_) => {
                    "UDP-сокет открывается; прохождение NAT/брандмауэра этим не доказано".into()
                }
                Err(e) => e.to_string(),
            },
        );
        let (inputs, outputs) = crate::audio::devices();
        report(
            "Компьютер / звук",
            Some(!inputs.is_empty() && !outputs.is_empty()),
            format!(
                "Устройств ввода: {}, вывода: {}. Доступ к захвату проверяется при входе в голос",
                inputs.len(),
                outputs.len()
            ),
        );
        let urls = iroh::RelayMode::Default.relay_map().urls::<Vec<_>>();
        for url in urls {
            std::thread::spawn(move || {
                let client = reqwest::blocking::Client::builder()
                    .timeout(Duration::from_secs(8))
                    .redirect(reqwest::redirect::Policy::none())
                    .build();
                loop {
                    if ENABLED.load(Ordering::Relaxed) {
                        let start = Instant::now();
                        let result = match &client {
                            Ok(client) => client
                                .get(url.as_str())
                                .send()
                                .map(|r| r.status())
                                .map_err(|e| e.to_string()),
                            Err(e) => Err(e.to_string()),
                        };
                        let (ok, text) = match result {
                            Ok(status) => (
                                true,
                                format!(
                                    "Ответ сайта (HTTPS) {} мс · HTTP {}",
                                    start.elapsed().as_millis(),
                                    status
                                ),
                            ),
                            Err(e) => (
                                false,
                                format!(
                                    "Сайт недоступен (HTTPS) · {e}"
                                ),
                            ),
                        };
                        report(format!("Relay HTTPS / {url}"), Some(ok), text);
                        if ENABLED.load(Ordering::Relaxed) {
                            let result = crate::internet::diagnostic_relay(url.clone());
                            report(
                                format!("Relay обмен / {url}"),
                                Some(result.is_ok()),
                                match result {
                                    Ok(ms) => format!(
                                        "Тест передачи через relay (QUIC) · {ms} мс"
                                    ),
                                    Err(e) => format!(
                                        "Проверка передачи через relay (QUIC) не прошла · {e:#}"
                                    ),
                                },
                            );
                        }
                    }
                    std::thread::sleep(Duration::from_secs(60));
                }
            });
        }
    });
}
fn status(entry: &Entry) -> &'static str {
    match entry.ok {
        Some(true) => "Работает",
        Some(false) => "Есть проблема",
        None => "Событие",
    }
}

fn heading(section: &str) -> String {
    for (prefix, label) in [
        ("Relay HTTPS / ", "Ответ сайта relay (HTTPS)"),
        ("Relay обмен / ", "Передача через relay (QUIC)"),
        ("QUIC проверка / ", "Выбор маршрута сервера (QUIC)"),
        ("Участник QUIC / ", "Связь с участником (QUIC)"),
    ] {
        if let Some(id) = section.strip_prefix(prefix) {
            let id = id.strip_prefix("https://").unwrap_or(id).trim_end_matches('/');
            return format!("{label} · {id}");
        }
    }
    match section {
        "MLS / сессия" => "Защищённая комната (MLS)".into(),
        "Worker" => "Сетевой процесс (worker)".into(),
        _ => section.into(),
    }
}

pub fn window(ctx: &egui::Context, open: &mut bool) {
    if !*open {
        return;
    }
    ctx.show_viewport_immediate(egui::ViewportId::from_hash_of("debug-terminal"), egui::ViewportBuilder::default().with_title("Диагностика ThodTalk").with_inner_size([780.0, 540.0]), |ctx, _| {
        if ctx.input(|i| i.viewport().close_requested()) { *open = false; }
        egui::CentralPanel::default().frame(egui::Frame::new().fill(egui::Color32::from_rgb(15, 20, 30)).inner_margin(18)).show(ctx, |ui| {
            ui.visuals_mut().override_text_color = Some(egui::Color32::from_rgb(215, 225, 235));
            let state = STATE.get_or_init(Default::default).lock().unwrap();
            ui.horizontal(|ui| {
                ui.heading("Диагностика связи");
                if ui.button("Скопировать всё").clicked() {
                    use std::fmt::Write;
                    let mut text = format!("ThodTalk {} · диагностика\n\nТекущие проверки\n", env!("CARGO_PKG_VERSION"));
                    for (entry, time) in state.current.values() {
                        let age = time.map(|t| t.elapsed().as_secs()).unwrap_or_default();
                        let _ = writeln!(text, "−{age} с · {} · {} · {}", status(entry), heading(&entry.section), entry.detail);
                    }
                    text.push_str("\nЖурнал\n");
                    for (entry, time) in state.history.iter().rev() {
                        let _ = writeln!(text, "−{} с · {} · {} · {}", time.elapsed().as_secs(), status(entry), heading(&entry.section), entry.detail);
                    }
                    ctx.copy_text(text);
                }
            });
            ui.label("HTTPS — ответ сайта · QUIC — передача данных · RTT — время туда и обратно");
            ui.separator();
            egui::ScrollArea::vertical().show(ui, |ui| {
                for (entry, time) in state.current.values() {
                    let age = time.map(|t| t.elapsed().as_secs()).unwrap_or(0);
                    let stale = age > if entry.section.starts_with("Relay") { 120 } else { 15 };
                    let color = if stale { egui::Color32::GRAY } else { match entry.ok { Some(true) => egui::Color32::from_rgb(85, 220, 150), Some(false) => egui::Color32::from_rgb(255, 115, 105), None => egui::Color32::from_rgb(245, 200, 95) } };
                    ui.horizontal_wrapped(|ui| {
                        ui.label(egui::RichText::new(heading(&entry.section)).strong());
                        ui.label(egui::RichText::new(if stale { "Устарело" } else { status(entry) }).color(color));
                        ui.label(egui::RichText::new(format!("{age} с назад")).weak());
                    });
                    ui.label(&entry.detail);
                    ui.add_space(5.0);
                }
                ui.separator();
                ui.collapsing(format!("Журнал · {}", state.history.len()), |ui| {
                    for (entry, time) in state.history.iter().rev() {
                        ui.label(format!("−{} с · {} · {}", time.elapsed().as_secs(), heading(&entry.section), entry.detail));
                    }
                });
            });
        });
        ctx.request_repaint_after(Duration::from_secs(1));
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn history_is_bounded_and_control_characters_removed() {
        for i in 0..400 {
            report("test", Some(true), format!("line\n{i}"));
        }
        let state = STATE.get().unwrap().lock().unwrap();
        assert!(state.history.len() <= 250);
        assert!(!state.current["test"].0.detail.contains('\n'));
    }
}

#[cfg(test)]
pub(crate) fn has_section(prefix: &str) -> bool {
    STATE
        .get_or_init(Default::default)
        .lock()
        .unwrap()
        .current
        .keys()
        .any(|key| key.starts_with(prefix))
}
