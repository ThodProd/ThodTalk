#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
use eframe::egui;
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

struct App {
    store: Store,
    profile: Profile,
    editing: bool,
    saved: bool,
    error: String,
    storage_error: bool,
    avatar: Option<egui::TextureHandle>,
    session: Option<Session>,
    connecting: Option<mpsc::Receiver<anyhow::Result<Session>>>,
    code: String,
    draft: String,
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
    speaking: std::collections::BTreeMap<Vec<u8>, std::time::Instant>,
    image_textures: std::collections::BTreeMap<uuid::Uuid, egui::TextureHandle>,
    update_rx: Option<mpsc::Receiver<anyhow::Result<Option<thodtalk::updater::Prepared>>>>,
    update_ready: Option<thodtalk::updater::Prepared>,
    update_status: String,
    update_started: bool,
    _lock: std::fs::File,
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
        let mut app = Self {
            store,
            profile,
            editing: !saved,
            saved,
            error,
            storage_error,
            avatar: None,
            session: None,
            connecting: None,
            code: String::new(),
            draft: String::new(),
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
            show_members: false,
            settings_tab: 0,
            choosing_key: None,
            user_card: None,
            speaking: Default::default(),
            image_textures: Default::default(),
            update_rx: None,
            update_ready: None,
            update_status: String::new(),
            update_started: false,
            _lock: lock,
        };
        app.input_device = app.profile.input_device.clone();
        app.output_device = app.profile.output_device.clone();
        app.ptt = app.profile.push_to_talk;
        app.profile.network_allowed = true;
        app.profile.microphone_allowed = true;
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
        let profile = self.profile.clone();
        let code = self.code.clone();
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
    fn stop_voice(&mut self) {
        if self.voice_channel.is_some() {
            self.sound(thodtalk::sounds::Sound::Leave);
        }
        self.audio.take();
        self.voice_channel = None;
        if let Some(session) = &mut self.session {
            let _ = session.send(Payload::VoiceState {
                channel: None,
                muted: true,
            });
        }
    }
    fn start_voice(&mut self) {
        if !self.profile.microphone_allowed {
            self.error = "Разрешите микрофон в профиле".into();
            return;
        }
        match Audio::start(&self.input_device, &self.output_device) {
            Ok(audio) => {
                self.sound(thodtalk::sounds::Sound::Join);
                self.audio = Some(audio);
                self.voice_channel = Some(self.channel.clone());
                if let Some(session) = &mut self.session {
                    let _ = session.send(Payload::VoiceState {
                        channel: self.voice_channel.clone(),
                        muted: self.muted,
                    });
                }
            }
            Err(e) => self.error = format!("Не удалось включить голос: {e}"),
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
                self.muted = !self.muted;
            }
            if deafen {
                self.deafened = !self.deafened;
            }
            if mute || deafen {
                self.voice_state();
            }
        }

        if let Some(rx) = &self.connecting {
            if let Ok(result) = rx.try_recv() {
                self.connecting = None;
                match result {
                    Ok(session) => {
                        self.session = Some(session);
                        self.code.clear();
                    }
                    Err(e) => self.error = format!("Подключение не удалось: {e}"),
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
                        thodtalk::sounds::play(thodtalk::sounds::Sound::Join);
                    } else if left {
                        thodtalk::sounds::play(thodtalk::sounds::Sound::Leave);
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
                    if self.voice_channel.as_ref() == Some(&channel) {
                        let gain = self
                            .profile
                            .volumes
                            .get(&key_string(&author))
                            .copied()
                            .unwrap_or(1.0);
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
            self.session.take();
        }
        if self.session.is_some() || self.connecting.is_some() {
            ctx.request_repaint_after(Duration::from_millis(16));
        }
    }
    fn profile_ui(&mut self, ui: &mut egui::Ui) {
        ui.heading("Настройки");
        ui.add_space(12.0);
        ui.horizontal_top(|ui| {
            ui.vertical(|ui| {
                ui.set_width(128.0);
                for (index, name) in ["Профиль", "Основное", "Звук", "Клавиши", "Обновления"]
                    .iter()
                    .enumerate()
                {
                    ui.selectable_value(&mut self.settings_tab, index, *name);
                }
            });
            ui.add_space(16.0);
            ui.vertical(|ui| {
                ui.set_max_width(500.0);
                match self.settings_tab {
                    0 => {
                        let mut choose = false;
                        ui.horizontal(|ui| {
                            ui::avatar(ui, self.avatar.as_ref(), &self.profile.name, 80.0);
                            ui.add_space(12.0);
                            ui.vertical(|ui| {
                                ui.label(ui::heading("Фотография профиля", 16.0));
                                choose = ui.button("Выбрать изображение").clicked();
                                ui.small("JPG, PNG, WebP · до 20 МБ");
                            });
                        });
                        if choose {
                            if let Some(path) = rfd::FileDialog::new()
                                .add_filter(
                                    "Изображения",
                                    &["png", "jpg", "jpeg", "webp", "gif", "bmp"],
                                )
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
                        ui.add_space(12.0);
                        ui.label("Отображаемое имя");
                        ui.add_sized(
                            [ui.available_width(), 32.0],
                            egui::TextEdit::singleline(&mut self.profile.name)
                                .hint_text(ui::hint(
                                    "Как к вам обращаться",
                                    !ui.visuals().dark_mode,
                                ))
                                .char_limit(40)
                                .margin(egui::vec2(10.0, 8.0)),
                        );
                    }
                    1 => {
                        if ui
                            .checkbox(&mut self.profile.light_theme, "Светлая тема")
                            .changed()
                        {
                            ui::theme(ui.ctx(), self.profile.light_theme);
                        }
                        ui.checkbox(
                            &mut self.profile.sounds,
                            "Звуки сообщений и голосового чата",
                        );
                        ui.weak("Профиль и переписки хранятся на этом компьютере.");
                    }
                    2 => {
                        egui::ComboBox::from_label("Микрофон")
                            .selected_text(if self.input_device.is_empty() {
                                "По умолчанию"
                            } else {
                                &self.input_device
                            })
                            .show_ui(ui, |ui| {
                                ui.selectable_value(
                                    &mut self.input_device,
                                    String::new(),
                                    "По умолчанию",
                                );
                                for name in &self.inputs {
                                    ui.selectable_value(&mut self.input_device, name.clone(), name);
                                }
                            });
                        egui::ComboBox::from_label("Наушники / динамики")
                            .selected_text(if self.output_device.is_empty() {
                                "По умолчанию"
                            } else {
                                &self.output_device
                            })
                            .show_ui(ui, |ui| {
                                ui.selectable_value(
                                    &mut self.output_device,
                                    String::new(),
                                    "По умолчанию",
                                );
                                for name in &self.outputs {
                                    ui.selectable_value(
                                        &mut self.output_device,
                                        name.clone(),
                                        name,
                                    );
                                }
                            });

                        ui.checkbox(&mut self.ptt, "Говорить при удержании клавиши");
                        ui.weak("Изменения устройств применяются при следующем входе в голос.");
                    }
                    3 => {
                        ui.weak("Клавиши действуют в активном окне. Esc — отменить назначение.");
                        for (index, label) in [
                            "Разговор по нажатию",
                            "Микрофон: вкл. / выкл.",
                            "Звук: вкл. / выкл.",
                        ]
                        .iter()
                        .enumerate()
                        {
                            let value = match index {
                                0 => &self.profile.ptt_key,
                                1 => &self.profile.mute_key,
                                _ => &self.profile.deafen_key,
                            };
                            ui.horizontal(|ui| {
                                ui.label(*label);
                                if ui
                                    .button(if self.choosing_key == Some(index) {
                                        "Нажмите клавишу…"
                                    } else {
                                        value
                                    })
                                    .clicked()
                                {
                                    self.choosing_key = Some(index);
                                }
                            });
                        }
                        ui.separator();
                        ui.label("Enter — отправить сообщение");
                        ui.label("Shift+Enter — новая строка");
                        ui.label("Tab / Shift+Tab — переход между элементами");
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
                        ui.label(format!("Установлена версия {}", env!("CARGO_PKG_VERSION")));
                        ui.checkbox(
                            &mut self.profile.auto_update,
                            "Загружать и устанавливать обновления автоматически",
                        );
                        ui.weak("Обновление применяется вне разговора, с перезапуском приложения.");
                        if ui
                            .add_enabled(
                                self.update_rx.is_none(),
                                egui::Button::new("Проверить обновления"),
                            )
                            .clicked()
                        {
                            self.check_updates();
                        }
                        ui.label(&self.update_status);
                        if self.update_ready.is_some()
                            && ui
                                .add_enabled(
                                    self.session.is_none() && self.connecting.is_none(),
                                    egui::Button::new("Установить и перезапустить"),
                                )
                                .clicked()
                        {
                            self.install_update(ui.ctx());
                        }
                    }
                }
            });
        });
        ui.add_space(16.0);
        ui.hyperlink_to("GitHub · ThodProd", "https://github.com/ThodProd");
        if ui
            .add_enabled(!self.storage_error, ui::primary("Сохранить профиль"))
            .clicked()
        {
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
    }

    fn home_ui(&mut self, ui: &mut egui::Ui) {
        ui.heading("Ваши разговоры");
        ui.label(
            egui::RichText::new("Подключитесь к друзьям или соберите свою компанию.")
                .color(ui::muted(self.profile.light_theme)),
        );
        ui.add_space(24.0);
        let enabled = self.profile.network_allowed && self.connecting.is_none();
        if !self.profile.network_allowed {
            ui.label("Для подключения разрешите сеть в профиле.");
        }
        let mut join = false;
        let mut host = false;
        egui::Frame::new()
            .fill(ui::surface(self.profile.light_theme))
            .corner_radius(14)
            .inner_margin(24)
            .show(ui, |ui| {
                ui.set_width((ui.available_width()).min(580.0));
                ui.label(ui::heading("Войти по приглашению", 20.0));
                ui.label(
                    egui::RichText::new("Один код от друга — и вы в общей комнате.")
                        .color(ui::muted(self.profile.light_theme)),
                );
                ui.add_space(8.0);
                ui.add_sized(
                    [ui.available_width(), 46.0],
                    egui::TextEdit::singleline(&mut self.code)
                        .hint_text(ui::hint("Вставьте код сервера", !ui.visuals().dark_mode))
                        .margin(egui::vec2(14.0, 13.0)),
                );
                join = ui
                    .add_enabled(
                        enabled && !self.code.trim().is_empty(),
                        ui::primary("Подключиться"),
                    )
                    .clicked();
            });
        ui.add_space(24.0);
        ui.label(ui::heading("Пригласить своих друзей", 20.0));
        ui.label(
            egui::RichText::new("Запустите сервер на этом компьютере и поделитесь кодом.")
                .color(ui::muted(self.profile.light_theme)),
        );
        host |= ui
            .add_enabled(
                enabled,
                egui::Button::new("Запустить сервер").min_size(egui::vec2(0.0, 30.0)),
            )
            .clicked();
        ui.small("Сервер доступен, пока приложение открыто.");
        if join {
            self.connect(false);
        }
        if host {
            self.connect(true);
        }
        if self.connecting.is_some() {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Подключаемся… В сложной сети это может занять до минуты.");
            });
        }
        if self.connecting.is_some() && ui.button("Отменить подключение").clicked()
        {
            if let Some(cancel) = self.cancel_connect.take() {
                cancel.store(true, Ordering::Relaxed);
            }
            self.connecting.take();
        }
        ui.add_space(24.0);
        if ui.button("Сохранённые переписки").clicked() {
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
                                format!("{} · {} сообщений", m.display_name, history.messages.len())
                            })
                            .unwrap_or_else(|| "Пустая переписка".into());
                        histories.push((format!("{} · {}", title, &id.to_string()[..8]), history));
                    }
                }
                Ok(histories)
            })();
            match result {
                Ok(histories) => self.archives = Some(histories),
                Err(e) => self.error = e.to_string(),
            }
        }
        if let Some(histories) = &self.archives {
            egui::ScrollArea::vertical()
                .max_height(ui.available_height())
                .show(ui, |ui| {
                    if histories.is_empty() {
                        ui.weak("Пока нет сохранённых сообщений.");
                    }
                    for (name, history) in histories {
                        ui.collapsing(name, |ui| {
                            ui.small("Последние 300 сообщений. История доступна без подключения.");
                            for message in history
                                .messages
                                .iter()
                                .rev()
                                .take(300)
                                .collect::<Vec<_>>()
                                .into_iter()
                                .rev()
                            {
                                ui.strong(format!(
                                    "{} · #{}",
                                    message.display_name, message.channel
                                ));
                                ui.label(&message.text);
                                if let Some(id) = message.image {
                                    if !self.image_textures.contains_key(&id) {
                                        if self.image_textures.len() >= 32 {
                                            self.image_textures.clear();
                                        }
                                        if let Ok(Some(bytes)) =
                                            self.store.load::<Vec<u8>>(&format!("image-{id}"))
                                        {
                                            if let Some(img) =
                                                texture(ui.ctx(), &format!("image-{id}"), &bytes)
                                            {
                                                self.image_textures.insert(id, img);
                                            }
                                        }
                                    }
                                    if let Some(img) = self.image_textures.get(&id) {
                                        ui.add(
                                            egui::Image::new(img)
                                                .max_size(egui::vec2(
                                                    ui.available_width().min(400.0),
                                                    280.0,
                                                ))
                                                .corner_radius(8),
                                        );
                                    }
                                }
                            }
                        });
                    }
                });
        }
    }
    fn chat_ui(&mut self, ui: &mut egui::Ui) {
        let light = self.profile.light_theme;
        let mut send = false;
        let mut attach = false;
        let session = self.session.as_mut().unwrap();
        ui.horizontal_wrapped(|ui| {
            ui.label(egui::RichText::new("#").size(28.0).color(ui::muted(light)));
            ui.heading(&self.channel);
            ui.label(
                egui::RichText::new("Сквозное шифрование")
                    .size(11.0)
                    .color(ui::muted(light)),
            );
            if ui
                .add(
                    egui::Button::new("👤")
                        .frame(false)
                        .min_size(egui::vec2(28.0, 28.0)),
                )
                .on_hover_text(format!("Участники: {}", session.members.len()))
                .clicked()
            {
                self.show_members = !self.show_members;
            }
        });
        ui.label(
            egui::RichText::new(&session.status)
                .size(12.0)
                .color(ui::muted(light)),
        );
        if session.hosting() {
            ui.horizontal_wrapped(|ui| {
                let copied = self
                    .copied_until
                    .is_some_and(|until| until > std::time::Instant::now());
                if ui
                    .add_enabled(
                        !session.code.is_empty(),
                        egui::Button::new(if copied {
                            "Код скопирован"
                        } else {
                            "Пригласить друзей"
                        }),
                    )
                    .on_hover_text("Общий код действует до остановки сервера")
                    .clicked()
                {
                    ui.ctx().copy_text(session.code.clone());
                    self.copied_until = Some(std::time::Instant::now() + Duration::from_secs(2));
                }
                let mut closed = session.closed;
                if ui.checkbox(&mut closed, "Закрыть вход").changed() {
                    session.set_closed(closed);
                }
            });
        }
        ui.add_space(4.0);
        ui.separator();
        ui.add_space(4.0);
        let height = (ui.available_height() - 100.0).max(80.0);
        egui::ScrollArea::vertical()
            .id_salt(("messages", &self.channel))
            .stick_to_bottom(true)
            .max_height(height)
            .min_scrolled_height(height)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let messages: Vec<_> = session
                    .history
                    .messages
                    .iter()
                    .filter(|m| m.channel == self.channel)
                    .collect();
                if messages.is_empty() {
                    ui.add_space((height / 2.0 - 70.0).max(16.0));
                    ui.vertical_centered(|ui| {
                        ui.label(egui::RichText::new("#").size(44.0).color(ui::accent(light)));
                        ui.label(ui::heading("Начните разговор", 24.0));
                        ui.label(
                            egui::RichText::new("Сообщения этого канала останутся у вас.")
                                .color(ui::muted(light)),
                        );
                    });
                }
                for message in messages
                    .iter()
                    .rev()
                    .take(300)
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                {
                    ui.horizontal_top(|ui| {
                        let avatar = self
                            .member_textures
                            .get(&message.author_key)
                            .map(|(_, image)| image);
                        ui::avatar(ui, avatar, &message.display_name, 34.0);
                        ui.vertical(|ui| {
                            ui.set_max_width(ui.available_width());
                            ui.horizontal(|ui| {
                                ui.label(ui::heading(&message.display_name, 14.0))
                                    .on_hover_text(format!(
                                        "Ключ профиля: {}",
                                        message
                                            .author_key
                                            .iter()
                                            .map(|b| format!("{b:02x}"))
                                            .collect::<String>()
                                    ));
                                let seconds = message.timestamp_ms / 1000;
                                ui.label(
                                    egui::RichText::new(format!(
                                        "{:02}:{:02} UTC",
                                        seconds / 3600 % 24,
                                        seconds / 60 % 60
                                    ))
                                    .size(12.0)
                                    .color(ui::muted(light)),
                                );
                            });
                            if let Some(id) = message.image {
                                if !self.image_textures.contains_key(&id) {
                                    if self.image_textures.len() >= 32 {
                                        self.image_textures.clear();
                                    }
                                    if let Ok(Some(bytes)) =
                                        self.store.load::<Vec<u8>>(&format!("image-{id}"))
                                    {
                                        if let Some(img) =
                                            texture(ui.ctx(), &format!("image-{id}"), &bytes)
                                        {
                                            self.image_textures.insert(id, img);
                                        }
                                    }
                                }
                                if let Some(img) = self.image_textures.get(&id) {
                                    ui.add(
                                        egui::Image::new(img)
                                            .max_size(egui::vec2(
                                                ui.available_width().min(400.0),
                                                280.0,
                                            ))
                                            .corner_radius(8),
                                    );
                                }
                            } else {
                                ui.add(egui::Label::new(&message.text).wrap());
                            }
                        });
                    });
                    ui.add_space(8.0);
                }
            });
        ui.add_space(4.0);
        let composer_id = ui.id().with("composer");
        let shortcut = ui.memory(|m| m.has_focus(composer_id))
            && ui.input(|i| !i.modifiers.shift && !i.modifiers.alt)
            && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter));
        egui::Frame::new()
            .fill(ui::surface(light))
            .corner_radius(12)
            .inner_margin(12)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    attach = ui
                        .add_enabled(session.ready(), egui::Button::new("＋"))
                        .on_hover_text("Прикрепить изображение")
                        .clicked();
                    let width = (ui.available_width() - 150.0).max(80.0);
                    let response = ui.add_sized(
                        [width, 50.0],
                        egui::TextEdit::multiline(&mut self.draft)
                            .id(composer_id)
                            .frame(false)
                            .hint_text(ui::hint(format!("Написать в #{}", self.channel), light))
                            .char_limit(16000),
                    );
                    send = ui
                        .add_enabled(
                            session.ready() && !self.draft.trim().is_empty(),
                            ui::primary("Отправить"),
                        )
                        .clicked()
                        || ((response.has_focus() || response.lost_focus()) && shortcut);
                });
            });
        ui.label(
            egui::RichText::new("Enter — отправить · Shift+Enter — новая строка")
                .size(11.0)
                .color(ui::muted(light)),
        );
        if attach {
            if let Some(path) = rfd::FileDialog::new()
                .add_filter("Изображения", &["png", "jpg", "jpeg", "webp", "gif", "bmp"])
                .pick_file()
            {
                if let Err(e) = session.image(&self.channel, path) {
                    self.error = e.to_string();
                }
            }
        }
        if send && session.ready() && !self.draft.trim().is_empty() {
            match session.chat(&self.channel, &self.draft) {
                Ok(()) => {
                    self.draft.clear();
                    self.sound(thodtalk::sounds::Sound::Sent);
                }
                Err(e) => self.error = e.to_string(),
            }
        }
    }
}
impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _: &mut eframe::Frame) {
        self.tick(ctx);
        let mut disconnect = false;
        let light = self.profile.light_theme;
        egui::SidePanel::left("sidebar")
            .exact_width(224.0)
            .resizable(false)
            .frame(egui::Frame::new().fill(ui::sidebar(light)).inner_margin(16))
            .show(ctx, |ui| {
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.image((self.brand.id(), egui::vec2(36.0, 36.0)));
                    ui.vertical(|ui| {
                        ui.label(ui::heading("ThodTalk", 21.0));
                        ui.label(
                            egui::RichText::new("Чаты и голос")
                                .size(12.0)
                                .color(ui::muted(light)),
                        );
                    });
                });
                ui.add_space(28.0);
                ui::section(ui, "ПРОСТРАНСТВО", light);
                if ui
                    .add_sized(
                        [ui.available_width(), 30.0],
                        egui::Button::new("Ваши разговоры").selected(!self.editing),
                    )
                    .clicked()
                    && self.saved
                {
                    self.editing = false;
                }
                if self.session.is_some() {
                    ui.add_space(20.0);
                    ui::section(ui, "ТЕКСТОВЫЕ КАНАЛЫ", light);
                    for channel in ["Общий", "Игры", "Музыка"] {
                        if ui
                            .add_sized(
                                [ui.available_width(), 30.0],
                                egui::Button::new(format!("#   {channel}"))
                                    .selected(self.channel == channel && !self.editing),
                            )
                            .clicked()
                        {
                            self.channel = channel.into();
                            self.editing = false;
                        }
                    }
                    ui.add_space(20.0);
                    ui::section(ui, "ГОЛОС", light);
                    self.voice_controls(ui);
                    egui::ScrollArea::vertical()
                        .id_salt("voice-members")
                        .max_height((ui.available_height() - 175.0).max(40.0))
                        .show(ui, |ui| {
                            for channel in ["Общий", "Игры", "Музыка"] {
                                self.member_rows(ui, Some(channel));
                            }
                        });
                    ui.add_space(8.0);
                    ui::section(ui, "СЕССИЯ", light);
                    ui.label(
                        egui::RichText::new(if self.session.as_ref().unwrap().hosting() {
                            "Вы — владелец сервера"
                        } else {
                            "У друзей"
                        })
                        .size(13.0)
                        .color(ui::muted(light)),
                    );
                }
                ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| {
                    ui.label(
                        egui::RichText::new(format!("ThodTalk {}", env!("CARGO_PKG_VERSION")))
                            .size(11.0)
                            .color(ui::muted(light)),
                    );
                    let account = ui.horizontal(|ui| {
                        ui::avatar(ui, self.avatar.as_ref(), &self.profile.name, 36.0);
                        ui.vertical(|ui| {
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
                            ui.label(
                                egui::RichText::new("Профиль и настройки")
                                    .size(11.0)
                                    .color(ui::muted(light)),
                            );
                        });
                    });
                    if ui
                        .interact(
                            account.response.rect,
                            ui.id().with("account-settings"),
                            egui::Sense::click(),
                        )
                        .on_hover_cursor(egui::CursorIcon::PointingHand)
                        .clicked()
                    {
                        self.editing = true;
                    }
                    ui.add_space(8.0);
                    if self.session.is_some()
                        && ui
                            .add_sized(
                                [ui.available_width(), 30.0],
                                egui::Button::new(egui::RichText::new("Отключиться").color(
                                    if light {
                                        egui::Color32::from_rgb(160, 70, 80)
                                    } else {
                                        egui::Color32::from_rgb(231, 160, 168)
                                    },
                                ))
                                .fill(if light {
                                    egui::Color32::from_rgb(252, 239, 241)
                                } else {
                                    egui::Color32::from_rgb(49, 35, 40)
                                }),
                            )
                            .clicked()
                    {
                        disconnect = true;
                    }
                });
            });
        if disconnect {
            self.stop_voice();
            self.session.take();
            self.member_textures.clear();
        }
        if self.session.is_some() && !self.editing && self.show_members {
            egui::SidePanel::right("members")
                .exact_width(190.0)
                .resizable(false)
                .frame(egui::Frame::new().fill(ui::sidebar(light)).inner_margin(12))
                .show(ctx, |ui| {
                    ui.label(ui::heading("Участники", 16.0));
                    egui::ScrollArea::vertical().show(ui, |ui| self.member_rows(ui, None));
                });
        }
        self.member_card(ctx);
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(ctx.style().visuals.panel_fill)
                    .inner_margin(20),
            )
            .show(ctx, |ui| {
                if !self.error.is_empty() {
                    egui::Frame::new()
                        .fill(ui::surface(light))
                        .corner_radius(8)
                        .inner_margin(12)
                        .show(ui, |ui| {
                            ui.colored_label(
                                if light {
                                    egui::Color32::from_rgb(165, 52, 62)
                                } else {
                                    egui::Color32::from_rgb(238, 154, 162)
                                },
                                &self.error,
                            );
                        });
                    ui.add_space(8.0);
                }
                if self.editing {
                    egui::ScrollArea::vertical().show(ui, |ui| self.profile_ui(ui));
                } else if self.session.is_some() {
                    self.chat_ui(ui);
                } else {
                    egui::ScrollArea::vertical().show(ui, |ui| self.home_ui(ui));
                }
            });
    }
}
fn key_string(key: &[u8]) -> String {
    key.iter().map(|b| format!("{b:02x}")).collect()
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
            }) {
                self.error = e.to_string();
            }
        }
    }
    fn voice_controls(&mut self, ui: &mut egui::Ui) {
        if let Some(channel) = &self.voice_channel {
            ui.label(
                egui::RichText::new(format!("Голос · {channel}"))
                    .size(12.0)
                    .color(ui::accent(self.profile.light_theme)),
            );
            ui.horizontal(|ui| {
                if ui
                    .selectable_label(self.muted, "Микр.")
                    .on_hover_text(format!("Микрофон · {}", self.profile.mute_key))
                    .clicked()
                {
                    self.muted = !self.muted;
                    self.voice_state();
                }
                if ui
                    .selectable_label(self.deafened, "Звук")
                    .on_hover_text(format!(
                        "Заглушить всё и микрофон · {}",
                        self.profile.deafen_key
                    ))
                    .clicked()
                {
                    self.deafened = !self.deafened;
                    self.voice_state();
                }
                if ui
                    .button("Выйти")
                    .on_hover_text("Выйти из голосового чата")
                    .clicked()
                {
                    self.stop_voice();
                }
            });
        } else if ui
            .add_enabled(
                self.session.as_ref().is_some_and(|s| s.ready()),
                egui::Button::new(format!("Войти · {}", self.channel)),
            )
            .clicked()
        {
            self.start_voice();
        }
    }
    fn member_rows(&mut self, ui: &mut egui::Ui, channel: Option<&str>) {
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
        if members.is_empty() {
            return;
        }
        if let Some(channel) = channel {
            ui.weak(channel);
        }
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
            let row = ui.horizontal(|ui| {
                ui::avatar(
                    ui,
                    self.member_textures.get(&key).map(|(_, t)| t),
                    &member.name,
                    24.0,
                );
                ui.add(
                    egui::Label::new(egui::RichText::new(&member.name).size(13.0).color(
                        if speaking {
                            egui::Color32::from_rgb(103, 210, 155)
                        } else {
                            ui.visuals().text_color()
                        },
                    ))
                    .truncate(),
                );
                if member.muted {
                    ui.weak("×");
                }
                if speaking {
                    let (rect, _) =
                        ui.allocate_exact_size(egui::vec2(18.0, 16.0), egui::Sense::hover());
                    let t = ui.input(|i| i.time) as f32;
                    for bar in 0..3 {
                        let h = 3.0 + ((t * 9.0 + bar as f32 * 1.7).sin().abs() * 10.0);
                        let x = rect.left() + 3.0 + bar as f32 * 5.0;
                        ui.painter().line_segment(
                            [
                                egui::pos2(x, rect.center().y - h / 2.0),
                                egui::pos2(x, rect.center().y + h / 2.0),
                            ],
                            egui::Stroke::new(2.0_f32, egui::Color32::from_rgb(103, 210, 155)),
                        );
                    }
                }
            });
            let response = ui.interact(
                row.response.rect,
                ui.id().with((&key, channel)),
                egui::Sense::click(),
            );
            if response.clicked() {
                self.user_card = Some(key.clone());
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
        let own = self.session.as_ref().unwrap().own_key() == key;
        let mut open = true;
        egui::Window::new("Участник")
            .id(egui::Id::new("user-card"))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(240.0)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui::avatar(
                        ui,
                        self.member_textures.get(&key).map(|(_, t)| t),
                        &member.name,
                        64.0,
                    );
                    ui.label(ui::heading(&member.name, 18.0));
                });
                if !own {
                    ui.add_space(8.0);
                    ui.label("Громкость для вас");
                    let volume = self.profile.volumes.entry(key_string(&key)).or_insert(1.0);
                    let mut percent = *volume * 100.0;
                    if ui
                        .add(egui::Slider::new(&mut percent, 0.0..=200.0).suffix(" %"))
                        .changed()
                    {
                        *volume = percent / 100.0;
                        let _ = self.store.save("profile", &self.profile);
                    }
                }
            });
        if !open || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.user_card = None;
        }
    }
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
fn main() {
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
