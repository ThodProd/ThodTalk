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
                                    "DNS + TCP + TLS + HTTP: {} мс, HTTP {}. Это время HTTPS-ответа, не игровой ping и не проверка QUIC",
                                    start.elapsed().as_millis(),
                                    status
                                ),
                            ),
                            Err(e) => (
                                false,
                                format!(
                                    "HTTPS недоступен: {e}. Проверьте интернет, DNS, VPN и фильтрацию HTTPS; причина блокировки не установлена"
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
                                        "QUIC через relay: ответ на контрольный пакет за {ms} мс. Два тестовых endpoint на этом ПК, без прямого маршрута; это не RTT до друга"
                                    ),
                                    Err(e) => format!(
                                        "Двусторонний QUIC-обмен не подтверждён: {e:#}. HTTPS может работать при неработающей передаче relay"
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
pub fn window(ctx: &egui::Context, open: &mut bool) {
    if !*open {
        return;
    }
    ctx.show_viewport_immediate(egui::ViewportId::from_hash_of("debug-terminal"), egui::ViewportBuilder::default().with_title("ThodTalk — терминал отладки").with_inner_size([960.0, 760.0]), |ctx, _| {
        if ctx.input(|i| i.viewport().close_requested()) { *open = false; }
        egui::CentralPanel::default().frame(egui::Frame::new().fill(egui::Color32::from_rgb(15, 20, 30)).inner_margin(18)).show(ctx, |ui| {
            ui.visuals_mut().override_text_color = Some(egui::Color32::from_rgb(215, 225, 235));
            ui.heading("THODTALK  /  ТЕРМИНАЛ ОТЛАДКИ");
            ui.label("Живые измерения • Relay каждые 60–100 с • QUIC каждые 3 с • журнал в памяти");
            ui.label("OK подтверждает только указанную проверку. Устаревшие измерения не подтверждают текущую связь.");
            ui.label("Высокий RTT: сравните прямой/relay маршрут и HTTPS. Дальний relay, VPN, Wi-Fi и очереди трафика — возможные причины, не установленный диагноз.");
            let state = STATE.get_or_init(Default::default).lock().unwrap();
            egui::ScrollArea::vertical().show(ui, |ui| {
                for (entry, time) in state.current.values() {
                    let age = time.map(|t| t.elapsed().as_secs()).unwrap_or(0);
                    let stale = age > if entry.section.starts_with("Relay") { 120 } else { 15 };
                    let color = if stale { egui::Color32::GRAY } else { match entry.ok { Some(true) => egui::Color32::from_rgb(85, 220, 150), Some(false) => egui::Color32::from_rgb(255, 115, 105), None => egui::Color32::from_rgb(245, 200, 95) } };
                    egui::Frame::new().fill(egui::Color32::from_rgb(25, 33, 46)).inner_margin(10).corner_radius(6).show(ui, |ui| {
                        ui.set_min_width(ui.available_width());
                        ui.label(egui::RichText::new(format!("{}  {}  • {} с назад", if stale { "СТАРЫЕ ДАННЫЕ" } else { match entry.ok { Some(true) => "OK", Some(false) => "ОШИБКА / ВНИМАНИЕ", None => "ЭТАП" } }, entry.section, age)).color(color).strong());
                        ui.label(&entry.detail);
                    });
                    ui.add_space(6.0);
                }
                ui.separator(); ui.heading("Хронология (последние 250 событий)");
                for (entry, time) in state.history.iter().rev() {
                    ui.label(format!("−{} с  [{}] {}", time.elapsed().as_secs(), entry.section, entry.detail));
                }
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
