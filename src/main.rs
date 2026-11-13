#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
use eframe::egui;
use thodtalk::storage::{Profile, Store, avatar_from_file};

struct App {
    store: Store,
    profile: Profile,
    editing: bool,
    saved: bool,
    error: String,
    storage_error: bool,
    avatar: Option<egui::TextureHandle>,
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>, store: Store) -> Self {
        cc.egui_ctx.set_visuals(egui::Visuals::dark());
        let mut style = (*cc.egui_ctx.style()).clone();
        style.spacing.item_spacing = egui::vec2(10.0, 12.0);
        style.spacing.button_padding = egui::vec2(16.0, 10.0);
        style.visuals.panel_fill = egui::Color32::from_rgb(28,29,32);
        style.visuals.selection.bg_fill = egui::Color32::from_rgb(54,118,232);
        cc.egui_ctx.set_style(style);
        let (profile, saved, error) = match store.load::<Profile>("profile") {
            Ok(Some(p)) => (p, true, String::new()),
            Ok(None) => (Profile::default(), false, String::new()),
            Err(e) => (Profile::default(), false, format!("Ошибка чтения профиля: {e}. Исходный файл сохранён.")),
        };
        let storage_error = !error.is_empty();
        let mut app = Self {store, profile, editing: !saved, saved, error, storage_error, avatar: None};
        app.update_avatar(&cc.egui_ctx);
        app
    }
    fn update_avatar(&mut self, ctx: &egui::Context) {
        self.avatar = image::load_from_memory(&self.profile.avatar).ok().map(|img| {
            let img = img.to_rgba8();
            ctx.load_texture("avatar", egui::ColorImage::from_rgba_unmultiplied([img.width() as usize, img.height() as usize], img.as_raw()), egui::TextureOptions::LINEAR)
        });
    }
    fn profile_ui(&mut self, ui: &mut egui::Ui) {
        ui.heading(if self.saved { "Мой профиль" } else { "Как вас представить друзьям?" });
        ui.label("Профиль хранится на этом компьютере. Почта и регистрация не нужны.");
        ui.add_space(16.0);
        if let Some(avatar) = &self.avatar { ui.image((avatar.id(), egui::vec2(96.0,96.0))); }
        if ui.button("Выбрать фотографию").clicked() {
            if let Some(path) = rfd::FileDialog::new().add_filter("Изображения", &["png","jpg","jpeg","webp","gif","bmp"]).pick_file() {
                match avatar_from_file(&path) {
                    Ok(bytes) => { self.profile.avatar = bytes; self.update_avatar(ui.ctx()); },
                    Err(e) => self.error = e.to_string(),
                }
            }
        }
        ui.label("Отображаемое имя");
        ui.add(egui::TextEdit::singleline(&mut self.profile.name).hint_text("Ваше имя").char_limit(40).desired_width(320.0));
        ui.add_space(12.0);
        ui.checkbox(&mut self.profile.network_allowed, "Разрешить подключение к друзьям по сети");
        ui.checkbox(&mut self.profile.microphone_allowed, "Разрешить использовать микрофон для голосового чата");
        ui.label(egui::RichText::new("Микрофон включается только при входе в голосовой канал. Фото выбирается через системный диалог.").small().color(egui::Color32::GRAY));
        if ui.add_enabled(!self.storage_error, egui::Button::new("Сохранить")).clicked() {
            match self.profile.validate().and_then(|_| self.store.save("profile", &self.profile)) {
                Ok(()) => {self.editing = false; self.saved = true; self.error.clear();},
                Err(e) => self.error = e.to_string(),
            }
        }
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _: &mut eframe::Frame) {
        egui::SidePanel::left("sidebar").exact_width(220.0).show(ctx, |ui| {
            ui.add_space(20.0); ui.heading("ThodTalk"); ui.label("В кругу своих"); ui.add_space(28.0);
            if ui.selectable_label(!self.editing, "Мои серверы").clicked() && self.saved {self.editing = false;}
            if ui.selectable_label(self.editing, "Мой профиль").clicked() {self.editing = true;}
            ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| {
                ui.label("0.1.0 · В разработке");
                ui.label(&self.profile.name);
            });
        });
        egui::CentralPanel::default().show(ctx, |ui| {
            egui::Frame::new().inner_margin(28.0).show(ui, |ui| {
                if self.editing {self.profile_ui(ui);} else {
                    ui.heading("Мои серверы"); ui.label("Профиль готов. Следующий этап — защищённые подключения и чаты.");
                }
                if !self.error.is_empty() { ui.add_space(16.0); ui.colored_label(egui::Color32::LIGHT_RED, &self.error); }
            });
        });
    }
}

fn main() -> anyhow::Result<()> {
    let store = Store::open(Store::default_path()?)?;
    let options = eframe::NativeOptions { viewport: egui::ViewportBuilder::default().with_inner_size([1000.0,700.0]).with_min_inner_size([720.0,560.0]), ..Default::default() };
    eframe::run_native("ThodTalk", options, Box::new(move |cc| Ok(Box::new(App::new(cc, store))))).map_err(|e| anyhow::anyhow!("{e}"))
}
