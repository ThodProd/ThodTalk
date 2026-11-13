use eframe::egui::{self, Color32, FontFamily, FontId, RichText};

pub const BLUE: Color32 = Color32::from_rgb(54, 118, 232);
pub fn surface(light: bool) -> Color32 {
    if light {
        Color32::from_rgb(247, 248, 250)
    } else {
        Color32::from_rgb(37, 38, 42)
    }
}
pub fn sidebar(light: bool) -> Color32 {
    if light {
        Color32::from_rgb(245, 245, 247)
    } else {
        Color32::from_rgb(23, 24, 27)
    }
}
pub fn muted(light: bool) -> Color32 {
    if light {
        Color32::from_rgb(118, 123, 133)
    } else {
        Color32::from_rgb(152, 156, 165)
    }
}
pub fn accent(light: bool) -> Color32 {
    if light {
        BLUE
    } else {
        Color32::from_rgb(129, 170, 255)
    }
}
pub fn heading(text: impl Into<String>, size: f32) -> RichText {
    RichText::new(text).font(FontId::new(size, FontFamily::Name("Semibold".into())))
}
pub fn fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    let body = std::fs::read("C:/Windows/Fonts/SegUIVar.ttf")
        .or_else(|_| std::fs::read("C:/Windows/Fonts/segoeui.ttf"));
    if let Ok(bytes) = body {
        fonts
            .font_data
            .insert("Segoe UI".into(), egui::FontData::from_owned(bytes).into());
        fonts
            .families
            .entry(FontFamily::Proportional)
            .or_default()
            .insert(0, "Segoe UI".into());
    }
    let mut headings = fonts.families[&FontFamily::Proportional].clone();
    if let Ok(bytes) = std::fs::read("C:/Windows/Fonts/seguisb.ttf") {
        fonts.font_data.insert(
            "Segoe Semibold".into(),
            egui::FontData::from_owned(bytes).into(),
        );
        headings.insert(0, "Segoe Semibold".into());
    }
    fonts
        .families
        .insert(FontFamily::Name("Semibold".into()), headings);
    ctx.set_fonts(fonts);
}
pub fn theme(ctx: &egui::Context, light: bool) {
    ctx.send_viewport_cmd(egui::ViewportCommand::SetTheme(if light {
        egui::SystemTheme::Light
    } else {
        egui::SystemTheme::Dark
    }));
    ctx.set_theme(if light {
        egui::ThemePreference::Light
    } else {
        egui::ThemePreference::Dark
    });
    let mut style = egui::Style::default();
    style.visuals = if light {
        egui::Visuals::light()
    } else {
        egui::Visuals::dark()
    };
    let text = if light {
        Color32::from_rgb(36, 38, 43)
    } else {
        Color32::from_rgb(236, 237, 240)
    };
    let line = if light {
        Color32::from_rgb(232, 233, 237)
    } else {
        Color32::from_rgb(48, 49, 54)
    };
    style.visuals.panel_fill = if light {
        Color32::WHITE
    } else {
        Color32::from_rgb(28, 29, 32)
    };
    style.visuals.window_fill = surface(light);
    style.visuals.extreme_bg_color = surface(light);
    style.visuals.override_text_color = Some(text);
    style.visuals.widgets.noninteractive.fg_stroke = egui::Stroke::new(1.0_f32, muted(light));
    style.visuals.widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0_f32, line);
    for widget in [
        &mut style.visuals.widgets.inactive,
        &mut style.visuals.widgets.hovered,
        &mut style.visuals.widgets.active,
        &mut style.visuals.widgets.open,
    ] {
        widget.corner_radius = egui::CornerRadius::same(16);
        widget.bg_fill = surface(light);
        widget.weak_bg_fill = surface(light);
        widget.fg_stroke = egui::Stroke::new(1.5_f32, text);
        widget.bg_stroke = egui::Stroke::NONE;
    }
    style.visuals.widgets.hovered.bg_stroke = egui::Stroke::NONE;
    style.visuals.widgets.hovered.weak_bg_fill = if light {
        Color32::from_rgb(237, 239, 244)
    } else {
        Color32::from_rgb(43, 45, 51)
    };
    style.visuals.widgets.active.bg_fill = if light {
        Color32::from_rgb(223, 234, 255)
    } else {
        Color32::from_rgb(42, 58, 83)
    };
    style.visuals.selection.bg_fill = if light {
        Color32::from_rgb(237, 241, 249)
    } else {
        Color32::from_rgb(34, 40, 51)
    };
    style.visuals.selection.stroke = egui::Stroke::new(0.0_f32, accent(light));
    style.visuals.window_corner_radius = egui::CornerRadius::same(12);
    style.spacing.item_spacing = egui::vec2(10.0, 12.0);
    style.spacing.button_padding = egui::vec2(16.0, 10.0);
    style.spacing.interact_size = egui::vec2(40.0, 40.0);
    style.spacing.icon_width = 18.0;
    style.spacing.icon_spacing = 10.0;
    style
        .text_styles
        .insert(egui::TextStyle::Body, FontId::proportional(15.0));
    style
        .text_styles
        .insert(egui::TextStyle::Button, FontId::proportional(15.0));
    style
        .text_styles
        .insert(egui::TextStyle::Small, FontId::proportional(12.0));
    style.text_styles.insert(
        egui::TextStyle::Heading,
        FontId::new(28.0, FontFamily::Name("Semibold".into())),
    );
    style
        .text_styles
        .insert(egui::TextStyle::Monospace, FontId::monospace(13.0));
    style.animation_time = 0.14;
    ctx.set_style(style);
}
pub fn primary(text: &str) -> egui::Button<'_> {
    egui::Button::new(RichText::new(text).color(Color32::WHITE))
        .fill(BLUE)
        .stroke(egui::Stroke::NONE)
        .min_size(egui::vec2(0.0, 44.0))
}
pub fn hint(text: impl Into<String>, light: bool) -> RichText {
    RichText::new(text)
        .size(13.0)
        .color(muted(light).gamma_multiply(0.65))
}
pub fn section(ui: &mut egui::Ui, text: &str, light: bool) {
    ui.label(RichText::new(text).size(12.0).color(muted(light)));
}
pub fn avatar(ui: &mut egui::Ui, image: Option<&egui::TextureHandle>, name: &str, size: f32) {
    if let Some(image) = image {
        ui.add(
            egui::Image::new((image.id(), egui::vec2(size, size)))
                .corner_radius((size / 2.0) as u8),
        );
    } else {
        let (rect, _) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::hover());
        ui.painter()
            .circle_filled(rect.center(), size / 2.0, Color32::from_rgb(43, 61, 90));
        ui.painter().text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            name.chars()
                .next()
                .unwrap_or('T')
                .to_uppercase()
                .to_string(),
            FontId::proportional(size * 0.4),
            Color32::from_rgb(173, 201, 255),
        );
    }
}
pub fn badge(ui: &mut egui::Ui, text: &str, light: bool) {
    egui::Frame::new()
        .fill(if light {
            Color32::from_rgb(235, 242, 255)
        } else {
            Color32::from_rgb(34, 46, 65)
        })
        .corner_radius(6)
        .inner_margin(egui::Margin::symmetric(9, 4))
        .show(ui, |ui| {
            ui.label(RichText::new(text).size(12.0).color(accent(light)));
        });
}
