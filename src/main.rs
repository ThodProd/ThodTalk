#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
use eframe::egui;
use eframe::egui::{Align, Frame, Layout, Margin, Rect, RichText, Sense, Vec2, pos2, vec2};
mod ui;
use std::{
    sync::{atomic::Ordering, mpsc},
    time::Duration,
};
use thodtalk::{
    audio::Audio,
    session::Payload,
    storage::{Profile, Store, avatar_from_file},
    worker::SessionProxy as Session,
};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Page {
    Home,
    Archive,
}

const CHANNELS: [&str; 3] = ["Общий", "Игры", "Музыка"];
const SETTINGS_TABS: [(ui::Icon, &str); 5] = [
    (ui::Icon::User, "Профиль"),
    (ui::Icon::Gear, "Основное"),
    (ui::Icon::Headphones, "Звук"),
    (ui::Icon::Keyboard, "Клавиши"),
    (ui::Icon::Download, "Обновления"),
];

struct App {
    store: Store,
    profile: Profile,
    editing: bool,
    saved: bool,
    page: Page,
    settings_backup: Option<Profile>,
    error: String,
    storage_error: bool,
    avatar: Option<egui::TextureHandle>,
    session: Option<Session>,
    connecting: Option<mpsc::Receiver<anyhow::Result<Session>>>,
    code: String,
    draft: String,
    editing_message: Option<uuid::Uuid>,
    focus_composer: bool,
    channel: String,
    audio: Option<Audio>,
    voice_channel: Option<String>,
    muted: bool,
    deafened: bool,
    ptt: bool,
    input_device: String,
    output_device: String,
    inputs: Vec<String>,
    outputs: Vec<String>,
    member_textures: std::collections::BTreeMap<Vec<u8>, (Vec<u8>, egui::TextureHandle)>,
    archives: Option<Vec<(String, thodtalk::history::History)>>,
    cancel_connect: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    brand: egui::TextureHandle,
    copied_until: Option<std::time::Instant>,
    show_members: bool,
    settings_tab: usize,
    choosing_key: Option<usize>,
    user_card: Option<Vec<u8>>,
    user_card_fresh: bool,
    preview_image: Option<uuid::Uuid>,
    preview_zoom: f32,
    speaking: std::collections::BTreeMap<Vec<u8>, std::time::Instant>,
    image_textures: std::collections::BTreeMap<uuid::Uuid, egui::TextureHandle>,
    update_rx: Option<mpsc::Receiver<anyhow::Result<Option<thodtalk::updater::Prepared>>>>,
    update_ready: Option<thodtalk::updater::Prepared>,
    update_status: String,
    update_started: bool,
    reconnect_attempts: u32,
    last_disconnect_time: Option<std::time::Instant>,
    last_hosting: bool,
    share: Option<thodtalk::share::ScreenShare>,
    screen: Option<ScreenView>,
    screen_visible: bool,
    share_picker: bool,
    share_sources: Vec<thodtalk::share::SourceInfo>,
    share_source: usize,
    file_preview: Option<(uuid::Uuid, String, u64)>,
    _lock: std::fs::File,
}

struct ScreenView {
    texture: egui::TextureHandle,
    author: Vec<u8>,
    updated: std::time::Instant,
}
impl App {
    fn new(cc: &eframe::CreationContext<'_>, store: Store, lock: std::fs::File) -> Self {
        ui::fonts(&cc.egui_ctx);
        let (profile, saved, error) = match store.load::<Profile>("profile") {
            Ok(Some(p)) => (p, true, String::new()),
            Ok(None) => (Profile::default(), false, String::new()),
            Err(e) => (
                Profile::default(),
                false,
                format!("Не удалось прочитать профиль: {e}. Исходные данные сохранены."),
            ),
        };
        let storage_error = !error.is_empty();
        let (inputs, outputs) = thodtalk::audio::devices();
        let last_server = profile.last_server_code.clone();
        let mut app = Self {
            store,
            profile,
            editing: !saved,
            saved,
            page: Page::Home,
            settings_backup: None,
            error,
            storage_error,
            avatar: None,
            session: None,
            connecting: None,
            code: String::new(),
            draft: String::new(),
            editing_message: None,
            focus_composer: false,
            channel: "Общий".into(),
            audio: None,
            voice_channel: None,
            muted: false,
            deafened: false,
            ptt: false,
            input_device: String::new(),
            output_device: String::new(),
            inputs,
            outputs,
            member_textures: Default::default(),
            archives: None,
            cancel_connect: None,
            brand: texture(
                &cc.egui_ctx,
                "brand",
                include_bytes!(concat!(env!("OUT_DIR"), "/icon.png")),
            )
            .unwrap(),
            copied_until: None,
            show_members: true,
            settings_tab: 0,
            choosing_key: None,
            user_card: None,
            user_card_fresh: false,
            preview_image: None,
            preview_zoom: 1.0,
            speaking: Default::default(),
            image_textures: Default::default(),
            update_rx: None,
            update_ready: None,
            update_status: String::new(),
            update_started: false,
            reconnect_attempts: 0,
            last_disconnect_time: None,
            last_hosting: false,
            share: None,
            screen: None,
            screen_visible: true,
            share_picker: false,
            share_sources: Vec::new(),
            share_source: 0,
            file_preview: None,
            _lock: lock,
        };
        app.input_device = app.profile.input_device.clone();
        app.output_device = app.profile.output_device.clone();
        app.ptt = app.profile.push_to_talk;
        app.profile.network_allowed = true;
        app.profile.microphone_allowed = true;
        // Auto-connect to last server if available
        if let Some(server_code) = last_server {
            if !saved || app.error.is_empty() {
                app.code = server_code;
            }
        }
        ui::theme(&cc.egui_ctx, app.profile.light_theme);
        app.update_avatar(&cc.egui_ctx);
        app
    }
    fn update_avatar(&mut self, ctx: &egui::Context) {
        self.avatar = texture(ctx, "avatar", &self.profile.avatar);
    }
    fn connect(&mut self, hosting: bool) {
        if self.connecting.is_some() {
            return;
        }
        let path = self.store.root.clone();
        let mut profile = self.profile.clone();
        let code = self.code.clone();

        // Remember last server connection
        if !hosting && !code.is_empty() {
            profile.last_server_code = Some(code.clone());
            let _ = self.store.save("profile", &profile);
        }

        self.last_hosting = hosting;
        let (tx, rx) = mpsc::sync_channel(1);
        self.connecting = Some(rx);
        self.error.clear();
        let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        self.cancel_connect = Some(cancel.clone());
        std::thread::spawn(move || {
            let result = Store::open(path)
                .and_then(|store| Session::connect(store, profile, hosting, &code, cancel));
            let _ = tx.send(result);
        });
    }
    fn sound(&self, sound: thodtalk::sounds::Sound) {
        if self.profile.sounds && !self.deafened {
            thodtalk::sounds::play(sound);
        }
    }
    fn toggle_mute(&mut self) {
        self.muted = !self.muted;
        self.voice_state();
        if !self.deafened {
            self.sound(if self.muted {
                thodtalk::sounds::Sound::Mute
            } else {
                thodtalk::sounds::Sound::Unmute
            });
        }
    }
    fn toggle_deafen(&mut self) {
        self.deafened = !self.deafened;
        self.voice_state();
        if self.profile.sounds {
            thodtalk::sounds::play(if self.deafened {
                thodtalk::sounds::Sound::Deafen
            } else {
                thodtalk::sounds::Sound::Undeafen
            });
        }
    }
    fn stop_voice(&mut self) {
        if self.voice_channel.is_some() {
            self.sound(thodtalk::sounds::Sound::Leave);
        }
        self.audio.take();
        self.voice_channel = None;
        self.share = None;
        self.screen = None;
        self.screen_visible = true;
        self.share_picker = false;
        if let Some(session) = &mut self.session {
            let _ = session.send(Payload::VoiceState {
                channel: None,
                muted: true,
                deafened: self.deafened,
            });
        }
    }
    fn start_voice(&mut self, channel: &str) {
        if !self.profile.microphone_allowed {
            self.error = "Разрешите микрофон в профиле".into();
            return;
        }
        match Audio::start_with_opts(
            &self.input_device,
            &self.output_device,
            self.profile.ffmpeg_noise_suppression,
            self.profile.voice_enhancement,
        ) {
            Ok(audio) => {
                self.sound(thodtalk::sounds::Sound::Join);
                self.audio = Some(audio);
                self.voice_channel = Some(channel.to_string());
                if let Some(session) = &mut self.session {
                    let _ = session.send(Payload::VoiceState {
                        channel: Some(channel.to_string()),
                        muted: self.muted,
                        deafened: self.deafened,
                    });
                }
            }
            Err(e) => self.error = format!("Не удалось включить голос: {e}"),
        }
    }
    fn toggle_user_mute(&mut self, key: Vec<u8>) {
        if self.profile.muted_users.contains(&key) {
            self.profile.muted_users.remove(&key);
        } else {
            self.profile.muted_users.insert(key);
        }
    }
    fn toggle_share(&mut self) {
        if self.share.is_some() {
            self.share = None;
            return;
        }
        if self.voice_channel.is_none() {
            self.error = "Сначала войдите в голосовой канал".into();
            return;
        }
        self.share_sources = thodtalk::share::sources();
        self.share_source = 0;
        self.share_picker = true;
    }
    fn start_share(&mut self) {
        let Some(info) = self.share_sources.get(self.share_source).cloned() else {
            self.error = "Не выбран источник демонстрации".into();
            return;
        };
        self.share = Some(thodtalk::share::ScreenShare::start(info.source));
        self.share_picker = false;
        self.screen_visible = true;
    }
    /// Files dropped anywhere on the window are sent to the open text channel.
    fn dropped_files(&mut self, ctx: &egui::Context) {
        let dropped = ctx.input(|i| i.raw.dropped_files.clone());
        if dropped.is_empty() || self.editing {
            return;
        }
        let Some(session) = &mut self.session else {
            return;
        };
        if !session.ready() {
            self.error = "Дождитесь подключения к серверу".into();
            return;
        }
        for file in dropped {
            let Some(path) = file.path else {
                continue;
            };
            let result = if is_image_path(&path) {
                session.image(&self.channel, path)
            } else {
                session.file(&self.channel, path)
            };
            if let Err(error) = result {
                self.error = error.to_string();
            }
        }
    }
    fn drop_hint(&self, ctx: &egui::Context) {
        if self.editing || self.session.is_none() {
            return;
        }
        let count = ctx.input(|i| i.raw.hovered_files.len());
        if count == 0 {
            return;
        }
        let screen = ctx.screen_rect();
        egui::Area::new(egui::Id::new("drop-hint"))
            .order(egui::Order::Foreground)
            .fixed_pos(screen.min)
            .show(ctx, |ui| {
                let (rect, _) = ui.allocate_exact_size(screen.size(), Sense::hover());
                ui.painter()
                    .rect_filled(rect, 0.0, egui::Color32::from_black_alpha(130));
                ui.painter().text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    format!(
                        "Отпустите, чтобы отправить в #{}\nФайлов: {count}",
                        self.channel
                    ),
                    egui::FontId::proportional(20.0),
                    egui::Color32::WHITE,
                );
            });
    }
    fn pump_screen(&mut self, ctx: &egui::Context) {
        // Outgoing: hand frames captured in the background to the session worker.
        let mut failed = None;
        if let Some(share) = &self.share {
            while let Ok(result) = share.frames.try_recv() {
                match result {
                    Ok(frame) => {
                        if let (Some(session), Some(channel)) =
                            (&mut self.session, &self.voice_channel)
                        {
                            if let Err(error) =
                                session.share_screen(channel, frame.width, frame.height, frame.data)
                            {
                                self.error = error.to_string();
                            }
                        }
                    }
                    Err(error) => {
                        failed = Some(error);
                        break;
                    }
                }
            }
        }
        if let Some(error) = failed {
            self.error = format!("Демонстрация экрана: {error}");
            self.share = None;
        }

        // Incoming: keep only a frame for the voice channel we are in.
        let frame = self.session.as_mut().and_then(|s| s.screen_in.take());
        let Some((author, channel, _width, _height, data)) = frame else {
            return;
        };
        let own_key = self.session.as_ref().map(|s| s.own_key());
        if self.voice_channel.as_deref() != Some(channel.as_str())
            || own_key.as_deref() == Some(author.as_slice())
        {
            return;
        }
        let Some(color) = screen_image(&data) else {
            return;
        };
        let switched = self
            .screen
            .as_ref()
            .is_some_and(|view| view.author != author);
        let fresh = self.screen.is_none();
        match &mut self.screen {
            Some(view) => {
                view.texture.set(color, egui::TextureOptions::LINEAR);
                view.author = author;
                view.updated = std::time::Instant::now();
            }
            None => {
                let texture = ctx.load_texture("screen-share", color, egui::TextureOptions::LINEAR);
                self.screen = Some(ScreenView {
                    texture,
                    author,
                    updated: std::time::Instant::now(),
                });
            }
        }
        if switched || fresh {
            self.screen_visible = true;
        }
    }
    fn tick(&mut self, ctx: &egui::Context) {
        if self.saved && self.profile.auto_update && !self.update_started {
            self.check_updates();
        }
        if let Some(rx) = &self.update_rx {
            if let Ok(result) = rx.try_recv() {
                self.update_rx = None;
                match result {
                    Ok(Some(prepared)) => {
                        self.update_status = format!(
                            "Версия {} загружена. Перезапуск после завершения разговора.",
                            prepared.version
                        );
                        self.update_ready = Some(prepared);
                    }
                    Ok(None) => {
                        self.update_status = "Установлена последняя опубликованная версия".into()
                    }
                    Err(e) => self.update_status = format!("Не удалось проверить обновление: {e}"),
                }
            }
            ctx.request_repaint_after(Duration::from_millis(100));
        }
        if self.profile.auto_update
            && self.update_ready.is_some()
            && self.session.is_none()
            && self.connecting.is_none()
            && !self.editing
            && self.draft.is_empty()
        {
            self.install_update(ctx);
        }
        if self.choosing_key.is_none() && !ctx.wants_keyboard_input() {
            let mute = egui::Key::from_name(&self.profile.mute_key)
                .is_some_and(|k| ctx.input(|i| i.key_pressed(k)));
            let deafen = egui::Key::from_name(&self.profile.deafen_key)
                .is_some_and(|k| ctx.input(|i| i.key_pressed(k)));
            if mute {
                self.toggle_mute();
            }
            if deafen {
                self.toggle_deafen();
            }
        }

        // Auto-reconnect after disconnection (up to 3 attempts over ~2 min)
        if let Some(last_dc) = self.last_disconnect_time {
            if self.session.is_none() && self.connecting.is_none() && self.reconnect_attempts < 3 {
                let wait_ms = 15_000 * (1 << self.reconnect_attempts); // 15s, 30s, 60s
                if last_dc.elapsed() > Duration::from_millis(wait_ms) {
                    self.reconnect_attempts += 1;
                    if !self.code.is_empty() {
                        self.connect(!self.last_hosting);
                    }
                }
            }
        }

        if let Some(rx) = &self.connecting {
            if let Ok(result) = rx.try_recv() {
                self.connecting = None;
                match result {
                    Ok(session) => {
                        self.session = Some(session);
                        self.last_disconnect_time = None;
                        self.reconnect_attempts = 0;
                        self.code.clear();
                    }
                    Err(e) => {
                        self.error = format!("Подключение не удалось: {e}");
                    }
                }
            }
        }
        let mut disconnected = false;
        if let Some(session) = &mut self.session {
            let old_message = session.history.messages.last().map(|m| m.id);
            let old_voices: std::collections::BTreeMap<_, _> = session
                .members
                .iter()
                .map(|(k, m)| (k.clone(), m.voice.clone()))
                .collect();
            if let Err(e) = session.poll() {
                self.error = format!("Соединение прервано: {e}. История сохранена локально.");
                if self.profile.sounds && !self.deafened {
                    thodtalk::sounds::play(thodtalk::sounds::Sound::Error);
                }
                disconnected = true;
            }
            if self.profile.sounds && !self.deafened {
                if session
                    .history
                    .messages
                    .last()
                    .is_some_and(|m| Some(m.id) != old_message && m.author_key != session.own_key())
                {
                    thodtalk::sounds::play(thodtalk::sounds::Sound::Received);
                }
                if self.voice_channel.is_some() {
                    let joined = session.members.iter().any(|(k, m)| {
                        k != &session.own_key()
                            && m.voice == self.voice_channel
                            && old_voices.get(k) != Some(&m.voice)
                    });
                    let left = old_voices.iter().any(|(k, v)| {
                        k != &session.own_key()
                            && v == &self.voice_channel
                            && session.members.get(k).map(|m| &m.voice) != Some(v)
                    });
                    if joined {
                        thodtalk::sounds::play(thodtalk::sounds::Sound::VoiceJoin);
                    } else if left {
                        thodtalk::sounds::play(thodtalk::sounds::Sound::VoiceLeave);
                    }
                }
            }
            if let Some(audio) = &mut self.audio {
                let key = egui::Key::from_name(&self.profile.ptt_key).unwrap_or(egui::Key::F8);
                let held = self.choosing_key.is_none() && ctx.input(|i| i.key_down(key));
                let enabled = !self.muted && !self.deafened && (!self.ptt || held);
                audio.transmitting.store(enabled, Ordering::Relaxed);
                audio.deafened.store(self.deafened, Ordering::Relaxed);
                if enabled && f32::from_bits(audio.level.load(Ordering::Relaxed)) > 0.015 {
                    self.speaking
                        .insert(session.own_key(), std::time::Instant::now());
                }
                if let Ok(error) = audio.errors.try_recv() {
                    self.error = error;
                }
                for _ in 0..8 {
                    let Ok(data) = audio.encoded.try_recv() else {
                        break;
                    };
                    if enabled {
                        if let Some(channel) = &self.voice_channel {
                            if let Err(e) = session.send(Payload::Voice {
                                channel: channel.clone(),
                                data,
                            }) {
                                self.error = e.to_string();
                            }
                        }
                    }
                }
                for (author, channel, data) in session.voice_in.drain(..) {
                    if self.voice_channel.as_ref() == Some(&channel)
                        && !self.profile.muted_users.contains(&author)
                    {
                        let gain = self
                            .profile
                            .volumes
                            .get(&key_string(&author))
                            .copied()
                            .unwrap_or(1.0)
                            .clamp(0.0, 1.5);
                        match audio.play(author.clone(), &data, gain) {
                            Ok(true) => {
                                self.speaking.insert(author, std::time::Instant::now());
                            }
                            Ok(false) => {}
                            Err(e) => self.error = format!("Голосовой пакет: {e}"),
                        }
                    }
                }
            } else {
                session.voice_in.clear();
            }
        }
        if disconnected {
            self.audio.take();
            self.voice_channel = None;
            self.share = None;
            self.screen = None;
            self.session.take();
        }
        self.pump_screen(ctx);
        if self.session.is_some() || self.connecting.is_some() {
            ctx.request_repaint_after(Duration::from_millis(16));
        }
    }
    fn disconnect(&mut self) {
        self.stop_voice();
        self.session.take();
        self.member_textures.clear();
        self.speaking.clear();
        self.user_card = None;
        self.page = Page::Home;
        self.last_disconnect_time = Some(std::time::Instant::now());
        self.reconnect_attempts = 0;
    }
    fn open_settings(&mut self) {
        self.settings_backup = Some(self.profile.clone());
        self.settings_tab = 0;
        self.editing = true;
    }
    /// Закрыть настройки без сохранения: возвращаем то, что было.
    fn close_settings(&mut self, ctx: &egui::Context) {
        if !self.saved {
            return;
        }
        if let Some(old) = self.settings_backup.take() {
            self.profile = old;
            self.input_device = self.profile.input_device.clone();
            self.output_device = self.profile.output_device.clone();
            self.ptt = self.profile.push_to_talk;
            ui::theme(ctx, self.profile.light_theme);
            self.update_avatar(ctx);
        }
        self.choosing_key = None;
        self.editing = false;
    }
    fn save_profile(&mut self) {
        self.profile.input_device = self.input_device.clone();
        self.profile.output_device = self.output_device.clone();
        self.profile.push_to_talk = self.ptt;
        match self
            .profile
            .validate()
            .and_then(|_| self.store.save("profile", &self.profile))
        {
            Ok(()) => {
                self.editing = false;
                self.saved = true;
                self.settings_backup = None;
                self.choosing_key = None;
                self.error.clear();
                if !self.profile.network_allowed {
                    self.stop_voice();
                    self.session.take();
                }
                if !self.profile.microphone_allowed {
                    self.stop_voice();
                }
                if let Some(session) = &mut self.session {
                    if let Err(e) = session.update_profile(self.profile.clone()) {
                        self.error = e.to_string();
                    }
                }
            }
            Err(e) => self.error = e.to_string(),
        }
    }
    fn load_archives(&mut self) {
        let result = (|| -> anyhow::Result<Vec<(String, thodtalk::history::History)>> {
            let mut histories = Vec::new();
            for entry in std::fs::read_dir(&self.store.root)? {
                let entry = entry?;
                let name = entry.file_name().to_string_lossy().into_owned();
                if let Some(id) = name
                    .strip_prefix("history-")
                    .and_then(|s| s.strip_suffix(".bin"))
                    .and_then(|s| uuid::Uuid::parse_str(s).ok())
                {
                    let history = thodtalk::history::History::open(&self.store, id)?;
                    let title = history
                        .messages
                        .last()
                        .map(|m| {
                            format!(
                                "{} · {}",
                                m.display_name,
                                messages_count(history.messages.len())
                            )
                        })
                        .unwrap_or_else(|| "Пустая переписка".into());
                    histories.push((format!("{} · {}", title, &id.to_string()[..8]), history));
                }
            }
            Ok(histories)
        })();
        match result {
            Ok(histories) => self.archives = Some(histories),
            Err(e) => {
                self.error = e.to_string();
                self.archives = Some(Vec::new());
            }
        }
    }

    // ───────────────────────── Баннер ошибки ─────────────────────────

    fn error_bar(&mut self, ctx: &egui::Context) {
        if self.error.is_empty() {
            return;
        }
        let light = self.profile.light_theme;
        let message = self.error.clone();
        let can_dismiss = !self.storage_error;
        let mut dismiss = false;
        egui::TopBottomPanel::top("error_bar")
            .frame(
                Frame::new()
                    .fill(ui::danger_bg(light))
                    .inner_margin(Margin::symmetric(16, 8)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui::icon_label(ui, ui::Icon::Warning, 18.0, ui::danger(light));
                    let w = (ui.available_width() - 40.0).max(100.0);
                    ui.allocate_ui_with_layout(
                        vec2(w, 0.0),
                        Layout::left_to_right(Align::Center),
                        |ui| {
                            ui.add(
                                egui::Label::new(RichText::new(&message).color(ui::danger(light)))
                                    .wrap(),
                            );
                        },
                    );
                    if can_dismiss {
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if ui::icon_btn_sized(
                                ui,
                                light,
                                ui::Icon::Close,
                                ui::Tone::Danger,
                                false,
                                26.0,
                            )
                            .on_hover_text("Скрыть")
                            .clicked()
                            {
                                dismiss = true;
                            }
                        });
                    }
                });
            });
        if dismiss {
            self.error.clear();
        }
    }

    // ───────────────────────── Левая панель ─────────────────────────

    fn sidebar_ui(&mut self, ctx: &egui::Context) {
        let light = self.profile.light_theme;
        egui::SidePanel::left("sidebar")
            .exact_width(240.0)
            .resizable(false)
            .frame(Frame::new().fill(ui::sidebar(light)))
            .show(ctx, |ui| {
                ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
                egui::TopBottomPanel::bottom("account_panel")
                    .frame(
                        Frame::new()
                            .fill(ui::deep(light))
                            .inner_margin(Margin::symmetric(8, 8)),
                    )
                    .show_separator_line(false)
                    .show_inside(ui, |ui| self.account_panel(ui));
                self.sidebar_header(ui);
                egui::ScrollArea::vertical()
                    .id_salt("sidebar-scroll")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        Frame::new()
                            .inner_margin(Margin::symmetric(8, 10))
                            .show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                ui.spacing_mut().item_spacing.y = 2.0;
                                self.sidebar_body(ui);
                            });
                    });
            });
    }

    fn sidebar_header(&mut self, ui: &mut egui::Ui) {
        let light = self.profile.light_theme;
        let title = match &self.session {
            None => "ThodTalk",
            Some(s) if s.hosting() => "Ваш сервер",
            Some(_) => "Сервер друга",
        };
        let connected = self.session.is_some();
        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 48.0), Sense::hover());
        let mut child = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(rect.shrink2(vec2(12.0, 0.0)))
                .layout(Layout::left_to_right(Align::Center)),
        );
        child.spacing_mut().item_spacing.x = 10.0;
        child.image((self.brand.id(), vec2(28.0, 28.0)));
        child.label(ui::heading(title, 17.0));
        if connected {
            child.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui::icon_btn(ui, light, ui::Icon::Logout, ui::Tone::Danger, false)
                    .on_hover_text("Отключиться от сервера")
                    .clicked()
                {
                    self.disconnect();
                }
            });
        }
        ui.painter().hline(
            rect.x_range(),
            rect.bottom(),
            egui::Stroke::new(1.0_f32, ui::line(light)),
        );
    }

    fn sidebar_body(&mut self, ui: &mut egui::Ui) {
        let light = self.profile.light_theme;
        if self.session.is_none() {
            if ui::nav_row(
                ui,
                light,
                ui::Icon::Home,
                "Главная",
                self.page == Page::Home,
                None,
            )
            .clicked()
            {
                self.page = Page::Home;
            }
            if ui::nav_row(
                ui,
                light,
                ui::Icon::Archive,
                "Сохранённые переписки",
                self.page == Page::Archive,
                None,
            )
            .clicked()
            {
                self.page = Page::Archive;
                self.load_archives();
            }
            return;
        }
        ui::section(ui, "ТЕКСТОВЫЕ КАНАЛЫ", light);
        for ch in CHANNELS {
            if ui::nav_row(ui, light, ui::Icon::Hash, ch, self.channel == ch, None).clicked() {
                self.channel = ch.into();
            }
        }
        ui.add_space(14.0);
        ui::section(ui, "ГОЛОСОВЫЕ КАНАЛЫ", light);
        let ready = self.session.as_ref().is_some_and(|s| s.ready());
        for ch in CHANNELS {
            let here = self.voice_channel.as_deref() == Some(ch);
            let resp = ui::nav_row(
                ui,
                light,
                ui::Icon::Speaker,
                ch,
                here,
                here.then(|| ui::success(light)),
            )
            .on_hover_text(if here {
                "Вы в этом голосовом канале"
            } else {
                "Нажмите, чтобы войти в голосовой канал"
            });
            if resp.clicked() && ready && !here {
                if self.voice_channel.is_some() {
                    self.stop_voice();
                }
                self.start_voice(ch);
            }
            self.member_rows(ui, Some(ch));
        }
    }

    /// Нижняя панель как в Discord: статус голоса и «я» с кнопками микрофона, звука и настроек.
    fn account_panel(&mut self, ui: &mut egui::Ui) {
        let light = self.profile.light_theme;
        let mut toggle_share = false;
        let mut toggle_watch = false;
        if let Some(channel) = self.voice_channel.clone() {
            let mut leave = false;
            let (rect, _) =
                ui.allocate_exact_size(vec2(ui.available_width(), 40.0), Sense::hover());
            let mut child = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(rect.shrink2(vec2(4.0, 0.0)))
                    .layout(Layout::left_to_right(Align::Center)),
            );
            child.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                ui.label(
                    RichText::new("Голос подключён")
                        .size(13.0)
                        .strong()
                        .color(ui::success(light)),
                );
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 5.0;
                    ui::icon_label(ui, ui::Icon::Speaker, 13.0, ui::muted(light));
                    ui.label(
                        RichText::new(channel.as_str())
                            .size(12.0)
                            .color(ui::muted(light)),
                    );
                    let ping = self.session.as_ref().and_then(|s| s.ping_ms);
                    match ping {
                        Some(ms) => {
                            ui.label(
                                RichText::new(format!("· {ms} мс"))
                                    .size(12.0)
                                    .color(ui::ping(light, ms)),
                            )
                            .on_hover_text("Задержка до сервера");
                        }
                        None => {
                            ui.label(
                                RichText::new("· измеряем…")
                                    .size(12.0)
                                    .color(ui::muted(light)),
                            );
                        }
                    }
                });
            });
            child.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui::icon_btn(ui, light, ui::Icon::HangUp, ui::Tone::Danger, false)
                    .on_hover_text("Выйти из голосового канала")
                    .clicked()
                {
                    leave = true;
                }
                let sharing = self.share.is_some();
                if ui::icon_btn(
                    ui,
                    light,
                    ui::Icon::Monitor,
                    if sharing {
                        ui::Tone::Active
                    } else {
                        ui::Tone::Normal
                    },
                    false,
                )
                .on_hover_text(if sharing {
                    "Остановить демонстрацию экрана"
                } else {
                    "Демонстрация экрана"
                })
                .clicked()
                {
                    toggle_share = true;
                }
                let available = self
                    .screen
                    .as_ref()
                    .is_some_and(|view| view.updated.elapsed() < Duration::from_secs(3));
                if available
                    && !sharing
                    && ui::icon_btn(
                        ui,
                        light,
                        ui::Icon::Eye,
                        if self.screen_visible {
                            ui::Tone::Active
                        } else {
                            ui::Tone::Normal
                        },
                        false,
                    )
                    .on_hover_text(if self.screen_visible {
                        "Скрыть демонстрацию экрана"
                    } else {
                        "Смотреть демонстрацию экрана"
                    })
                    .clicked()
                {
                    toggle_watch = true;
                }
            });
            ui.add_space(4.0);
            ui.painter().hline(
                rect.x_range(),
                ui.cursor().top(),
                egui::Stroke::new(1.0_f32, ui::line(light)),
            );
            ui.add_space(6.0);
            if leave {
                self.stop_voice();
            }
            if toggle_share {
                self.toggle_share();
            }
            if toggle_watch {
                self.screen_visible = !self.screen_visible;
            }
        }

        let online = self.session.is_some();
        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 40.0), Sense::hover());
        let icons_w = 3.0 * 28.0 + 2.0 * 2.0;
        let left_rect =
            Rect::from_min_max(rect.min, pos2(rect.right() - icons_w - 6.0, rect.bottom()));
        let right_rect = Rect::from_min_max(pos2(rect.right() - icons_w, rect.top()), rect.max);

        let mut left = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(left_rect)
                .layout(Layout::left_to_right(Align::Center)),
        );
        left.spacing_mut().item_spacing.x = 8.0;
        ui::Avatar::new(self.avatar.as_ref(), &self.profile.name, 34.0)
            .status(if online { ui::GREEN } else { ui::GRAY }, ui::deep(light))
            .show(&mut left);
        left.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            ui.add(
                egui::Label::new(ui::heading(
                    if self.profile.name.is_empty() {
                        "Ваш профиль"
                    } else {
                        &self.profile.name
                    },
                    14.0,
                ))
                .truncate(),
            );
            ui.add(
                egui::Label::new(
                    RichText::new(if online {
                        "В сети"
                    } else {
                        "Не подключён"
                    })
                    .size(11.5)
                    .color(ui::muted(light)),
                )
                .truncate(),
            );
        });

        let mut right = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(right_rect)
                .layout(Layout::right_to_left(Align::Center)),
        );
        right.spacing_mut().item_spacing.x = 2.0;
        if ui::icon_btn_sized(
            &mut right,
            light,
            ui::Icon::Gear,
            ui::Tone::Normal,
            false,
            28.0,
        )
        .on_hover_text("Настройки")
        .clicked()
        {
            self.open_settings();
        }
        let deaf_tip = format!(
            "{} звук · {}",
            if self.deafened {
                "Включить"
            } else {
                "Выключить"
            },
            self.profile.deafen_key
        );
        if ui::icon_btn_sized(
            &mut right,
            light,
            ui::Icon::Headphones,
            if self.deafened {
                ui::Tone::Alert
            } else {
                ui::Tone::Normal
            },
            self.deafened,
            28.0,
        )
        .on_hover_text(deaf_tip)
        .clicked()
        {
            self.toggle_deafen();
        }
        let mic_off = self.muted || self.deafened;
        let mic_tip = format!(
            "{} микрофон · {}",
            if self.muted {
                "Включить"
            } else {
                "Выключить"
            },
            self.profile.mute_key
        );
        if ui::icon_btn_sized(
            &mut right,
            light,
            ui::Icon::Mic,
            if mic_off {
                ui::Tone::Alert
            } else {
                ui::Tone::Normal
            },
            mic_off,
            28.0,
        )
        .on_hover_text(mic_tip)
        .clicked()
        {
            self.toggle_mute();
        }
    }

    // ───────────────────────── Участники ─────────────────────────

    fn members_panel(&mut self, ctx: &egui::Context) {
        let light = self.profile.light_theme;
        let count = self.session.as_ref().map_or(0, |s| s.members.len());
        egui::SidePanel::right("members")
            .exact_width(220.0)
            .resizable(false)
            .frame(
                Frame::new()
                    .fill(ui::sidebar(light))
                    .inner_margin(Margin::symmetric(8, 14)),
            )
            .show(ctx, |ui| {
                ui::section(ui, &format!("УЧАСТНИКИ — {count}"), light);
                ui.add_space(4.0);
                egui::ScrollArea::vertical()
                    .id_salt("members-scroll")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing.y = 2.0;
                        self.member_rows(ui, None);
                    });
            });
    }

    /// `channel = Some(..)` — компактный список под голосовым каналом, `None` — правая панель.
    fn member_rows(&mut self, ui: &mut egui::Ui, channel: Option<&str>) {
        let light = self.profile.light_theme;
        let Some(session) = &self.session else {
            return;
        };
        let own = session.own_key();
        let hosting = session.hosting();
        let members: Vec<_> = session
            .members
            .iter()
            .filter(|(_, m)| channel.is_none() || m.voice.as_deref() == channel)
            .map(|(k, m)| (k.clone(), m.clone()))
            .collect();
        let compact = channel.is_some();
        for (key, member) in members {
            if !member.avatar.is_empty()
                && self
                    .member_textures
                    .get(&key)
                    .is_none_or(|(bytes, _)| bytes != &member.avatar)
            {
                if let Some(image) = texture(ui.ctx(), &format!("member-{key:?}"), &member.avatar) {
                    self.member_textures
                        .insert(key.clone(), (member.avatar.clone(), image));
                }
            }
            let speaking = !member.muted
                && self
                    .speaking
                    .get(&key)
                    .is_some_and(|time| time.elapsed() < Duration::from_millis(220));
            let (height, avatar_size, indent) = if compact {
                (28.0, 22.0, 20.0)
            } else {
                (44.0, 32.0, 0.0)
            };
            let in_voice = member.voice.is_some();
            let show_icon = (member.muted || member.deafened) && in_voice || (!compact && in_voice);
            let icon_width = if member.deafened {
                44.0
            } else if show_icon {
                26.0
            } else {
                4.0
            };
            let textures = &self.member_textures;
            let response = ui::row(ui, light, height, indent, icon_width, |ui| {
                let mut avatar = ui::Avatar::new(
                    textures.get(&key).map(|(_, t)| t),
                    &member.name,
                    avatar_size,
                )
                .ring(speaking.then(|| ui::success(light)));
                if !compact {
                    avatar = avatar.status(ui::GREEN, ui::sidebar(light));
                }
                avatar.show(ui);
                ui.add(
                    egui::Label::new(
                        RichText::new(&member.name)
                            .size(if compact { 14.0 } else { 14.5 })
                            .color(if speaking {
                                ui::success(light)
                            } else if compact {
                                ui::muted(light)
                            } else {
                                ui::text(light)
                            }),
                    )
                    .truncate(),
                );
            });
            if member.deafened {
                // Полностью выключен звук: и микрофон, и наушники перечёркнуты.
                let color = ui::danger(light);
                ui::paint_icon(
                    ui.painter(),
                    ui::Icon::Headphones,
                    Rect::from_center_size(
                        pos2(response.rect.right() - 14.0, response.rect.center().y),
                        Vec2::splat(15.0),
                    ),
                    color,
                    true,
                );
                ui::paint_icon(
                    ui.painter(),
                    ui::Icon::Mic,
                    Rect::from_center_size(
                        pos2(response.rect.right() - 32.0, response.rect.center().y),
                        Vec2::splat(15.0),
                    ),
                    color,
                    true,
                );
            } else if show_icon {
                let (icon, color, slashed) = if member.muted {
                    (ui::Icon::Mic, ui::danger(light), true)
                } else {
                    (ui::Icon::Speaker, ui::muted(light), false)
                };
                ui::paint_icon(
                    ui.painter(),
                    icon,
                    Rect::from_center_size(
                        pos2(response.rect.right() - 14.0, response.rect.center().y),
                        Vec2::splat(15.0),
                    ),
                    color,
                    slashed,
                );
            }
            if response.clicked() {
                self.user_card = Some(key.clone());
                self.user_card_fresh = true;
            }
            response.context_menu(|ui| {
                if key != own {
                    let volume_key = key_string(&key);
                    let volume = self.profile.volumes.entry(volume_key).or_insert(1.0);
                    if ui
                        .button(if *volume == 0.0 {
                            "Включить звук участника"
                        } else {
                            "Заглушить для себя"
                        })
                        .clicked()
                    {
                        *volume = if *volume == 0.0 { 1.0 } else { 0.0 };
                        let _ = self.store.save("profile", &self.profile);
                        ui.close_menu();
                    }
                    let is_muted = self.profile.muted_users.contains(&key);
                    if ui
                        .button(if is_muted {
                            "Включить голос"
                        } else {
                            "Отключить голос"
                        })
                        .clicked()
                    {
                        self.toggle_user_mute(key.clone());
                        let _ = self.store.save("profile", &self.profile);
                        ui.close_menu();
                    }
                    if hosting {
                        ui.separator();
                        if ui.button("Кикнуть").clicked() {
                            if let Err(e) = self.session.as_mut().unwrap().kick(&key) {
                                self.error = e.to_string();
                            }
                            ui.close_menu();
                        }
                        if ui.button("Заблокировать до остановки сервера").clicked()
                        {
                            if let Err(e) = self.session.as_mut().unwrap().ban(&key) {
                                self.error = e.to_string();
                            }
                            ui.close_menu();
                        }
                    }
                }
            });
        }
    }

    fn member_card(&mut self, ctx: &egui::Context) {
        let light = self.profile.light_theme;
        let Some(key) = self.user_card.clone() else {
            return;
        };
        let Some(member) = self
            .session
            .as_ref()
            .and_then(|s| s.members.get(&key))
            .cloned()
        else {
            self.user_card = None;
            return;
        };
        let (own, hosting) = self
            .session
            .as_ref()
            .map(|s| (s.own_key() == key, s.hosting()))
            .unwrap_or((false, false));
        let texture = self.member_textures.get(&key).map(|(_, t)| t.clone());
        let volume_key = key_string(&key);
        let mut volume = self
            .profile
            .volumes
            .get(&volume_key)
            .copied()
            .unwrap_or(1.0)
            .clamp(0.0, 1.5);
        let mut percent = volume * 100.0;
        let mut local_mute = volume <= 0.0;
        let mut volume_changed = false;
        let mut open = true;
        let mut kick = false;
        let mut ban = false;

        let frame = Frame::new()
            .fill(ui::surface(light))
            .stroke(egui::Stroke::new(1.0, ui::line(light)))
            .corner_radius(12)
            .inner_margin(Margin::same(16));
        let window_response = egui::Window::new("Участник")
            .id(egui::Id::new("user-card"))
            .title_bar(false)
            .collapsible(false)
            .resizable(false)
            .default_width(300.0)
            .default_pos(pos2(ctx.screen_rect().right() - 320.0, 96.0))
            .frame(frame)
            .show(ctx, |ui| {
                ui.set_width(300.0);
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 12.0;
                    ui::Avatar::new(texture.as_ref(), &member.name, 56.0)
                        .status(
                            if member.voice.is_some() {
                                ui::GREEN
                            } else {
                                ui::muted(light)
                            },
                            ui::surface(light),
                        )
                        .show(ui);
                    ui.vertical(|ui| {
                        ui.label(
                            ui::heading(&member.name, 18.0)
                                .color(ui::name_color(&member.name, light)),
                        );
                        let status = if own {
                            "Это вы".to_owned()
                        } else if let Some(channel) = &member.voice {
                            format!("В голосовом канале #{channel}")
                        } else {
                            "В сети".to_owned()
                        };
                        ui.label(RichText::new(status).size(12.5).color(ui::muted(light)));
                    });
                    ui.with_layout(Layout::right_to_left(Align::Min), |ui| {
                        if ui::icon_btn_sized(
                            ui,
                            light,
                            ui::Icon::Close,
                            ui::Tone::Normal,
                            false,
                            28.0,
                        )
                        .on_hover_text("Закрыть")
                        .clicked()
                        {
                            open = false;
                        }
                    });
                });

                if !own {
                    ui.add_space(4.0);
                    ui::divider(ui, light);
                    ui.horizontal(|ui| {
                        ui::icon_label(ui, ui::Icon::Speaker, 16.0, ui::muted(light));
                        ui.label(RichText::new("Громкость").size(14.0));
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            ui.label(
                                RichText::new(format!("{percent:.0}%"))
                                    .size(13.0)
                                    .strong()
                                    .color(ui::muted(light)),
                            );
                        });
                    });
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 8.0;
                        if ui::icon_btn_sized(
                            ui,
                            light,
                            ui::Icon::Minus,
                            ui::Tone::Normal,
                            false,
                            28.0,
                        )
                        .on_hover_text("Тише")
                        .clicked()
                        {
                            percent = (percent - 10.0).max(0.0);
                            volume = percent / 100.0;
                            volume_changed = true;
                        }
                        let slider_width = (ui.available_width() - 72.0).max(90.0);
                        ui.spacing_mut().slider_width = slider_width;
                        if ui
                            .add(
                                egui::Slider::new(&mut percent, 0.0..=150.0)
                                    .show_value(false)
                                    .step_by(1.0),
                            )
                            .changed()
                        {
                            volume = percent / 100.0;
                            volume_changed = true;
                        }
                        if ui::icon_btn_sized(
                            ui,
                            light,
                            ui::Icon::Plus,
                            ui::Tone::Normal,
                            false,
                            28.0,
                        )
                        .on_hover_text("Громче")
                        .clicked()
                        {
                            percent = (percent + 10.0).min(150.0);
                            volume = percent / 100.0;
                            volume_changed = true;
                        }
                    });
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 6.0;
                        if ui
                            .small_button("0%")
                            .on_hover_text("Полностью заглушить")
                            .clicked()
                        {
                            percent = 0.0;
                            volume = 0.0;
                            volume_changed = true;
                        }
                        if ui
                            .small_button("100%")
                            .on_hover_text("Стандартная громкость")
                            .clicked()
                        {
                            percent = 100.0;
                            volume = 1.0;
                            volume_changed = true;
                        }
                        if ui
                            .small_button("150%")
                            .on_hover_text("Максимальная громкость")
                            .clicked()
                        {
                            percent = 150.0;
                            volume = 1.5;
                            volume_changed = true;
                        }
                    });
                    local_mute = volume <= 0.0;
                    let mute_before = local_mute;
                    ui.add_space(4.0);
                    ui::setting_row(
                        ui,
                        light,
                        "Заглушить для себя",
                        "Не слышать этого участника",
                        |ui| {
                            let mut value = local_mute;
                            if ui::toggle(ui, &mut value).changed() {
                                local_mute = value;
                            }
                        },
                    );
                    if local_mute != mute_before {
                        volume = if local_mute { 0.0 } else { 1.0 };
                        percent = volume * 100.0;
                        volume_changed = true;
                    }

                    let mut is_globally_muted = self.profile.muted_users.contains(&key);
                    let mute_before_global = is_globally_muted;
                    ui::setting_row(
                        ui,
                        light,
                        "Отключить голос",
                        "Полностью отключить голос этого участника",
                        |ui| {
                            if ui::toggle(ui, &mut is_globally_muted).changed() {
                                // Toggle passed
                            }
                        },
                    );
                    if is_globally_muted != mute_before_global {
                        self.toggle_user_mute(key.clone());
                        let _ = self.store.save("profile", &self.profile);
                    }

                    if hosting {
                        ui::divider(ui, light);
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 8.0;
                            let width = (ui.available_width() - 8.0) / 2.0;
                            let kick_btn = egui::Button::new(
                                RichText::new("Кикнуть")
                                    .size(14.0)
                                    .strong()
                                    .color(ui::danger(light)),
                            )
                            .fill(ui::danger_bg(light))
                            .stroke(egui::Stroke::new(
                                1.0,
                                ui::danger(light).gamma_multiply(0.35),
                            ))
                            .min_size(vec2(width, 34.0));
                            if ui
                                .add(kick_btn)
                                .on_hover_text("Отключить от сервера")
                                .clicked()
                            {
                                kick = true;
                            }
                            let ban_btn = egui::Button::new(
                                RichText::new("Заблокировать")
                                    .size(14.0)
                                    .strong()
                                    .color(ui::danger(light)),
                            )
                            .fill(ui::danger_bg(light))
                            .stroke(egui::Stroke::new(
                                1.0,
                                ui::danger(light).gamma_multiply(0.35),
                            ))
                            .min_size(vec2(width, 34.0));
                            if ui
                                .add(ban_btn)
                                .on_hover_text("Заблокировать до остановки сервера")
                                .clicked()
                            {
                                ban = true;
                            }
                        });
                    }
                }
            });

        if volume_changed {
            volume = volume.clamp(0.0, 1.5);
            self.profile.volumes.insert(volume_key, volume);
            let _ = self.store.save("profile", &self.profile);
        }
        if ban {
            if let Some(session) = self.session.as_mut() {
                if let Err(e) = session.ban(&key) {
                    self.error = e.to_string();
                }
            }
            self.user_card = None;
        } else if kick {
            if let Some(session) = self.session.as_mut() {
                if let Err(e) = session.kick(&key) {
                    self.error = e.to_string();
                }
            }
            self.user_card = None;
        }
        let card_rect = window_response
            .map(|r| r.response.rect)
            .unwrap_or(Rect::NOTHING);
        let clicked_outside = ctx.input(|i| i.pointer.any_click())
            && !card_rect.contains(
                ctx.input(|i| i.pointer.interact_pos())
                    .unwrap_or(pos2(-1.0, -1.0)),
            );
        if !open
            || ctx.input(|i| i.key_pressed(egui::Key::Escape))
            || (!self.user_card_fresh && clicked_outside)
        {
            self.user_card = None;
        }
        self.user_card_fresh = false;
    }

    fn image_preview(&mut self, ctx: &egui::Context) {
        let Some(id) = self.preview_image else {
            return;
        };
        if !self.image_textures.contains_key(&id) {
            if let Ok(Some(bytes)) = self.store.load::<Vec<u8>>(&format!("image-{id}")) {
                if let Some(image) = texture(ctx, &format!("image-{id}"), &bytes) {
                    self.image_textures.insert(id, image);
                }
            }
        }
        let Some(image) = self.image_textures.get(&id).cloned() else {
            self.preview_image = None;
            return;
        };
        let screen = ctx.screen_rect();
        let mut close = false;
        let mut save = false;

        // Mouse wheel zoom
        ctx.input(|i| {
            if i.raw_scroll_delta.y > 0.0 {
                self.preview_zoom = (self.preview_zoom * 1.2).min(8.0);
            } else if i.raw_scroll_delta.y < 0.0 {
                self.preview_zoom = (self.preview_zoom / 1.2).max(0.5);
            }
        });

        egui::Area::new(egui::Id::new("image-preview"))
            .order(egui::Order::Foreground)
            .fixed_pos(screen.min)
            .show(ctx, |ui| {
                let (bg_rect, bg_response) = ui.allocate_exact_size(screen.size(), Sense::click());
                ui.painter()
                    .rect_filled(bg_rect, 0.0, egui::Color32::from_black_alpha(210));
                let source = image.size_vec2();
                let max = egui::vec2(screen.width() * 0.86, screen.height() * 0.82);
                let auto_scale = (max.x / source.x).min(max.y / source.y).min(4.0);
                let scale = auto_scale * self.preview_zoom;
                let size = source * scale;
                let image_rect = Rect::from_center_size(bg_rect.center(), size);
                let response = ui.put(
                    image_rect,
                    egui::Image::new((image.id(), size))
                        .corner_radius(10)
                        .sense(Sense::click()),
                );
                if bg_response.clicked() && !response.hovered() {
                    close = true;
                }
                response.context_menu(|ui| {
                    if ui.button("Сохранить на компьютер").clicked() {
                        save = true;
                        ui.close_menu();
                    }
                });
                let close_rect = Rect::from_min_size(
                    pos2(screen.right() - 52.0, screen.top() + 16.0),
                    Vec2::splat(32.0),
                );
                if ui
                    .put(
                        close_rect,
                        egui::Button::new(
                            RichText::new("×").size(28.0).color(egui::Color32::WHITE),
                        )
                        .frame(false),
                    )
                    .clicked()
                {
                    close = true;
                }
                let zoom_text = format!("×{:.1}", self.preview_zoom);
                ui.painter().text(
                    pos2(screen.center().x, screen.bottom() - 48.0),
                    egui::Align2::CENTER_CENTER,
                    format!("Скролл для зума {}", zoom_text),
                    egui::FontId::proportional(13.0),
                    egui::Color32::from_white_alpha(180),
                );
                ui.painter().text(
                    pos2(screen.center().x, screen.bottom() - 28.0),
                    egui::Align2::CENTER_CENTER,
                    "Клик по фону — закрыть · ПКМ по картинке — сохранить",
                    egui::FontId::proportional(13.0),
                    egui::Color32::from_white_alpha(180),
                );
            });
        if save {
            if let Err(e) = save_attachment(&self.store, id) {
                self.error = e.to_string();
            }
        }
        if close || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.preview_image = None;
            self.preview_zoom = 1.0;
        }
    }

    fn file_preview_window(&mut self, ctx: &egui::Context) {
        let Some((id, name, size)) = self.file_preview.clone() else {
            return;
        };
        let light = self.profile.light_theme;
        let mut save = false;
        let mut open = false;
        let mut close = false;
        let mut window_open = true;
        egui::Window::new("Файл")
            .id(egui::Id::new("file-preview"))
            .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
            .collapsible(false)
            .resizable(false)
            .open(&mut window_open)
            .show(ctx, |ui| {
                ui.set_width(430.0);
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 12.0;
                    let (rect, _) =
                        ui.allocate_exact_size(Vec2::splat(42.0), Sense::hover());
                    ui::paint_icon(
                        ui.painter(),
                        ui::Icon::File,
                        rect.shrink(3.0),
                        ui::accent(light),
                        false,
                    );
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 2.0;
                        ui.add(
                            egui::Label::new(
                                RichText::new(&name).size(15.0).strong(),
                            )
                            .wrap(),
                        );
                        ui.label(
                            RichText::new(file_size(size))
                                .size(12.0)
                                .color(ui::muted(light)),
                        );
                    });
                });
                ui.add_space(10.0);
                ui.label(
                    RichText::new(
                        "Открывайте только файлы от людей, которым доверяете: они могут содержать вирусы.",
                    )
                    .size(12.0)
                    .color(ui::muted(light)),
                );
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    if ui.add(ui::primary("Скачать")).clicked() {
                        save = true;
                    }
                    if ui.add(ui::secondary("Открыть")).clicked() {
                        open = true;
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.add(ui::secondary("Закрыть")).clicked() {
                            close = true;
                        }
                    });
                });
            });
        if save {
            match save_file(&self.store, id, &name) {
                Ok(()) => close = true,
                Err(error) => self.error = error.to_string(),
            }
        }
        if open {
            match open_file(&self.store, id, &name) {
                Ok(()) => close = true,
                Err(error) => self.error = error.to_string(),
            }
        }
        if close || !window_open || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.file_preview = None;
        }
    }

    // ───────────────────────── Чат ─────────────────────────

    fn share_picker_window(&mut self, ctx: &egui::Context) {
        if !self.share_picker {
            return;
        }
        let light = self.profile.light_theme;
        let mut open = true;
        let mut start = false;
        let mut cancel = false;
        let mut refresh = false;
        egui::Window::new("Поделиться экраном")
            .id(egui::Id::new("share-picker"))
            .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
            .collapsible(false)
            .resizable(false)
            .open(&mut open)
            .show(ctx, |ui| {
                ui.set_width(460.0);
                ui.label(
                    RichText::new("Что показывать?")
                        .size(16.0)
                        .strong()
                        .color(ui::text(light)),
                );
                ui.label(
                    RichText::new(
                        "Выберите весь экран или отдельное окно. Демонстрацию увидят все участники сервера, звук не передаётся.",
                    )
                    .size(12.5)
                    .color(ui::muted(light)),
                );
                ui.add_space(8.0);
                egui::ScrollArea::vertical()
                    .id_salt("share-sources")
                    .max_height(300.0)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        for (index, source) in self.share_sources.iter().enumerate() {
                            let text = format!("{}\n{}", source.title, source.detail);
                            let response = ui.selectable_value(
                                &mut self.share_source,
                                index,
                                RichText::new(text).size(13.0),
                            );
                            if response.double_clicked() {
                                self.share_source = index;
                                start = true;
                            }
                        }
                    });
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    if ui.add(ui::primary("Начать трансляцию")).clicked() {
                        start = true;
                    }
                    if ui.add(ui::secondary("Отмена")).clicked() {
                        cancel = true;
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui
                            .add(ui::secondary("Обновить список"))
                            .on_hover_text("Перечитать открытые окна")
                            .clicked()
                        {
                            refresh = true;
                        }
                    });
                });
            });
        if refresh {
            self.share_sources = thodtalk::share::sources();
            self.share_source = self
                .share_source
                .min(self.share_sources.len().saturating_sub(1));
        }
        if start {
            self.start_share();
        } else if cancel || !open {
            self.share_picker = false;
        }
    }

    fn screen_window(&mut self, ctx: &egui::Context) {
        let light = self.profile.light_theme;
        let Some(view) = &self.screen else {
            return;
        };
        if !self.screen_visible || view.updated.elapsed() > Duration::from_secs(3) {
            return;
        }
        let texture = view.texture.clone();
        let author = view.author.clone();
        let name = self
            .session
            .as_ref()
            .and_then(|s| s.members.get(&author).map(|m| m.name.clone()))
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| "Участник".into());
        let source = texture.size_vec2();
        let max = egui::vec2(
            (ctx.screen_rect().width() * 0.72).clamp(320.0, 1100.0),
            (ctx.screen_rect().height() * 0.62).max(200.0),
        );
        let scale = (max.x / source.x).min(max.y / source.y).min(1.0);
        let mut hide = false;
        egui::Window::new(format!("Демонстрация экрана · {name}"))
            .id(egui::Id::new("screen-viewer"))
            .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                ui.add(egui::Image::new((texture.id(), source * scale)).corner_radius(8));
                ui.horizontal(|ui| {
                    if ui.button("Скрыть").clicked() {
                        hide = true;
                    }
                    ui.label(
                        RichText::new("Демонстрация обновляется в реальном времени")
                            .size(11.5)
                            .color(ui::muted(light)),
                    );
                });
            });
        if hide {
            self.screen_visible = false;
        }
    }

    fn chat_bar(&mut self, ctx: &egui::Context) {
        let light = self.profile.light_theme;
        let (status, hosting, code, closed) = {
            let s = self.session.as_ref().unwrap();
            (s.status.clone(), s.hosting(), s.code.clone(), s.closed)
        };
        let copied = self
            .copied_until
            .is_some_and(|until| until > std::time::Instant::now());
        let short_status = if closed {
            "Вход закрыт"
        } else if hosting {
            if code.is_empty() {
                "Сервер работает"
            } else {
                "Код готов"
            }
        } else {
            "Подключено"
        };
        let members_shown = self.show_members;
        let mut toggle_members = false;
        let mut toggle_closed = false;
        let mut copy = false;
        egui::TopBottomPanel::top("chat_bar")
            .exact_height(48.0)
            .frame(
                Frame::new()
                    .fill(ui::bg(light))
                    .inner_margin(Margin::symmetric(16, 0)),
            )
            .show(ctx, |ui| {
                ui.horizontal_centered(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui::icon_btn(
                            ui,
                            light,
                            ui::Icon::Users,
                            if members_shown {
                                ui::Tone::Active
                            } else {
                                ui::Tone::Normal
                            },
                            false,
                        )
                        .on_hover_text(if members_shown {
                            "Скрыть участников"
                        } else {
                            "Показать участников"
                        })
                        .clicked()
                        {
                            toggle_members = true;
                        }
                        if hosting {
                            if ui::icon_btn(
                                ui,
                                light,
                                ui::Icon::Lock,
                                if closed {
                                    ui::Tone::Alert
                                } else {
                                    ui::Tone::Normal
                                },
                                false,
                            )
                            .on_hover_text(if closed {
                                "Вход закрыт — нажмите, чтобы открыть"
                            } else {
                                "Закрыть вход для новых участников"
                            })
                            .clicked()
                            {
                                toggle_closed = true;
                            }
                            let invite = ui
                                .add_enabled_ui(!code.is_empty(), |ui| {
                                    ui::icon_btn(
                                        ui,
                                        light,
                                        if copied {
                                            ui::Icon::Check
                                        } else {
                                            ui::Icon::UserPlus
                                        },
                                        if copied {
                                            ui::Tone::Active
                                        } else {
                                            ui::Tone::Normal
                                        },
                                        false,
                                    )
                                })
                                .inner
                                .on_hover_text(if copied {
                                    "Код скопирован"
                                } else {
                                    "Пригласить друзей — скопировать код"
                                });
                            if invite.clicked() {
                                copy = true;
                            }
                        }
                        ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                            ui::icon_label(ui, ui::Icon::Hash, 20.0, ui::muted(light));
                            ui.label(ui::heading(&self.channel, 16.0));
                            ui::icon_label(ui, ui::Icon::Shield, 15.0, ui::muted(light))
                                .on_hover_text("Сквозное шифрование");
                            ui.add(
                                egui::Label::new(
                                    RichText::new(short_status)
                                        .size(12.5)
                                        .color(ui::muted(light)),
                                )
                                .truncate(),
                            )
                            .on_hover_text(&status);
                        });
                    });
                });
            });
        if toggle_members {
            self.show_members = !self.show_members;
        }
        if toggle_closed {
            self.session.as_mut().unwrap().set_closed(!closed);
        }
        if copy {
            ctx.copy_text(code);
            self.copied_until = Some(std::time::Instant::now() + Duration::from_secs(2));
            ctx.request_repaint_after(Duration::from_millis(2100));
        }
    }

    fn composer(&mut self, ctx: &egui::Context) {
        let light = self.profile.light_theme;
        let ready = self.session.as_ref().is_some_and(|s| s.ready());
        // Важно: ID текстового поля не должен совпадать с ID панели "composer",
        // иначе egui хранит фокус и состояние панели в одной ячейке памяти.
        let composer_id = egui::Id::new("composer-input");
        if std::mem::take(&mut self.focus_composer) {
            ctx.memory_mut(|m| m.request_focus(composer_id));
        }
        let focused = ctx.memory(|m| m.has_focus(composer_id));
        let shortcut = focused
            && ctx.input(|i| !i.modifiers.shift && !i.modifiers.alt)
            && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter));
        // Стрелка вверх в пустом поле — редактировать своё последнее сообщение.
        let edit_last = focused
            && self.draft.is_empty()
            && ctx.input(|i| i.modifiers == egui::Modifiers::NONE)
            && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp));
        if edit_last {
            let channel = self.channel.clone();
            if let Some(session) = &self.session {
                let own = session.own_key();
                if let Some(m) = session.history.messages.iter().rev().find(|m| {
                    m.channel == channel
                        && m.author_key == own
                        && m.image.is_none()
                        && m.file.is_none()
                }) {
                    self.draft = m.text.clone();
                    self.editing_message = Some(m.id);
                    self.focus_composer = true;
                }
            }
        }
        let cancel_edit = focused
            && self.editing_message.is_some()
            && ctx.input(|i| i.modifiers == egui::Modifiers::NONE)
            && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape));
        if cancel_edit {
            self.editing_message = None;
            self.draft.clear();
        }
        // Если сообщение исчезло (удалено или сменился канал) — сбрасываем режим правки.
        if let Some(id) = self.editing_message {
            let still_there = self.session.as_ref().is_some_and(|s| {
                s.history
                    .messages
                    .iter()
                    .any(|m| m.id == id && m.channel == self.channel)
            });
            if !still_there {
                self.editing_message = None;
                self.draft.clear();
            }
        }
        let hint = match self.editing_message {
            Some(_) => "Редактирование сообщения · Enter — сохранить, Esc — отмена".to_string(),
            None => format!("Написать в #{}", self.channel),
        };
        let mut send = false;
        let mut attach = false;
        egui::TopBottomPanel::bottom("composer-panel")
            .frame(Frame::new().fill(ui::bg(light)).inner_margin(Margin {
                left: 16,
                right: 16,
                top: 6,
                bottom: 16,
            }))
            .show_separator_line(false)
            .show(ctx, |ui| {
                Frame::new()
                    .fill(ui::input(light))
                    .corner_radius(10)
                    .inner_margin(Margin::symmetric(8, 6))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 8.0;
                            attach = ui::composer_btn(
                                ui,
                                light,
                                ui::Icon::Plus,
                                ui::Tone::Normal,
                                ready,
                            )
                            .on_hover_text("Прикрепить файл")
                            .clicked();

                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                let can_send = ready && !self.draft.trim().is_empty();
                                let clicked = ui::composer_send(ui, light, can_send)
                                    .on_hover_text(if self.editing_message.is_some() {
                                        "Сохранить изменения · Enter"
                                    } else {
                                        "Отправить · Enter"
                                    })
                                    .clicked();

                                ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                                    let response = ui.add_sized(
                                        [ui.available_width(), 40.0],
                                        egui::TextEdit::multiline(&mut self.draft)
                                            .id(composer_id)
                                            .frame(false)
                                            .desired_rows(1)
                                            .hint_text(ui::hint(hint, light))
                                            .char_limit(16000)
                                            .margin(vec2(4.0, 10.0)),
                                    );
                                    send = clicked
                                        || ((response.has_focus() || response.lost_focus())
                                            && shortcut);
                                });
                            });
                        });
                    });
            });
        if attach {
            if let Some(path) = rfd::FileDialog::new().pick_file() {
                let session = self.session.as_mut().unwrap();
                let result = if is_image_path(&path) {
                    session.image(&self.channel, path)
                } else {
                    session.file(&self.channel, path)
                };
                if let Err(e) = result {
                    self.error = e.to_string();
                }
            }
        }
        if send && ready && !self.draft.trim().is_empty() {
            let session = self.session.as_mut().unwrap();
            let result = match self.editing_message {
                Some(id) => session.edit_chat(id, &self.draft),
                None => session.chat(&self.channel, &self.draft),
            };
            match result {
                Ok(()) => {
                    let edited = self.editing_message.take().is_some();
                    self.draft.clear();
                    if !edited {
                        self.sound(thodtalk::sounds::Sound::Sent);
                    }
                    ctx.memory_mut(|m| m.request_focus(composer_id));
                }
                Err(e) => self.error = e.to_string(),
            }
        }
    }

    fn messages_ui(&mut self, ui: &mut egui::Ui) {
        let light = self.profile.light_theme;
        let channel = self.channel.clone();
        let session = self.session.as_ref().unwrap();
        // 0 — редактировать, 1 — удалить у себя, 2 — удалить у всех.
        let mut action: Option<(uuid::Uuid, u8)> = None;
        let mut save_existing: Option<(uuid::Uuid, String)> = None;
        let mut open_existing: Option<(uuid::Uuid, String, u64)> = None;
        egui::ScrollArea::vertical()
            .id_salt(("messages", &channel))
            .stick_to_bottom(true)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                Frame::new()
                    .inner_margin(Margin::symmetric(16, 12))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        let messages: Vec<_> = session
                            .history
                            .messages
                            .iter()
                            .filter(|m| m.channel == channel)
                            .collect();
                        let start = messages.len().saturating_sub(300);
                        let shown = &messages[start..];
                        if start == 0 {
                            ui.add_space(12.0);
                            let (rect, _) = ui.allocate_exact_size(Vec2::splat(68.0), Sense::hover());
                            ui.painter()
                                .circle_filled(rect.center(), 34.0, ui::selected(light));
                            ui::paint_icon(
                                ui.painter(),
                                ui::Icon::Hash,
                                Rect::from_center_size(rect.center(), Vec2::splat(34.0)),
                                ui::text(light),
                                false,
                            );
                            ui.add_space(10.0);
                            ui.label(ui::heading(format!("Добро пожаловать в #{channel}!"), 26.0));
                            ui.label(
                                RichText::new(format!(
                                    "Это начало канала #{channel}. Сообщения останутся только у вас и участников."
                                ))
                                .color(ui::muted(light)),
                            );
                            ui.add_space(8.0);
                        }
                        for (i, m) in shown.iter().enumerate() {
                            let first = i == 0 || {
                                let p = shown[i - 1];
                                p.author_key != m.author_key
                                    || m.timestamp_ms.abs_diff(p.timestamp_ms) > 300_000
                            };
                            ui.add_space(if i == 0 {
                                0.0
                            } else if first {
                                16.0
                            } else {
                                2.0
                            });
                            ui.horizontal_top(|ui| {
                                ui.spacing_mut().item_spacing = vec2(12.0, 0.0);
                                if first {
                                    let avatar =
                                        self.member_textures.get(&m.author_key).map(|(_, t)| t);
                                    ui::Avatar::new(avatar, &m.display_name, 38.0).show(ui);
                                } else {
                                    // Фиксированная колонка аватара, чтобы продолжение
                                    // сообщения начиналось ровно под текстом первого.
                                    let _ = ui.allocate_exact_size(Vec2::splat(38.0), Sense::hover());
                                }
                                ui.vertical(|ui| {
                                    ui.spacing_mut().item_spacing.y = 2.0;
                                    ui.set_max_width(ui.available_width());
                                    if first {
                                        ui.horizontal(|ui| {
                                            ui.spacing_mut().item_spacing.x = 8.0;
                                            ui.label(
                                                ui::heading(&m.display_name, 15.0)
                                                    .color(ui::name_color(&m.display_name, light)),
                                            )
                                            .on_hover_text(format!(
                                                "Ключ профиля: {}",
                                                key_string(&m.author_key)
                                            ));
                                            let seconds = m.timestamp_ms / 1000;
                                            ui.label(
                                                RichText::new(format!(
                                                    "{:02}:{:02}",
                                                    seconds / 3600 % 24,
                                                    seconds / 60 % 60
                                                ))
                                                .size(11.5)
                                                .color(ui::muted(light)),
                                            )
                                            .on_hover_text("Время по UTC");
                                            if m.edited {
                                                ui.label(
                                                    RichText::new("изменено")
                                                        .size(11.5)
                                                        .italics()
                                                        .color(ui::muted(light)),
                                                );
                                            }
                                        });
                                    }
                                    if let Some(id) = m.image {
                                        if let Some(response) = show_image(
                                            ui,
                                            &self.store,
                                            &mut self.image_textures,
                                            id,
                                        ) {
                                            if response.clicked() {
                                                self.preview_image = Some(id);
                                            }
                                            let own = m.author_key == session.own_key();
                                            let can_delete_all = own || session.hosting();
                                            response.context_menu(|ui| {
                                                if ui.button("Сохранить на компьютер").clicked() {
                                                    if let Err(e) =
                                                        save_attachment(&self.store, id)
                                                    {
                                                        self.error = e.to_string();
                                                    }
                                                    ui.close_menu();
                                                }
                                                ui.separator();
                                                if ui.button("Удалить у себя").clicked() {
                                                    action = Some((m.id, 1));
                                                    ui.close_menu();
                                                }
                                                if can_delete_all
                                                    && ui.button("Удалить у всех").clicked()
                                                {
                                                    action = Some((m.id, 2));
                                                    ui.close_menu();
                                                }
                                            });
                                        } else {
                                            ui.add(
                                                egui::Label::new(
                                                    RichText::new("Изображение недоступно")
                                                        .size(14.0)
                                                        .color(ui::muted(light)),
                                                )
                                                .wrap(),
                                            );
                                        }
                                    } else if let Some(file) = &m.file {
                                        let own = m.author_key == session.own_key();
                                        let can_delete_all = own || session.hosting();
                                        let card = Frame::new()
                                            .fill(ui::selected(light))
                                            .corner_radius(8)
                                            .inner_margin(Margin::symmetric(12, 9))
                                            .show(ui, |ui| {
                                                ui.horizontal(|ui| {
                                                    ui.spacing_mut().item_spacing.x = 10.0;
                                                    let (rect, _) = ui.allocate_exact_size(
                                                        Vec2::splat(34.0),
                                                        Sense::hover(),
                                                    );
                                                    ui::paint_icon(
                                                        ui.painter(),
                                                        ui::Icon::File,
                                                        rect.shrink(3.0),
                                                        ui::muted(light),
                                                        false,
                                                    );
                                                    ui.vertical(|ui| {
                                                        ui.spacing_mut().item_spacing.y = 1.0;
                                                        ui.label(
                                                            RichText::new(&file.name)
                                                                .size(14.0)
                                                                .strong(),
                                                        )
                                                        .on_hover_text(&file.name);
                                                        ui.label(
                                                            RichText::new(file_size(file.size))
                                                                .size(11.5)
                                                                .color(ui::muted(light)),
                                                        );
                                                    });
                                                    ui.with_layout(
                                                        Layout::right_to_left(Align::Center),
                                                        |ui| {
                                                            let (rect, response) = ui
                                                                .allocate_exact_size(
                                                                    Vec2::splat(26.0),
                                                                    Sense::hover(),
                                                                );
                                                            ui::paint_icon(
                                                                ui.painter(),
                                                                ui::Icon::Download,
                                                                rect.shrink(4.0),
                                                                ui::muted(light),
                                                                false,
                                                            );
                                                            response.on_hover_text(
                                                                "Нажмите на карточку, чтобы открыть или сохранить",
                                                            );
                                                        },
                                                    );
                                                });
                                            })
                                            .response;
                                        let response = ui.interact(
                                            card.rect,
                                            ui.id().with(("file-card", m.id)),
                                            Sense::click(),
                                        );
                                        if response.hovered() {
                                            ui.ctx()
                                                .set_cursor_icon(egui::CursorIcon::PointingHand);
                                            ui.painter().rect_stroke(
                                                card.rect,
                                                egui::CornerRadius::same(8),
                                                egui::Stroke::new(1.0_f32, ui::accent(light)),
                                                egui::StrokeKind::Inside,
                                            );
                                        }
                                        if response.clicked() {
                                            open_existing =
                                                Some((m.id, file.name.clone(), file.size));
                                        }
                                        response.context_menu(|ui| {
                                            if ui.button("Сохранить на компьютер").clicked() {
                                                save_existing =
                                                    Some((m.id, file.name.clone()));
                                                ui.close_menu();
                                            }
                                            ui.separator();
                                            if ui.button("Удалить у себя").clicked() {
                                                action = Some((m.id, 1));
                                                ui.close_menu();
                                            }
                                            if can_delete_all
                                                && ui.button("Удалить у всех").clicked()
                                            {
                                                action = Some((m.id, 2));
                                                ui.close_menu();
                                            }
                                        });
                                    } else {
                                        let response = ui.add(
                                            egui::Label::new(RichText::new(&m.text).size(14.5))
                                                .wrap()
                                                .sense(Sense::click()),
                                        );
                                        let own = m.author_key == session.own_key();
                                        let can_delete_all = own || session.hosting();
                                        response.context_menu(|ui| {
                                            if own && ui.button("Редактировать").clicked() {
                                                action = Some((m.id, 0));
                                                ui.close_menu();
                                            }
                                            if ui.button("Удалить у себя").clicked() {
                                                action = Some((m.id, 1));
                                                ui.close_menu();
                                            }
                                            if can_delete_all
                                                && ui.button("Удалить у всех").clicked()
                                            {
                                                action = Some((m.id, 2));
                                                ui.close_menu();
                                            }
                                        });
                                    }
                                });
                            });
                        }
                        ui.add_space(4.0);
                    });
            });
        if let Some((id, name)) = save_existing {
            if let Err(e) = save_file(&self.store, id, &name) {
                self.error = e.to_string();
            }
        }
        if let Some((id, name, size)) = open_existing {
            self.file_preview = Some((id, name, size));
        }
        if let Some((id, kind)) = action {
            match kind {
                0 => {
                    if let Some(session) = &self.session {
                        if let Some(m) = session.history.messages.iter().find(|m| m.id == id) {
                            self.draft = m.text.clone();
                            self.editing_message = Some(id);
                            self.focus_composer = true;
                        }
                    }
                }
                1 => {
                    if let Some(session) = &mut self.session {
                        if let Err(e) = session.delete_chat(id, false) {
                            self.error = e.to_string();
                        }
                    }
                }
                _ => {
                    if let Some(session) = &mut self.session {
                        if let Err(e) = session.delete_chat(id, true) {
                            self.error = e.to_string();
                        }
                    }
                }
            }
        }
    }

    // ───────────────────────── Главная и архив ─────────────────────────

    fn home_ui(&mut self, ui: &mut egui::Ui) {
        let light = self.profile.light_theme;
        let busy = self.connecting.is_some();
        let enabled = self.profile.network_allowed && !busy;
        let mut join = false;
        let mut host = false;
        let mut cancel = false;
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let width = (ui.available_width() - 48.0).clamp(280.0, 480.0);
                let side = ((ui.available_width() - width) / 2.0).max(0.0);
                ui.add_space(48.0);
                ui.horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    ui.add_space(side);
                    ui.vertical(|ui| {
                        ui.set_width(width);
                        ui.spacing_mut().item_spacing.y = 8.0;
                        ui.vertical_centered(|ui| {
                            ui.image((self.brand.id(), vec2(72.0, 72.0)));
                            ui.add_space(6.0);
                            ui.label(ui::heading("Добро пожаловать в ThodTalk", 26.0));
                            ui.label(
                                RichText::new("Голосовой и текстовый чат с друзьями")
                                    .color(ui::muted(light)),
                            );
                        });
                        ui.add_space(20.0);
                        if busy {
                            ui::card(ui, light, |ui| {
                                ui.horizontal(|ui| {
                                    ui.spinner();
                                    ui.label(ui::heading("Подключаемся…", 17.0));
                                });
                                ui.label(
                                    RichText::new("В сложной сети это может занять до минуты.")
                                        .color(ui::muted(light)),
                                );
                                ui.add_space(6.0);
                                if ui.add(ui::secondary("Отменить")).clicked() {
                                    cancel = true;
                                }
                            });
                            return;
                        }
                        if !self.profile.network_allowed {
                            ui.label(
                                RichText::new("Для подключения разрешите сеть в настройках.")
                                    .color(ui::danger(light)),
                            );
                        }
                        // Quick join last server
                        if let Some(last_code) = self.profile.last_server_code.clone() {
                            if !last_code.is_empty() {
                                let code_to_join = last_code.clone();
                                ui::card(ui, light, |ui| {
                                    ui.label(ui::heading("Быстрое подключение", 17.0));
                                    ui.label(
                                        RichText::new("Переподключиться к последнему серверу")
                                            .color(ui::muted(light)),
                                    );
                                    ui.add_space(6.0);
                                    if ui.add_enabled(enabled, ui::primary("Присоединиться")).clicked() {
                                        self.code = code_to_join;
                                        self.connect(false);
                                    }
                                });
                            }
                        }
                        ui::card(ui, light, |ui| {
                            ui.label(ui::heading("Войти по приглашению", 17.0));
                            ui.label(
                                RichText::new("Вставьте код, который прислал друг")
                                    .color(ui::muted(light)),
                            );
                            ui.add_space(6.0);
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 8.0;
                                let w = (ui.available_width() - 128.0).max(100.0);
                                let response = ui.add_sized(
                                    [w, 38.0],
                                    egui::TextEdit::singleline(&mut self.code)
                                        .hint_text(ui::hint("Код сервера", light))
                                        .margin(vec2(12.0, 10.0)),
                                );
                                let submit = response.lost_focus()
                                    && ui.input(|i| i.key_pressed(egui::Key::Enter));
                                let ok = enabled && !self.code.trim().is_empty();
                                join = ui
                                    .add_enabled(
                                        ok,
                                        ui::primary("Подключиться").min_size(vec2(120.0, 38.0)),
                                    )
                                    .clicked()
                                    || (submit && ok);
                            });
                        });
                        ui::card(ui, light, |ui| {
                            ui.label(ui::heading("Создать свой сервер", 17.0));
                            ui.label(
                                RichText::new(
                                    "Запустите сервер на этом компьютере и поделитесь кодом. Он работает, пока приложение открыто.",
                                )
                                .color(ui::muted(light)),
                            );
                            ui.add_space(6.0);
                            host = ui
                                .add_enabled(enabled, ui::secondary("Запустить сервер"))
                                .clicked();
                        });
                    });
                });
            });
        if cancel {
            if let Some(cancel) = self.cancel_connect.take() {
                cancel.store(true, Ordering::Relaxed);
            }
            self.connecting.take();
        }
        if join {
            self.connect(false);
        }
        if host {
            self.connect(true);
        }
    }

    fn archive_ui(&mut self, ui: &mut egui::Ui) {
        let light = self.profile.light_theme;
        if self.archives.is_none() {
            self.load_archives();
        }
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                Frame::new()
                    .inner_margin(Margin::symmetric(32, 28))
                    .show(ui, |ui| {
                        ui.set_max_width(720.0);
                        ui.label(ui::heading("Сохранённые переписки", 24.0));
                        ui.label(
                            RichText::new(
                                "Доступны без подключения. Показаны последние 300 сообщений.",
                            )
                            .color(ui::muted(light)),
                        );
                        ui.add_space(16.0);
                        match &self.archives {
                            None => {}
                            Some(histories) if histories.is_empty() => {
                                ui.add_space(40.0);
                                ui.vertical_centered(|ui| {
                                    let (rect, _) =
                                        ui.allocate_exact_size(Vec2::splat(56.0), Sense::hover());
                                    ui::paint_icon(
                                        ui.painter(),
                                        ui::Icon::Archive,
                                        rect,
                                        ui::muted(light),
                                        false,
                                    );
                                    ui.add_space(8.0);
                                    ui.label(ui::heading("Пока ничего нет", 18.0));
                                    ui.label(
                                        RichText::new(
                                            "Здесь появятся переписки после первого разговора.",
                                        )
                                        .color(ui::muted(light)),
                                    );
                                });
                            }
                            Some(histories) => {
                                for (name, history) in histories {
                                    ui::card(ui, light, |ui| {
                                        ui.collapsing(RichText::new(name).size(15.0), |ui| {
                                            ui.add_space(6.0);
                                            for m in history
                                                .messages
                                                .iter()
                                                .rev()
                                                .take(300)
                                                .collect::<Vec<_>>()
                                                .into_iter()
                                                .rev()
                                            {
                                                ui.horizontal(|ui| {
                                                    ui.spacing_mut().item_spacing.x = 8.0;
                                                    ui.label(
                                                        ui::heading(&m.display_name, 14.0).color(
                                                            ui::name_color(&m.display_name, light),
                                                        ),
                                                    );
                                                    ui.label(
                                                        RichText::new(format!("#{}", m.channel))
                                                            .size(12.0)
                                                            .color(ui::muted(light)),
                                                    );
                                                });
                                                if let Some(id) = m.image {
                                                    if let Some(response) = show_image(
                                                        ui,
                                                        &self.store,
                                                        &mut self.image_textures,
                                                        id,
                                                    ) {
                                                        if response.clicked() {
                                                            self.preview_image = Some(id);
                                                        }
                                                        response.context_menu(|ui| {
                                                            if ui
                                                                .button("Сохранить на компьютер")
                                                                .clicked()
                                                            {
                                                                if let Err(e) =
                                                                    save_attachment(&self.store, id)
                                                                {
                                                                    self.error = e.to_string();
                                                                }
                                                                ui.close_menu();
                                                            }
                                                        });
                                                    } else {
                                                        ui.label(
                                                            RichText::new("Изображение недоступно")
                                                                .size(14.0)
                                                                .color(ui::muted(light)),
                                                        );
                                                    }
                                                } else {
                                                    ui.label(&m.text);
                                                }
                                                ui.add_space(6.0);
                                            }
                                        });
                                    });
                                    ui.add_space(8.0);
                                }
                            }
                        }
                    });
            });
    }

    // ───────────────────────── Настройки ─────────────────────────

    fn settings_screen(&mut self, ctx: &egui::Context) {
        let light = self.profile.light_theme;
        let can_close = self.saved;
        if can_close
            && self.choosing_key.is_none()
            && ctx.input(|i| i.key_pressed(egui::Key::Escape))
        {
            self.close_settings(ctx);
            return;
        }
        self.error_bar(ctx);
        egui::SidePanel::left("settings_nav")
            .exact_width(250.0)
            .resizable(false)
            .frame(
                Frame::new()
                    .fill(ui::sidebar(light))
                    .inner_margin(Margin::symmetric(12, 24)),
            )
            .show(ctx, |ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                ui::section(ui, "НАСТРОЙКИ", light);
                ui.add_space(4.0);
                for (i, (icon, name)) in SETTINGS_TABS.iter().enumerate() {
                    if ui::nav_row(ui, light, *icon, name, self.settings_tab == i, None).clicked() {
                        self.settings_tab = i;
                    }
                }
                ui.with_layout(Layout::bottom_up(Align::LEFT), |ui| {
                    ui.horizontal(|ui| {
                        ui.add_space(10.0);
                        ui.label(
                            RichText::new(format!("ThodTalk {}", env!("CARGO_PKG_VERSION")))
                                .size(11.5)
                                .color(ui::muted(light)),
                        );
                    });
                    ui.horizontal(|ui| {
                        ui.add_space(10.0);
                        ui.hyperlink_to("GitHub · ThodProd", "https://github.com/ThodProd");
                    });
                });
            });

        let mut do_save = false;
        let mut do_cancel = false;
        egui::TopBottomPanel::bottom("settings_save")
            .frame(
                Frame::new()
                    .fill(ui::sidebar(light))
                    .inner_margin(Margin::symmetric(32, 12)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("Настройки сохранятся на этом компьютере")
                            .size(12.5)
                            .color(ui::muted(light)),
                    );
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui
                            .add_enabled(!self.storage_error, ui::primary("Сохранить"))
                            .clicked()
                        {
                            do_save = true;
                        }
                        if can_close && ui.add(ui::secondary("Отмена")).clicked() {
                            do_cancel = true;
                        }
                    });
                });
            });
        egui::CentralPanel::default()
            .frame(
                Frame::new()
                    .fill(ui::bg(light))
                    .inner_margin(Margin::symmetric(40, 28)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(ui::heading(SETTINGS_TABS[self.settings_tab].1, 24.0));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if can_close
                            && ui::icon_btn_sized(
                                ui,
                                light,
                                ui::Icon::Close,
                                ui::Tone::Normal,
                                false,
                                36.0,
                            )
                            .on_hover_text("Закрыть · Esc")
                            .clicked()
                        {
                            do_cancel = true;
                        }
                    });
                });
                ui.add_space(16.0);
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.set_max_width(620.0);
                        self.settings_tab_ui(ui);
                    });
            });
        if do_save {
            self.save_profile();
        } else if do_cancel {
            self.close_settings(ctx);
        }
    }

    fn settings_tab_ui(&mut self, ui: &mut egui::Ui) {
        let light = self.profile.light_theme;
        let muted = ui::muted(light);
        match self.settings_tab {
            0 => {
                let mut choose = false;
                ui::card(ui, light, |ui| {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 20.0;
                        let resp = ui::Avatar::new(self.avatar.as_ref(), &self.profile.name, 96.0)
                            .clickable(ui::Icon::Image)
                            .show(ui);
                        choose = resp.on_hover_text("Сменить фото").clicked();
                        ui.vertical(|ui| {
                            ui::field_label(ui, "ОТОБРАЖАЕМОЕ ИМЯ", light);
                            ui.add_sized(
                                [ui.available_width(), 38.0],
                                egui::TextEdit::singleline(&mut self.profile.name)
                                    .hint_text(ui::hint("Как к вам обращаться", light))
                                    .char_limit(40)
                                    .margin(vec2(12.0, 10.0)),
                            );
                            ui.add_space(6.0);
                            ui.label(
                                RichText::new(
                                    "Нажмите на фото, чтобы сменить. JPG, PNG, WebP · до 20 МБ",
                                )
                                .size(12.0)
                                .color(muted),
                            );
                        });
                    });
                });
                if choose {
                    if let Some(path) = rfd::FileDialog::new()
                        .add_filter("Изображения", &["png", "jpg", "jpeg", "webp", "gif", "bmp"])
                        .pick_file()
                    {
                        match avatar_from_file(&path) {
                            Ok(bytes) => {
                                self.profile.avatar = bytes;
                                self.update_avatar(ui.ctx());
                            }
                            Err(e) => self.error = e.to_string(),
                        }
                    }
                }
            }
            1 => {
                let mut theme_changed = false;
                ui::card(ui, light, |ui| {
                    ui::setting_row(
                        ui,
                        light,
                        "Светлая тема",
                        "Переключает светлое и тёмное оформление",
                        |ui| {
                            theme_changed = ui::toggle(ui, &mut self.profile.light_theme).changed();
                        },
                    );
                    ui::divider(ui, light);
                    ui::setting_row(
                        ui,
                        light,
                        "Звуки",
                        "Сообщения, вход и выход из голосового канала",
                        |ui| {
                            ui::toggle(ui, &mut self.profile.sounds);
                        },
                    );
                });
                if theme_changed {
                    ui::theme(ui.ctx(), self.profile.light_theme);
                }
                ui.add_space(10.0);
                ui.label(
                    RichText::new("Профиль и переписки хранятся только на этом компьютере.")
                        .size(12.5)
                        .color(muted),
                );
            }
            2 => {
                let desc = if self.ptt {
                    format!(
                        "Микрофон работает, только пока удерживается {}",
                        self.profile.ptt_key
                    )
                } else {
                    "Микрофон включён постоянно, пока вы в голосовом канале".to_string()
                };
                ui::card(ui, light, |ui| {
                    ui::field_label(ui, "МИКРОФОН", light);
                    device_combo(ui, "input-device", &mut self.input_device, &self.inputs);
                    ui.add_space(14.0);
                    ui::field_label(ui, "ДИНАМИКИ ИЛИ НАУШНИКИ", light);
                    device_combo(ui, "output-device", &mut self.output_device, &self.outputs);
                    ui::divider(ui, light);
                    ui::setting_row(
                        ui,
                        light,
                        "Говорить по нажатию клавиши",
                        &desc,
                        |ui| {
                            ui::toggle(ui, &mut self.ptt);
                        },
                    );
                    ui::divider(ui, light);
                    ui::setting_row(
                        ui,
                        light,
                        "Шумоподавление (FFmpeg)",
                        "Удаляет фоновый шум через встроенный фильтр",
                        |ui| {
                            ui::toggle(ui, &mut self.profile.ffmpeg_noise_suppression);
                        },
                    );
                    ui::divider(ui, light);
                    ui::setting_row(
                        ui,
                        light,
                        "Улучшение голоса",
                        "Усиливает чистоту и яркость голоса",
                        |ui| {
                            ui::toggle(ui, &mut self.profile.voice_enhancement);
                        },
                    );
                });
                ui.add_space(10.0);
                ui.label(
                    RichText::new("Устройства сменятся при следующем входе в голосовой канал.")
                        .size(12.5)
                        .color(muted),
                );
            }
            3 => {
                let labels = [
                    "Говорить по нажатию",
                    "Микрофон: включить / выключить",
                    "Звук: включить / выключить",
                ];
                let values = [
                    self.profile.ptt_key.clone(),
                    self.profile.mute_key.clone(),
                    self.profile.deafen_key.clone(),
                ];
                let mut start = None;
                ui::card(ui, light, |ui| {
                    for (index, label) in labels.iter().enumerate() {
                        if index > 0 {
                            ui::divider(ui, light);
                        }
                        let choosing = self.choosing_key == Some(index);
                        ui::setting_row(ui, light, label, "", |ui| {
                            let caption = if choosing {
                                "Нажмите клавишу…".to_string()
                            } else {
                                values[index].clone()
                            };
                            let mut text = RichText::new(caption).monospace();
                            let mut button =
                                egui::Button::new(text.clone()).min_size(vec2(120.0, 32.0));
                            if choosing {
                                text = text.color(egui::Color32::WHITE);
                                button = egui::Button::new(text)
                                    .min_size(vec2(120.0, 32.0))
                                    .fill(ui::BLUE);
                            }
                            if ui.add(button).clicked() {
                                start = Some(index);
                            }
                        });
                    }
                });
                if start.is_some() {
                    self.choosing_key = start;
                }
                ui.add_space(10.0);
                ui.label(
                    RichText::new("Клавиши работают в активном окне. Esc — отменить назначение.")
                        .size(12.5)
                        .color(muted),
                );
                ui.add_space(14.0);
                ui::card(ui, light, |ui| {
                    ui::field_label(ui, "В ЧАТЕ", light);
                    for (key, what) in [
                        ("Enter", "Отправить сообщение"),
                        ("Shift + Enter", "Новая строка"),
                        ("Tab", "Переход между элементами"),
                    ] {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 12.0;
                            ui::keycap(ui, light, key);
                            ui.label(what);
                        });
                    }
                });
                if let Some(index) = self.choosing_key {
                    let pressed = ui.input(|i| {
                        i.events.iter().find_map(|e| match e {
                            egui::Event::Key {
                                key,
                                physical_key,
                                pressed: true,
                                repeat: false,
                                ..
                            } => Some(physical_key.unwrap_or(*key)),
                            _ => None,
                        })
                    });
                    if let Some(key) = pressed {
                        if key != egui::Key::Escape {
                            let name = key.name().to_string();
                            let others = [
                                &self.profile.ptt_key,
                                &self.profile.mute_key,
                                &self.profile.deafen_key,
                            ];
                            if [egui::Key::Enter, egui::Key::Tab].contains(&key)
                                || others
                                    .iter()
                                    .enumerate()
                                    .any(|(i, k)| i != index && **k == name)
                            {
                                self.error = "Эта клавиша уже занята".into();
                            } else {
                                match index {
                                    0 => self.profile.ptt_key = name,
                                    1 => self.profile.mute_key = name,
                                    _ => self.profile.deafen_key = name,
                                };
                            }
                        }
                        self.choosing_key = None;
                    }
                }
            }
            _ => {
                let mut check = false;
                let mut install = false;
                let status_color = if self.update_status.contains("Не удалось")
                    || self.update_status.contains("ошибка")
                {
                    ui::danger(light)
                } else {
                    muted
                };
                ui::card(ui, light, |ui| {
                    ui.label(ui::heading(
                        format!("ThodTalk {}", env!("CARGO_PKG_VERSION")),
                        18.0,
                    ));
                    if !self.update_status.is_empty() {
                        ui.label(
                            RichText::new(&self.update_status)
                                .size(13.0)
                                .color(status_color),
                        );
                    }
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if self.update_rx.is_some() {
                            ui.spinner();
                        }
                        if ui
                            .add_enabled(
                                self.update_rx.is_none(),
                                ui::secondary("Проверить обновления"),
                            )
                            .clicked()
                        {
                            check = true;
                        }
                        if self.update_ready.is_some()
                            && ui
                                .add_enabled(
                                    self.session.is_none() && self.connecting.is_none(),
                                    ui::primary("Установить и перезапустить"),
                                )
                                .clicked()
                        {
                            install = true;
                        }
                    });
                    ui::divider(ui, light);
                    ui::setting_row(
                        ui,
                        light,
                        "Устанавливать автоматически",
                        "Обновление применяется вне разговора, с перезапуском приложения.",
                        |ui| {
                            ui::toggle(ui, &mut self.profile.auto_update);
                        },
                    );
                });
                if check {
                    self.check_updates();
                }
                if install {
                    self.install_update(ui.ctx());
                }
            }
        }
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _: &mut eframe::Frame) {
        self.tick(ctx);
        self.dropped_files(ctx);
        self.drop_hint(ctx);
        if self.editing {
            self.settings_screen(ctx);
            self.member_card(ctx);
            self.image_preview(ctx);
            self.file_preview_window(ctx);
            self.screen_window(ctx);
            self.share_picker_window(ctx);
            return;
        }
        self.error_bar(ctx);
        self.sidebar_ui(ctx);
        if self.session.is_some() {
            self.chat_bar(ctx);
            if self.show_members {
                self.members_panel(ctx);
            }
            self.composer(ctx);
            egui::CentralPanel::default()
                .frame(egui::Frame::new().fill(ui::bg(self.profile.light_theme)))
                .show(ctx, |ui| self.messages_ui(ui));
        } else {
            egui::CentralPanel::default()
                .frame(egui::Frame::new().fill(ui::bg(self.profile.light_theme)))
                .show(ctx, |ui| {
                    if self.page == Page::Archive {
                        self.archive_ui(ui);
                    } else {
                        self.home_ui(ui);
                    }
                });
        }
        self.member_card(ctx);
        self.image_preview(ctx);
        self.file_preview_window(ctx);
        self.screen_window(ctx);
        self.share_picker_window(ctx);
    }
}
fn key_string(key: &[u8]) -> String {
    key.iter().map(|b| format!("{b:02x}")).collect()
}
fn messages_count(count: usize) -> String {
    let last_two = count % 100;
    let last = count % 10;
    let word = if (11..=14).contains(&last_two) {
        "сообщений"
    } else if last == 1 {
        "сообщение"
    } else if (2..=4).contains(&last) {
        "сообщения"
    } else {
        "сообщений"
    };
    format!("{count} {word}")
}
impl App {
    fn check_updates(&mut self) {
        if self.update_rx.is_some() {
            return;
        }
        self.update_started = true;
        self.update_status = "Проверяем GitHub и загружаем новую версию…".into();
        let root = self.store.root.clone();
        let (tx, rx) = mpsc::channel();
        self.update_rx = Some(rx);
        std::thread::spawn(move || {
            let _ = tx.send(thodtalk::updater::background_prepare(&root));
        });
    }
    fn install_update(&mut self, ctx: &egui::Context) {
        if let Some(update) = self.update_ready.take() {
            match thodtalk::updater::install(&update) {
                Ok(()) => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
                Err(e) => self.update_status = format!("Обновление не установлено: {e}"),
            }
        }
    }
    fn voice_state(&mut self) {
        if let Some(session) = &mut self.session {
            if let Err(e) = session.send(Payload::VoiceState {
                channel: self.voice_channel.clone(),
                muted: self.muted || self.deafened,
                deafened: self.deafened,
            }) {
                self.error = e.to_string();
            }
        }
    }
}
fn screen_image(bytes: &[u8]) -> Option<egui::ColorImage> {
    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .ok()?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(1920);
    limits.max_image_height = Some(1200);
    limits.max_alloc = Some(32 * 1024 * 1024);
    reader.limits(limits);
    let image = reader.decode().ok()?.to_rgba8();
    Some(egui::ColorImage::from_rgba_unmultiplied(
        [image.width() as usize, image.height() as usize],
        image.as_raw(),
    ))
}

fn texture(ctx: &egui::Context, name: &str, bytes: &[u8]) -> Option<egui::TextureHandle> {
    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .ok()?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(1280);
    limits.max_image_height = Some(1280);
    limits.max_alloc = Some(16 * 1024 * 1024);
    reader.limits(limits);
    let img = reader.decode().ok()?.to_rgba8();
    Some(ctx.load_texture(
        name,
        egui::ColorImage::from_rgba_unmultiplied(
            [img.width() as usize, img.height() as usize],
            img.as_raw(),
        ),
        egui::TextureOptions::LINEAR,
    ))
}
fn show_image(
    ui: &mut egui::Ui,
    store: &Store,
    textures: &mut std::collections::BTreeMap<uuid::Uuid, egui::TextureHandle>,
    id: uuid::Uuid,
) -> Option<egui::Response> {
    if !textures.contains_key(&id) {
        if textures.len() >= 32 {
            textures.clear();
        }
        if let Ok(Some(bytes)) = store.load::<Vec<u8>>(&format!("image-{id}")) {
            if let Some(image) = texture(ui.ctx(), &format!("image-{id}"), &bytes) {
                textures.insert(id, image);
            }
        }
    }
    let image = textures.get(&id)?;
    let source = image.size_vec2();
    let max = egui::vec2(ui.available_width().min(420.0), 300.0);
    let scale = (max.x / source.x).min(max.y / source.y).min(1.0);
    Some(
        ui.add(
            egui::Image::new((image.id(), source * scale))
                .corner_radius(8)
                .sense(Sense::click()),
        )
        .on_hover_cursor(egui::CursorIcon::ZoomIn),
    )
}

fn is_image_path(path: &std::path::Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            matches!(
                extension.to_ascii_lowercase().as_str(),
                "png" | "jpg" | "jpeg" | "webp" | "gif" | "bmp"
            )
        })
}

fn file_size(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    let size = bytes as f64;
    if size < KB {
        format!("{bytes} Б")
    } else if size < KB * KB {
        format!("{:.1} КБ", size / KB)
    } else {
        format!("{:.1} МБ", size / (KB * KB))
    }
}

fn safe_file_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c.is_control() || "<>:\"/\\|?*".contains(c) {
                '_'
            } else {
                c
            }
        })
        .collect();
    let cleaned = cleaned.trim().trim_end_matches('.').trim();
    if cleaned.is_empty() {
        return "file".into();
    }
    let stem = cleaned.split('.').next().unwrap_or("").to_ascii_uppercase();
    let reserved = matches!(
        stem.as_str(),
        "CON"
            | "PRN"
            | "AUX"
            | "NUL"
            | "COM1"
            | "COM2"
            | "COM3"
            | "COM4"
            | "COM5"
            | "COM6"
            | "COM7"
            | "COM8"
            | "COM9"
            | "LPT1"
            | "LPT2"
            | "LPT3"
            | "LPT4"
            | "LPT5"
            | "LPT6"
            | "LPT7"
            | "LPT8"
            | "LPT9"
    );
    let cleaned = cleaned.chars().take(120).collect::<String>();
    if reserved {
        format!("_{cleaned}")
    } else {
        cleaned
    }
}

fn save_file(store: &Store, id: uuid::Uuid, name: &str) -> anyhow::Result<()> {
    let bytes = thodtalk::attachments::load_bytes(store, id)?
        .ok_or_else(|| anyhow::anyhow!("Файл не найден"))?;
    if let Some(path) = rfd::FileDialog::new()
        .set_file_name(safe_file_name(name))
        .save_file()
    {
        std::fs::write(path, bytes)?;
    }
    Ok(())
}

fn open_file(store: &Store, id: uuid::Uuid, name: &str) -> anyhow::Result<()> {
    let bytes = thodtalk::attachments::load_bytes(store, id)?
        .ok_or_else(|| anyhow::anyhow!("Файл не найден"))?;
    let directory = std::env::temp_dir().join("thodtalk-open");
    std::fs::create_dir_all(&directory)?;
    let path = directory.join(format!("{id}-{}", safe_file_name(name)));
    std::fs::write(&path, &bytes)?;
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // Mark the extracted copy as downloaded so Windows can warn about
        // executable content before it runs.
        let _ = std::fs::write(
            format!("{}:Zone.Identifier", path.display()),
            "[ZoneTransfer]\r\nZoneId=3\r\n",
        );
        let target = path.to_string_lossy().into_owned();
        std::process::Command::new("cmd")
            .args(["/C", "start", ""])
            .arg(target)
            .creation_flags(0x0800_0000)
            .spawn()?;
    }
    #[cfg(not(windows))]
    {
        std::process::Command::new("xdg-open").arg(&path).spawn()?;
    }
    Ok(())
}

fn save_attachment(store: &Store, id: uuid::Uuid) -> anyhow::Result<()> {
    let bytes = store
        .load::<Vec<u8>>(&format!("image-{id}"))?
        .ok_or_else(|| anyhow::anyhow!("Изображение не найдено"))?;
    if let Some(path) = rfd::FileDialog::new()
        .set_file_name(format!("thodtalk-{id}.jpg"))
        .add_filter("JPEG", &["jpg", "jpeg"])
        .save_file()
    {
        std::fs::write(path, bytes)?;
    }
    Ok(())
}

fn device_combo(ui: &mut egui::Ui, id: &str, value: &mut String, options: &[String]) {
    egui::ComboBox::from_id_salt(id)
        .selected_text(if value.is_empty() {
            "По умолчанию"
        } else {
            value.as_str()
        })
        .width(ui.available_width())
        .show_ui(ui, |ui| {
            ui.selectable_value(value, String::new(), "По умолчанию");
            for name in options {
                ui.selectable_value(value, name.clone(), name);
            }
        });
}
fn main() {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let args: Vec<_> = std::env::args().collect();
    if args.get(1).is_some_and(|s| s == "--update-worker") {
        if thodtalk::updater::run_worker().is_err() {
            std::process::exit(1);
        }
        return;
    }
    if args.get(1).is_some_and(|s| s == "--apply-update") {
        let result = (|| -> anyhow::Result<()> {
            thodtalk::updater::apply(
                std::path::Path::new(
                    args.get(2)
                        .ok_or_else(|| anyhow::anyhow!("Нет плана обновления"))?,
                ),
                args.get(3)
                    .ok_or_else(|| anyhow::anyhow!("Нет процесса"))?
                    .parse()?,
            )
        })();
        if let Err(e) = result {
            rfd::MessageDialog::new()
                .set_title("Обновление ThodTalk")
                .set_description(format!("Обновление не установлено: {e:#}"))
                .show();
        }
        return;
    }
    if std::env::args().any(|arg| arg == "--session-worker") {
        if thodtalk::worker::run().is_err() {
            std::process::exit(1);
        }
        return;
    }
    if let Err(error) = run() {
        rfd::MessageDialog::new()
            .set_title("ThodTalk")
            .set_description(format!("{error:#}"))
            .set_level(rfd::MessageLevel::Error)
            .show();
    }
}
fn run() -> anyhow::Result<()> {
    let store = Store::open(Store::default_path()?)?;
    std::fs::create_dir_all(store.root.join("runtime"))?;
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(store.root.join("runtime/instance.lock"))?;
    lock.try_lock()
        .map_err(|_| anyhow::anyhow!("ThodTalk уже открыт с этим профилем"))?;
    let image = image::load_from_memory(include_bytes!(concat!(env!("OUT_DIR"), "/icon.png")))?
        .into_rgba8();
    let icon = egui::IconData {
        width: image.width(),
        height: image.height(),
        rgba: image.into_raw(),
    };
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_icon(icon)
            .with_inner_size([1100.0, 760.0])
            .with_min_inner_size([800.0, 600.0]),
        ..Default::default()
    };
    eframe::run_native(
        "ThodTalk",
        options,
        Box::new(move |cc| Ok(Box::new(App::new(cc, store, lock)))),
    )
    .map_err(|e| anyhow::anyhow!("{e}"))
}
