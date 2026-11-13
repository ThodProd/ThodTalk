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
    choosing_ptt_key: bool,
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
            choosing_ptt_key: false,
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
    fn stop_voice(&mut self) {
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
            if let Err(e) = session.poll() {
                self.error = format!("Соединение прервано: {e}. История сохранена локально.");
                disconnected = true;
            }
            if let Some(audio) = &mut self.audio {
                let key = egui::Key::from_name(&self.profile.ptt_key).unwrap_or(egui::Key::F8);
                let held = !self.choosing_ptt_key && ctx.input(|i| i.key_down(key));
                let enabled = !self.muted && !self.deafened && (!self.ptt || held);
                audio.transmitting.store(enabled, Ordering::Relaxed);
                audio.deafened.store(self.deafened, Ordering::Relaxed);
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
                        if let Err(e) = audio.play(author, &data) {
                            self.error = format!("Голосовой пакет: {e}");
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
        ui.set_max_width(600.0);
        ui.heading(if self.saved {
            "Мой профиль"
        } else {
            "Ваш профиль"
        });
        ui.label(
            egui::RichText::new("Имя и фотография, которые увидят ваши друзья.")
                .color(ui::muted(self.profile.light_theme)),
        );
        ui.add_space(20.0);
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
        ui.add_space(12.0);
        ui.label("Отображаемое имя");
        ui.add_sized(
            [ui.available_width(), 44.0],
            egui::TextEdit::singleline(&mut self.profile.name)
                .hint_text(ui::hint("Как к вам обращаться", !ui.visuals().dark_mode))
                .char_limit(40)
                .margin(egui::vec2(12.0, 12.0)),
        );
        ui.add_space(20.0);
        ui.separator();
        ui.add_space(12.0);
        ui.label(ui::heading("Подключение и звук", 18.0));
        ui.small("Микрофон включается только после входа в голос.");
        egui::ComboBox::from_label("Микрофон")
            .selected_text(if self.input_device.is_empty() {
                "По умолчанию"
            } else {
                &self.input_device
            })
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut self.input_device, String::new(), "По умолчанию");
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
                ui.selectable_value(&mut self.output_device, String::new(), "По умолчанию");
                for name in &self.outputs {
                    ui.selectable_value(&mut self.output_device, name.clone(), name);
                }
            });
        ui.checkbox(&mut self.ptt, "Говорить при удержании клавиши");
        if self.ptt {
            ui.horizontal(|ui| {
                ui.label("Клавиша:");
                if ui
                    .button(if self.choosing_ptt_key {
                        "Нажмите клавишу…"
                    } else {
                        &self.profile.ptt_key
                    })
                    .clicked()
                {
                    self.choosing_ptt_key = true;
                }
                ui.small("В активном окне · Esc — отмена");
            });
            if self.choosing_ptt_key {
                let key = ui.input(|i| {
                    i.events.iter().find_map(|event| match event {
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
                if let Some(key) = key {
                    if key != egui::Key::Escape {
                        self.profile.ptt_key = key.name().into();
                    }
                    self.choosing_ptt_key = false;
                }
            }
        }
        if ui
            .checkbox(&mut self.profile.light_theme, "Светлая тема")
            .changed()
        {
            ui::theme(ui.ctx(), self.profile.light_theme);
        }
        ui.add_space(16.0);
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
                egui::Button::new("Запустить сервер").min_size(egui::vec2(0.0, 44.0)),
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
                ui.label("Подключаемся… В сложной сети это может занять до 3 минут.");
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
                            }
                        });
                    }
                });
        }
    }
    fn chat_ui(&mut self, ui: &mut egui::Ui) {
        let light = self.profile.light_theme;
        let mut enter_voice = false;
        let mut leave_voice = false;
        let mut send = false;
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
        let height = (ui.available_height() - 164.0).max(80.0);
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
                            ui.add(egui::Label::new(&message.text).wrap());
                        });
                    });
                    ui.add_space(8.0);
                }
            });
        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            if let Some(channel) = &self.voice_channel {
                ui::badge(ui, &format!("Голос · {channel}"), light);
                let changed = ui.checkbox(&mut self.muted, "Без микрофона").changed()
                    | ui.checkbox(&mut self.deafened, "Без звука").changed();
                if changed {
                    let _ = session.send(Payload::VoiceState {
                        channel: self.voice_channel.clone(),
                        muted: self.muted || self.deafened,
                    });
                }
                leave_voice = ui.button("Выйти").clicked();
            } else {
                enter_voice = ui
                    .add_enabled(session.ready(), egui::Button::new("Войти в голос"))
                    .clicked();
                ui.label(
                    egui::RichText::new("Микрофон выключен")
                        .size(12.0)
                        .color(ui::muted(light)),
                );
            }
        });
        let shortcut = ui.input(|i| i.modifiers.ctrl && i.key_pressed(egui::Key::Enter));
        egui::Frame::new()
            .fill(ui::surface(light))
            .corner_radius(12)
            .inner_margin(12)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let width = (ui.available_width() - 120.0).max(80.0);
                    let response = ui.add_sized(
                        [width, 50.0],
                        egui::TextEdit::multiline(&mut self.draft)
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
                        || (response.has_focus() && shortcut);
                });
            });
        ui.label(
            egui::RichText::new("Ctrl+Enter — отправить · Enter — новая строка")
                .size(11.0)
                .color(ui::muted(light)),
        );
        if send {
            match session.chat(&self.channel, &self.draft) {
                Ok(()) => self.draft.clear(),
                Err(e) => self.error = e.to_string(),
            }
        }
        if leave_voice {
            self.stop_voice();
        }
        if enter_voice {
            self.start_voice();
        }
    }
}
impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _: &mut eframe::Frame) {
        self.tick(ctx);
        let mut disconnect = false;
        let mut kick = None;
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
                        [ui.available_width(), 40.0],
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
                                [ui.available_width(), 40.0],
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
                                [ui.available_width(), 38.0],
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
        if let Some(session) = &self.session {
            if !self.editing && self.show_members {
                egui::SidePanel::right("members")
                    .exact_width(208.0)
                    .resizable(false)
                    .frame(egui::Frame::new().fill(ui::sidebar(light)).inner_margin(16))
                    .show(ctx, |ui| {
                        ui.add_space(16.0);
                        ui.label(ui::heading(
                            format!("Участники · {}", session.members.len()),
                            16.0,
                        ));
                        ui.add_space(16.0);
                        egui::ScrollArea::vertical().show(ui, |ui| {
                            for (key, member) in &session.members {
                                if !member.avatar.is_empty()
                                    && self
                                        .member_textures
                                        .get(key)
                                        .is_none_or(|(bytes, _)| bytes != &member.avatar)
                                {
                                    if let Some(image) =
                                        texture(ctx, &format!("member-{key:?}"), &member.avatar)
                                    {
                                        self.member_textures
                                            .insert(key.clone(), (member.avatar.clone(), image));
                                    }
                                }
                                ui.horizontal_top(|ui| {
                                    ui::avatar(
                                        ui,
                                        self.member_textures.get(key).map(|(_, image)| image),
                                        &member.name,
                                        32.0,
                                    );
                                    ui.vertical(|ui| {
                                        ui.add(
                                            egui::Label::new(ui::heading(&member.name, 14.0))
                                                .truncate(),
                                        )
                                        .on_hover_text(
                                            format!(
                                                "Ключ профиля: {}",
                                                key.iter()
                                                    .map(|b| format!("{b:02x}"))
                                                    .collect::<String>()
                                            ),
                                        );
                                        ui.label(
                                            egui::RichText::new(
                                                if let Some(channel) = &member.voice {
                                                    format!(
                                                        "{} · {}",
                                                        channel,
                                                        if member.muted {
                                                            "без микрофона"
                                                        } else {
                                                            "в голосе"
                                                        }
                                                    )
                                                } else {
                                                    "В сети".into()
                                                },
                                            )
                                            .size(12.0)
                                            .color(ui::muted(light)),
                                        );
                                    });
                                });
                                if session.hosting() && key != &session.own_key() {
                                    ui.menu_button("Управление", |ui| {
                                        if ui.button("Кикнуть из сессии").clicked() {
                                            kick = Some(key.clone());
                                            ui.close_menu();
                                        }
                                    });
                                }
                                ui.add_space(12.0);
                            }
                        });
                    });
            }
        }
        if let Some(key) = kick {
            if let Some(session) = &mut self.session {
                if let Err(e) = session.kick(&key) {
                    self.error = e.to_string();
                }
            }
        }
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(ctx.style().visuals.panel_fill)
                    .inner_margin(28),
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
fn texture(ctx: &egui::Context, name: &str, bytes: &[u8]) -> Option<egui::TextureHandle> {
    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .ok()?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(512);
    limits.max_image_height = Some(512);
    limits.max_alloc = Some(4 * 1024 * 1024);
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
