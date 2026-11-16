use eframe::egui::{
    self, Align, Align2, Color32, CornerRadius, FontFamily, FontId, Layout, Pos2, Rect, Response,
    RichText, Sense, Shape, Stroke, StrokeKind, Ui, UiBuilder, Vec2, pos2, vec2,
};
use std::f32::consts::PI;

// ───────────────────────── Палитра (в духе Discord) ─────────────────────────

pub const BLUE: Color32 = Color32::from_rgb(54, 118, 232);
pub const GREEN: Color32 = Color32::from_rgb(35, 165, 90);
pub const GRAY: Color32 = Color32::from_rgb(128, 132, 142);

fn pick(light: bool, l: (u8, u8, u8), d: (u8, u8, u8)) -> Color32 {
    let (r, g, b) = if light { l } else { d };
    Color32::from_rgb(r, g, b)
}
/// Основной фон (чат, контент).
pub fn bg(light: bool) -> Color32 {
    pick(light, (255, 255, 255), (49, 51, 56))
}
/// Боковые панели.
pub fn sidebar(light: bool) -> Color32 {
    pick(light, (242, 243, 245), (43, 45, 49))
}
/// Самый тёмный слой: панель пользователя, поля ввода.
pub fn deep(light: bool) -> Color32 {
    pick(light, (233, 235, 238), (30, 31, 34))
}
/// Карточки в настройках.
pub fn surface(light: bool) -> Color32 {
    sidebar(light)
}
/// Поле сообщения.
pub fn input(light: bool) -> Color32 {
    pick(light, (235, 237, 239), (56, 58, 64))
}
pub fn hover(light: bool) -> Color32 {
    pick(light, (227, 229, 232), (53, 55, 60))
}
pub fn selected(light: bool) -> Color32 {
    pick(light, (216, 219, 224), (66, 69, 76))
}
pub fn text(light: bool) -> Color32 {
    pick(light, (46, 51, 56), (219, 222, 225))
}
pub fn muted(light: bool) -> Color32 {
    pick(light, (94, 103, 114), (148, 155, 164))
}
pub fn line(light: bool) -> Color32 {
    pick(light, (225, 227, 231), (63, 65, 71))
}
pub fn accent(light: bool) -> Color32 {
    pick(light, (54, 118, 232), (129, 170, 255))
}
pub fn danger(light: bool) -> Color32 {
    pick(light, (216, 55, 62), (242, 95, 100))
}
pub fn danger_bg(light: bool) -> Color32 {
    pick(light, (253, 235, 236), (66, 38, 42))
}
pub fn success(light: bool) -> Color32 {
    pick(light, (30, 150, 80), (59, 196, 120))
}
/// Мягкая подсветка задержки: обычная, повышенная, высокая.
pub fn ping(light: bool, ms: u32) -> Color32 {
    if ms <= 80 {
        muted(light)
    } else if ms <= 160 {
        pick(light, (168, 122, 20), (214, 178, 92))
    } else {
        danger(light)
    }
}
fn button_bg(light: bool) -> Color32 {
    pick(light, (225, 227, 231), (78, 80, 88))
}
fn button_hover(light: bool) -> Color32 {
    pick(light, (212, 215, 220), (92, 94, 102))
}

const PALETTE: [(u8, u8, u8); 7] = [
    (88, 101, 242),
    (35, 165, 90),
    (230, 160, 30),
    (225, 70, 150),
    (232, 76, 80),
    (0, 158, 186),
    (155, 89, 210),
];
fn palette_index(name: &str) -> usize {
    name.bytes()
        .fold(7u32, |h, b| h.wrapping_mul(31).wrapping_add(b as u32)) as usize
        % PALETTE.len()
}
/// Цвет имени автора: у каждого человека свой, как в Discord.
pub fn name_color(name: &str, light: bool) -> Color32 {
    let (r, g, b) = PALETTE[palette_index(name)];
    let c = Color32::from_rgb(r, g, b);
    if light { c.gamma_multiply(0.85) } else { c }
}

// ───────────────────────── Шрифты и тема ─────────────────────────

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
    let txt = text(light);
    style.visuals.panel_fill = bg(light);
    style.visuals.window_fill = sidebar(light);
    style.visuals.window_stroke = Stroke::new(1.0_f32, line(light));
    style.visuals.window_corner_radius = egui::CornerRadius::same(12);
    style.visuals.extreme_bg_color = deep(light);
    style.visuals.faint_bg_color = hover(light);
    style.visuals.hyperlink_color = accent(light);
    style.visuals.override_text_color = None;
    style.visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0_f32, txt);
    style.visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, line(light));
    style.visuals.widgets.noninteractive.bg_fill = sidebar(light);
    for widget in [
        &mut style.visuals.widgets.inactive,
        &mut style.visuals.widgets.hovered,
        &mut style.visuals.widgets.active,
        &mut style.visuals.widgets.open,
    ] {
        widget.corner_radius = egui::CornerRadius::same(8);
        widget.bg_fill = button_bg(light);
        widget.weak_bg_fill = button_bg(light);
        widget.fg_stroke = Stroke::new(1.0_f32, txt);
        widget.bg_stroke = Stroke::NONE;
        widget.expansion = 0.0;
    }
    style.visuals.widgets.hovered.weak_bg_fill = button_hover(light);
    style.visuals.widgets.hovered.bg_fill = button_hover(light);
    style.visuals.widgets.active.weak_bg_fill = button_hover(light);
    style.visuals.widgets.active.bg_fill = button_hover(light);
    style.visuals.widgets.open.weak_bg_fill = button_hover(light);
    style.visuals.selection.bg_fill = BLUE;
    style.visuals.selection.stroke = Stroke::new(1.0_f32, Color32::WHITE);
    style.spacing.item_spacing = vec2(8.0, 6.0);
    style.spacing.button_padding = vec2(14.0, 7.0);
    style.spacing.interact_size = vec2(28.0, 28.0);
    style.spacing.icon_width = 18.0;
    style.spacing.icon_spacing = 10.0;
    style.spacing.scroll.floating = true;
    style
        .text_styles
        .insert(egui::TextStyle::Body, FontId::proportional(14.0));
    style
        .text_styles
        .insert(egui::TextStyle::Button, FontId::proportional(14.0));
    style
        .text_styles
        .insert(egui::TextStyle::Small, FontId::proportional(12.0));
    style.text_styles.insert(
        egui::TextStyle::Heading,
        FontId::new(24.0, FontFamily::Name("Semibold".into())),
    );
    style
        .text_styles
        .insert(egui::TextStyle::Monospace, FontId::monospace(13.0));
    style.animation_time = 0.12;
    ctx.set_style(style);
}

// ───────────────────────── Иконки (рисуются кодом, без шрифтов) ─────────────────────────

#[derive(Clone, Copy, PartialEq)]
pub enum Icon {
    Mic,
    Headphones,
    Gear,
    Plus,
    Minus,
    Close,
    Send,
    Hash,
    Speaker,
    HangUp,
    Users,
    UserPlus,
    Shield,
    Lock,
    Logout,
    Home,
    Archive,
    Image,
    User,
    Keyboard,
    Download,
    Check,
    Warning,
    Monitor,
    Eye,
    File,
}

/// Рисует иконку в квадрате `rect`. Все иконки заданы на сетке 24×24.
pub fn paint_icon(p: &egui::Painter, icon: Icon, rect: Rect, color: Color32, slashed: bool) {
    let s = rect.width().min(rect.height()) / 24.0;
    let o = rect.center() - vec2(12.0, 12.0) * s;
    let pt = |x: f32, y: f32| pos2(o.x + x * s, o.y + y * s);
    let w = (1.9 * s).max(1.2);
    let st = Stroke::new(w, color);
    let seg = |a: (f32, f32), b: (f32, f32)| {
        p.line_segment([pt(a.0, a.1), pt(b.0, b.1)], st);
    };
    let poly = |pts: &[(f32, f32)], closed: bool| {
        let v: Vec<Pos2> = pts.iter().map(|q| pt(q.0, q.1)).collect();
        p.add(if closed {
            Shape::closed_line(v, st)
        } else {
            Shape::line(v, st)
        });
    };
    let arc = |cx: f32, cy: f32, r: f32, a0: f32, a1: f32| -> Vec<(f32, f32)> {
        (0..=20)
            .map(|i| {
                let t = a0 + (a1 - a0) * i as f32 / 20.0;
                (cx + r * t.cos(), cy + r * t.sin())
            })
            .collect()
    };
    let dot = |x: f32, y: f32, r: f32| {
        p.circle_filled(pt(x, y), r * s, color);
    };
    let ring = |x: f32, y: f32, r: f32| {
        p.circle_stroke(pt(x, y), r * s, st);
    };
    let fill_rect = |a: (f32, f32), b: (f32, f32), r: f32| {
        p.rect_filled(
            Rect::from_two_pos(pt(a.0, a.1), pt(b.0, b.1)),
            CornerRadius::same((r * s).round() as u8),
            color,
        );
    };
    let stroke_rect = |a: (f32, f32), b: (f32, f32), r: f32| {
        p.rect_stroke(
            Rect::from_two_pos(pt(a.0, a.1), pt(b.0, b.1)),
            CornerRadius::same((r * s).round() as u8),
            st,
            StrokeKind::Middle,
        );
    };
    let tri = |a: (f32, f32), b: (f32, f32), c: (f32, f32)| {
        p.add(Shape::convex_polygon(
            vec![pt(a.0, a.1), pt(b.0, b.1), pt(c.0, c.1)],
            color,
            Stroke::NONE,
        ));
    };
    match icon {
        Icon::Mic => {
            dot(12.0, 6.5, 3.2);
            dot(12.0, 11.0, 3.2);
            fill_rect((8.8, 6.5), (15.2, 11.0), 0.0);
            poly(&arc(12.0, 11.0, 6.8, 0.0, PI), false);
            seg((12.0, 17.8), (12.0, 21.0));
            seg((8.5, 21.0), (15.5, 21.0));
        }
        Icon::Headphones => {
            poly(&arc(12.0, 13.0, 8.0, PI, 2.0 * PI), false);
            fill_rect((3.5, 13.0), (8.0, 20.0), 1.6);
            fill_rect((16.0, 13.0), (20.5, 20.0), 1.6);
        }
        Icon::Gear => {
            ring(12.0, 12.0, 6.4);
            ring(12.0, 12.0, 2.4);
            for k in 0..8 {
                let a = k as f32 * PI / 4.0;
                p.line_segment(
                    [
                        pt(12.0 + 6.8 * a.cos(), 12.0 + 6.8 * a.sin()),
                        pt(12.0 + 10.0 * a.cos(), 12.0 + 10.0 * a.sin()),
                    ],
                    Stroke::new(3.0 * s, color),
                );
            }
        }
        Icon::Plus => {
            seg((12.0, 4.5), (12.0, 19.5));
            seg((4.5, 12.0), (19.5, 12.0));
        }
        Icon::Minus => {
            seg((4.5, 12.0), (19.5, 12.0));
        }
        Icon::Close => {
            seg((5.5, 5.5), (18.5, 18.5));
            seg((18.5, 5.5), (5.5, 18.5));
        }
        Icon::Send => {
            tri((3.0, 3.5), (21.5, 12.0), (7.5, 12.0));
            tri((3.0, 20.5), (21.5, 12.0), (7.5, 12.0));
        }
        Icon::Hash => {
            seg((9.5, 4.0), (7.5, 20.0));
            seg((16.5, 4.0), (14.5, 20.0));
            seg((4.0, 9.0), (20.0, 9.0));
            seg((4.0, 15.0), (20.0, 15.0));
        }
        Icon::Speaker => {
            fill_rect((3.0, 9.0), (8.0, 15.0), 0.0);
            p.add(Shape::convex_polygon(
                vec![pt(8.0, 9.0), pt(13.5, 4.5), pt(13.5, 19.5), pt(8.0, 15.0)],
                color,
                Stroke::NONE,
            ));
            poly(&arc(13.5, 12.0, 4.6, -0.9, 0.9), false);
            poly(&arc(13.5, 12.0, 8.6, -0.9, 0.9), false);
        }
        Icon::HangUp => {
            let thick = Stroke::new(3.6 * s, color);
            let pts: Vec<Pos2> = arc(12.0, 20.0, 10.0, 1.2 * PI, 1.8 * PI)
                .iter()
                .map(|q| pt(q.0, q.1))
                .collect();
            p.add(Shape::line(pts, thick));
            for x in [3.9_f32, 20.1] {
                p.line_segment([pt(x, 14.1), pt(x, 17.4)], thick);
            }
        }
        Icon::Users => {
            ring(9.0, 8.0, 3.3);
            poly(&arc(9.0, 20.5, 6.5, PI, 2.0 * PI), false);
            ring(16.8, 8.5, 2.6);
            poly(&arc(17.0, 19.5, 5.0, 1.25 * PI, 2.0 * PI), false);
        }
        Icon::UserPlus => {
            ring(10.0, 8.0, 3.5);
            poly(&arc(10.0, 20.5, 7.0, PI, 2.0 * PI), false);
            seg((18.0, 5.5), (18.0, 12.5));
            seg((14.5, 9.0), (21.5, 9.0));
        }
        Icon::Shield => {
            poly(
                &[
                    (12.0, 3.0),
                    (20.0, 6.0),
                    (20.0, 12.0),
                    (12.0, 21.0),
                    (4.0, 12.0),
                    (4.0, 6.0),
                ],
                true,
            );
            poly(&[(8.5, 11.5), (11.0, 14.0), (15.5, 9.0)], false);
        }
        Icon::Lock => {
            stroke_rect((5.0, 11.0), (19.0, 21.0), 2.0);
            poly(&arc(12.0, 11.0, 4.5, PI, 2.0 * PI), false);
            dot(12.0, 16.0, 1.4);
        }
        Icon::Logout => {
            poly(&[(10.0, 4.0), (4.0, 4.0), (4.0, 20.0), (10.0, 20.0)], false);
            seg((9.0, 12.0), (20.0, 12.0));
            poly(&[(16.0, 8.0), (20.0, 12.0), (16.0, 16.0)], false);
        }
        Icon::Home => {
            poly(&[(3.0, 12.0), (12.0, 4.0), (21.0, 12.0)], false);
            poly(
                &[(5.5, 10.5), (5.5, 20.0), (18.5, 20.0), (18.5, 10.5)],
                false,
            );
            poly(
                &[(10.0, 20.0), (10.0, 14.0), (14.0, 14.0), (14.0, 20.0)],
                false,
            );
        }
        Icon::Archive => {
            stroke_rect((3.0, 4.0), (21.0, 9.0), 2.0);
            poly(&[(4.5, 9.0), (4.5, 20.0), (19.5, 20.0), (19.5, 9.0)], false);
            seg((10.0, 13.0), (14.0, 13.0));
        }
        Icon::Image => {
            stroke_rect((3.0, 5.0), (21.0, 19.0), 3.0);
            dot(9.0, 10.0, 1.8);
            poly(
                &[
                    (3.5, 17.0),
                    (9.0, 12.5),
                    (13.0, 16.0),
                    (16.0, 13.5),
                    (20.5, 17.5),
                ],
                false,
            );
        }
        Icon::User => {
            ring(12.0, 8.0, 4.0);
            poly(&arc(12.0, 22.0, 8.0, PI, 2.0 * PI), false);
        }
        Icon::Keyboard => {
            stroke_rect((2.0, 6.0), (22.0, 18.0), 3.0);
            for x in [6.5_f32, 10.0, 14.0, 17.5] {
                dot(x, 10.0, 1.0);
            }
            seg((7.0, 14.5), (17.0, 14.5));
        }
        Icon::Download => {
            seg((12.0, 4.0), (12.0, 15.0));
            poly(&[(7.5, 10.5), (12.0, 15.0), (16.5, 10.5)], false);
            poly(
                &[(4.0, 16.0), (4.0, 20.0), (20.0, 20.0), (20.0, 16.0)],
                false,
            );
        }
        Icon::Check => {
            poly(&[(4.5, 12.5), (10.0, 18.0), (19.5, 6.5)], false);
        }
        Icon::Warning => {
            poly(&[(12.0, 3.5), (21.5, 20.0), (2.5, 20.0)], true);
            seg((12.0, 9.5), (12.0, 14.0));
            dot(12.0, 17.0, 1.1);
        }
        Icon::Monitor => {
            stroke_rect((3.0, 4.5), (21.0, 16.5), 2.0);
            seg((12.0, 16.5), (12.0, 20.5));
            seg((8.0, 20.5), (16.0, 20.5));
        }
        Icon::Eye => {
            poly(
                &[
                    (2.5, 12.0),
                    (7.0, 7.5),
                    (12.0, 5.5),
                    (17.0, 7.5),
                    (21.5, 12.0),
                    (17.0, 16.5),
                    (12.0, 18.5),
                    (7.0, 16.5),
                ],
                true,
            );
            ring(12.0, 12.0, 3.0);
        }
        Icon::File => {
            poly(
                &[
                    (5.0, 3.0),
                    (14.5, 3.0),
                    (19.0, 7.5),
                    (19.0, 21.0),
                    (5.0, 21.0),
                ],
                true,
            );
            poly(&[(14.5, 3.0), (14.5, 7.5), (19.0, 7.5)], false);
        }
    }
    if slashed {
        p.line_segment([pt(3.5, 3.5), pt(20.5, 20.5)], Stroke::new(w * 1.15, color));
    }
}

/// Неинтерактивная иконка (например, `#` рядом с названием канала).
pub fn icon_label(ui: &mut Ui, icon: Icon, size: f32, color: Color32) -> Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    paint_icon(ui.painter(), icon, rect, color, false);
    resp
}

#[derive(Clone, Copy, PartialEq)]
pub enum Tone {
    /// Серая, при наведении светлеет.
    Normal,
    /// Всегда цвета акцента (включённый переключатель).
    Active,
    /// Серая, при наведении краснеет (выйти, отключиться).
    Danger,
    /// Всегда красная (микрофон выключен).
    Alert,
}

pub fn icon_btn(ui: &mut Ui, light: bool, icon: Icon, tone: Tone, slashed: bool) -> Response {
    icon_btn_sized(ui, light, icon, tone, slashed, 32.0)
}
pub fn icon_btn_sized(
    ui: &mut Ui,
    light: bool,
    icon: Icon,
    tone: Tone,
    slashed: bool,
    size: f32,
) -> Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(size), Sense::click());
    let enabled = ui.is_enabled();
    let hovered = enabled && resp.hovered();
    if hovered {
        ui.painter()
            .rect_filled(rect, CornerRadius::same(6), hover(light));
    }
    let color = match tone {
        Tone::Normal => {
            if hovered {
                text(light)
            } else {
                muted(light)
            }
        }
        Tone::Active => accent(light),
        Tone::Danger => {
            if hovered {
                danger(light)
            } else {
                muted(light)
            }
        }
        Tone::Alert => danger(light),
    };
    paint_icon(
        ui.painter(),
        icon,
        Rect::from_center_size(rect.center(), Vec2::splat(size * 0.62)),
        color,
        slashed,
    );
    if enabled {
        resp.on_hover_cursor(egui::CursorIcon::PointingHand)
    } else {
        resp
    }
}

/// Кнопка панели сообщения высотой 40 px: иконка всегда по центру поля.
pub fn composer_btn(ui: &mut Ui, light: bool, icon: Icon, tone: Tone, enabled: bool) -> Response {
    let enabled = enabled && ui.is_enabled();
    let (rect, resp) = ui.allocate_exact_size(
        vec2(32.0, 40.0),
        if enabled {
            Sense::click()
        } else {
            Sense::hover()
        },
    );
    let hovered = enabled && resp.hovered();
    if hovered {
        ui.painter().rect_filled(
            Rect::from_center_size(rect.center(), Vec2::splat(28.0)),
            CornerRadius::same(6),
            hover(light),
        );
    }
    let color = match tone {
        Tone::Normal => {
            if hovered {
                text(light)
            } else {
                muted(light)
            }
        }
        Tone::Active => accent(light),
        Tone::Danger => {
            if hovered {
                danger(light)
            } else {
                muted(light)
            }
        }
        Tone::Alert => danger(light),
    };
    paint_icon(
        ui.painter(),
        icon,
        Rect::from_center_size(rect.center(), Vec2::splat(20.0)),
        color,
        false,
    );
    if enabled {
        resp.on_hover_cursor(egui::CursorIcon::PointingHand)
    } else {
        resp
    }
}

/// Стрелка отправки без фона. Высота та же, что у поля ввода.
pub fn composer_send(ui: &mut Ui, light: bool, enabled: bool) -> Response {
    let enabled = enabled && ui.is_enabled();
    let (rect, resp) = ui.allocate_exact_size(
        vec2(34.0, 40.0),
        if enabled {
            Sense::click()
        } else {
            Sense::hover()
        },
    );
    let hovered = enabled && resp.hovered();
    let color = if !enabled {
        muted(light).gamma_multiply(0.6)
    } else if hovered {
        accent(light).gamma_multiply(0.85)
    } else {
        accent(light)
    };
    paint_icon(
        ui.painter(),
        Icon::Send,
        Rect::from_center_size(rect.center(), Vec2::splat(20.0)),
        color,
        false,
    );
    if enabled {
        resp.on_hover_cursor(egui::CursorIcon::PointingHand)
    } else {
        resp
    }
}

// ───────────────────────── Аватары ─────────────────────────

pub struct Avatar<'a> {
    image: Option<&'a egui::TextureHandle>,
    name: &'a str,
    size: f32,
    status: Option<(Color32, Color32)>,
    ring: Option<Color32>,
    sense: Sense,
    overlay: Option<Icon>,
}
impl<'a> Avatar<'a> {
    pub fn new(image: Option<&'a egui::TextureHandle>, name: &'a str, size: f32) -> Self {
        Self {
            image,
            name,
            size,
            status: None,
            ring: None,
            sense: Sense::hover(),
            overlay: None,
        }
    }
    /// Точка статуса в углу: (цвет точки, цвет фона под аватаром).
    pub fn status(mut self, dot: Color32, cutout: Color32) -> Self {
        self.status = Some((dot, cutout));
        self
    }
    /// Зелёное кольцо вокруг говорящего.
    pub fn ring(mut self, color: Option<Color32>) -> Self {
        self.ring = color;
        self
    }
    /// Кликабельный аватар с иконкой при наведении.
    pub fn clickable(mut self, overlay: Icon) -> Self {
        self.sense = Sense::click();
        self.overlay = Some(overlay);
        self
    }
    pub fn show(self, ui: &mut Ui) -> Response {
        let size = self.size;
        let (rect, resp) = ui.allocate_exact_size(Vec2::splat(size), self.sense);
        let center = rect.center();
        if let Some(image) = self.image {
            ui.put(
                rect,
                egui::Image::new((image.id(), vec2(size, size)))
                    .fit_to_exact_size(vec2(size, size))
                    .corner_radius((size / 2.0) as u8),
            );
        } else {
            let (r, g, b) = PALETTE[palette_index(self.name)];
            ui.painter()
                .circle_filled(center, size / 2.0, Color32::from_rgb(r, g, b));
            ui.painter().text(
                center,
                Align2::CENTER_CENTER,
                self.name
                    .chars()
                    .next()
                    .unwrap_or('T')
                    .to_uppercase()
                    .to_string(),
                FontId::new(size * 0.45, FontFamily::Name("Semibold".into())),
                Color32::WHITE,
            );
        }
        if let Some(ring) = self.ring {
            ui.painter()
                .circle_stroke(center, size / 2.0 + 1.5, Stroke::new(2.0_f32, ring));
        }
        if let Some((dot, cutout)) = self.status {
            let c = pos2(rect.right() - size * 0.14, rect.bottom() - size * 0.14);
            let r = (size * 0.17).max(4.0);
            ui.painter().circle_filled(c, r + 2.0, cutout);
            ui.painter().circle_filled(c, r, dot);
        }
        if let Some(icon) = self.overlay {
            if resp.hovered() {
                ui.painter()
                    .circle_filled(center, size / 2.0, Color32::from_black_alpha(150));
                paint_icon(
                    ui.painter(),
                    icon,
                    Rect::from_center_size(center, Vec2::splat(size * 0.32)),
                    Color32::WHITE,
                    false,
                );
            }
        }
        resp
    }
}

// ───────────────────────── Строки и виджеты ─────────────────────────

/// Строка навигации: иконка слева, подпись, подсветка при наведении.
pub fn nav_row(
    ui: &mut Ui,
    light: bool,
    icon: Icon,
    label: &str,
    selected_row: bool,
    icon_color: Option<Color32>,
) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 34.0), Sense::click());
    let hovered = resp.hovered();
    if selected_row {
        ui.painter()
            .rect_filled(rect, CornerRadius::same(6), selected(light));
    } else if hovered {
        ui.painter()
            .rect_filled(rect, CornerRadius::same(6), hover(light));
    }
    let fg = if selected_row || hovered {
        text(light)
    } else {
        muted(light)
    };
    paint_icon(
        ui.painter(),
        icon,
        Rect::from_center_size(pos2(rect.left() + 20.0, rect.center().y), Vec2::splat(18.0)),
        icon_color.unwrap_or(fg),
        false,
    );
    ui.painter().text(
        pos2(rect.left() + 38.0, rect.center().y),
        Align2::LEFT_CENTER,
        label,
        FontId::new(
            14.5,
            if selected_row {
                FontFamily::Name("Semibold".into())
            } else {
                FontFamily::Proportional
            },
        ),
        fg,
    );
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Произвольная кликабельная строка (участники). Содержимое выкладывается слева направо.
pub fn row(
    ui: &mut Ui,
    light: bool,
    height: f32,
    indent: f32,
    pad_right: f32,
    add: impl FnOnce(&mut Ui),
) -> Response {
    let (full, resp) = ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::click());
    let rect = Rect::from_min_max(pos2(full.left() + indent, full.top()), full.max);
    if resp.hovered() {
        ui.painter()
            .rect_filled(rect, CornerRadius::same(6), hover(light));
    }
    let mut child = ui.new_child(
        UiBuilder::new()
            .max_rect(Rect::from_min_max(
                pos2(rect.left() + 8.0, rect.top()),
                pos2(rect.right() - pad_right, rect.bottom()),
            ))
            .layout(Layout::left_to_right(Align::Center)),
    );
    child.style_mut().interaction.selectable_labels = false;
    child.spacing_mut().item_spacing.x = 8.0;
    add(&mut child);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Подпись раздела: «ТЕКСТОВЫЕ КАНАЛЫ».
pub fn section(ui: &mut Ui, label: &str, light: bool) {
    ui.horizontal(|ui| {
        ui.add_space(10.0);
        ui.label(RichText::new(label).size(11.5).strong().color(muted(light)));
    });
    ui.add_space(2.0);
}

pub fn hint(label: impl Into<String>, light: bool) -> RichText {
    RichText::new(label)
        .size(14.0)
        .color(muted(light).gamma_multiply(0.8))
}

pub fn primary(label: &str) -> egui::Button<'_> {
    egui::Button::new(RichText::new(label).color(Color32::WHITE).strong())
        .fill(BLUE)
        .stroke(Stroke::NONE)
        .min_size(vec2(0.0, 36.0))
}
pub fn secondary(label: &str) -> egui::Button<'_> {
    egui::Button::new(label).min_size(vec2(0.0, 36.0))
}

/// Карточка настроек.
pub fn card<R>(ui: &mut Ui, light: bool, add: impl FnOnce(&mut Ui) -> R) -> R {
    egui::Frame::new()
        .fill(surface(light))
        .stroke(Stroke::new(1.0_f32, line(light)))
        .corner_radius(10)
        .inner_margin(16)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui)
        })
        .inner
}
pub fn divider(ui: &mut Ui, light: bool) {
    ui.add_space(10.0);
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
    ui.painter()
        .rect_filled(rect, CornerRadius::ZERO, line(light));
    ui.add_space(10.0);
}

/// Переключатель (как в Discord).
pub fn toggle(ui: &mut Ui, on: &mut bool) -> Response {
    let (rect, mut resp) = ui.allocate_exact_size(vec2(42.0, 24.0), Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    let t = ui.ctx().animate_bool_with_time(resp.id, *on, 0.12);
    let off = if ui.visuals().dark_mode {
        Color32::from_rgb(110, 114, 124)
    } else {
        Color32::from_rgb(163, 166, 174)
    };
    ui.painter()
        .rect_filled(rect, CornerRadius::same(12), off.lerp_to_gamma(BLUE, t));
    let cx = egui::lerp((rect.left() + 12.0)..=(rect.right() - 12.0), t);
    ui.painter()
        .circle_filled(pos2(cx, rect.center().y), 9.0, Color32::WHITE);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Строка настройки: слева название и пояснение, справа элемент управления.
pub fn setting_row(
    ui: &mut Ui,
    light: bool,
    title: &str,
    desc: &str,
    control: impl FnOnce(&mut Ui),
) {
    ui.horizontal(|ui| {
        let w = (ui.available_width() - 120.0).max(140.0);
        ui.allocate_ui_with_layout(vec2(w, 0.0), Layout::top_down(Align::Min), |ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            ui.label(RichText::new(title).size(15.0));
            if !desc.is_empty() {
                ui.label(RichText::new(desc).size(12.5).color(muted(light)));
            }
        });
        ui.with_layout(Layout::right_to_left(Align::Min), control);
    });
}

/// Подпись над полем ввода.
pub fn field_label(ui: &mut Ui, label: &str, light: bool) {
    ui.label(RichText::new(label).size(12.0).strong().color(muted(light)));
    ui.add_space(2.0);
}

/// «Клавиша» — рамка с названием кнопки.
pub fn keycap(ui: &mut Ui, light: bool, label: &str) {
    egui::Frame::new()
        .fill(deep(light))
        .corner_radius(6)
        .inner_margin(egui::Margin::symmetric(8, 3))
        .show(ui, |ui| {
            ui.label(RichText::new(label).monospace().size(12.5));
        });
}
