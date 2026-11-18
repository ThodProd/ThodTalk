use crate::ui;
use eframe::egui::{
    self, Align2, Color32, FontId, Rect, RichText, Sense, Stroke, StrokeKind, pos2, vec2,
};
use thodtalk::share::{Source, SourceInfo};

pub const ORANGE: Color32 = Color32::from_rgb(255, 140, 57);
pub const STAGE: Color32 = Color32::from_rgb(11, 13, 18);
pub const TOOLBAR: Color32 = Color32::from_rgb(23, 26, 34);
pub const TEXT: Color32 = Color32::from_rgb(238, 240, 246);
pub const MUTED: Color32 = Color32::from_rgb(162, 171, 189);

pub fn first_source(sources: &[SourceInfo], windows: bool) -> usize {
    sources
        .iter()
        .position(|item| {
            if windows {
                matches!(item.source, Source::Window(_))
            } else {
                matches!(item.source, Source::Monitor { .. })
            }
        })
        .unwrap_or(0)
}

pub fn monitors(ui: &mut egui::Ui, sources: &[SourceInfo], selected: &mut usize, light: bool) {
    let displays: Vec<_> = sources
        .iter()
        .enumerate()
        .filter_map(|(index, item)| {
            let Source::Monitor {
                x,
                y,
                width,
                height,
            } = item.source
            else {
                return None;
            };
            Some((
                index,
                Rect::from_min_size(pos2(x as f32, y as f32), vec2(width as f32, height as f32)),
            ))
        })
        .collect();
    let (canvas, _) = ui.allocate_exact_size(vec2(ui.available_width(), 222.0), Sense::hover());
    ui.painter().rect_filled(canvas, 12, ui::deep(light));
    for ix in 0..(canvas.width() as usize / 16) {
        for iy in 0..(canvas.height() as usize / 16) {
            ui.painter().circle_filled(
                canvas.min + vec2(12.0 + ix as f32 * 16.0, 12.0 + iy as f32 * 16.0),
                1.0,
                ui::line(light),
            );
        }
    }
    let bounds = displays.iter().map(|(_, rect)| *rect).reduce(Rect::union);
    let Some(bounds) = bounds else {
        ui.painter().text(
            canvas.center(),
            Align2::CENTER_CENTER,
            "Весь рабочий стол",
            FontId::proportional(16.0),
            ui::text(light),
        );
        return;
    };
    let usable = canvas.shrink2(vec2(28.0, 23.0));
    let scale = (usable.width() / bounds.width()).min(usable.height() / bounds.height());
    let origin = usable.center() - bounds.size() * scale / 2.0;
    for &(index, display) in &displays {
        let rect = Rect::from_min_size(
            origin + (display.min - bounds.min) * scale,
            display.size() * scale,
        );
        let rect = rect.shrink((rect.width().min(rect.height()) * 0.04).min(4.0));
        let item = &sources[index];
        let hit = ui
            .interact(rect, ui.id().with(index), Sense::click())
            .on_hover_text(format!("{} · {}", item.title, item.detail))
            .on_hover_cursor(egui::CursorIcon::PointingHand);
        if hit.clicked() {
            *selected = index;
        }
        let active = index == *selected || *selected == 0;
        let fill = if active {
            if light {
                Color32::from_rgb(255, 236, 219)
            } else {
                Color32::from_rgb(64, 40, 32)
            }
        } else if hit.hovered() {
            ui::hover(light)
        } else {
            ui::surface(light)
        };
        let edge = if active { ORANGE } else { ui::muted(light) };
        let painter = ui.painter().with_clip_rect(rect);
        painter.rect(
            rect,
            9,
            fill,
            Stroke::new(if active { 2.0_f32 } else { 1.0_f32 }, edge),
            StrokeKind::Inside,
        );
        if rect.width() > 44.0 && rect.height() > 30.0 {
            let number = item.title.split_whitespace().last().unwrap_or("");
            painter.text(
                rect.center(),
                Align2::CENTER_CENTER,
                number,
                FontId::proportional((rect.height() * 0.26).clamp(15.0, 34.0)),
                ui::text(light),
            );
        }
    }
    if displays.len() > 1 {
        ui.horizontal_wrapped(|ui| {
            for &(index, _) in &displays {
                ui.selectable_value(selected, index, &sources[index].title);
            }
            ui.selectable_value(selected, 0, "Все мониторы");
        });
    }
}

pub fn windows(
    ui: &mut egui::Ui,
    sources: &[SourceInfo],
    selected: &mut usize,
    light: bool,
) -> bool {
    let mut start = false;
    egui::ScrollArea::vertical()
        .max_height(252.0)
        .min_scrolled_height(222.0)
        .show(ui, |ui| {
            let mut any = false;
            for (index, item) in sources
                .iter()
                .enumerate()
                .filter(|(_, item)| matches!(item.source, Source::Window(_)))
            {
                any = true;
                let (rect, hit) =
                    ui.allocate_exact_size(vec2(ui.available_width(), 51.0), Sense::click());
                let active = index == *selected;
                let fill = if active || hit.hovered() {
                    ui::hover(light)
                } else {
                    ui::deep(light)
                };
                let edge = if active { ORANGE } else { ui::line(light) };
                ui.painter().rect(
                    rect,
                    10,
                    fill,
                    Stroke::new(1.0_f32, edge),
                    StrokeKind::Inside,
                );
                let icon = Rect::from_center_size(
                    pos2(rect.left() + 27.0, rect.center().y),
                    vec2(23.0, 23.0),
                );
                ui::paint_icon(
                    ui.painter(),
                    ui::Icon::Monitor,
                    icon,
                    ui::muted(light),
                    false,
                );
                let mut text = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_max(
                    rect.left_top() + vec2(50.0, 7.0),
                    rect.right_bottom() - vec2(12.0, 4.0),
                )));
                text.add(egui::Label::new(RichText::new(&item.title).strong()).truncate())
                    .on_hover_text(&item.title);
                text.label(
                    RichText::new(&item.detail)
                        .size(11.0)
                        .color(ui::muted(light)),
                );
                if hit.clicked() {
                    *selected = index;
                }
                start |= hit.double_clicked();
            }
            if !any {
                ui.label("Нет открытых окон для демонстрации");
            }
        });
    start
}

pub fn viewer_style(ui: &mut egui::Ui) {
    let style = ui.style_mut();
    style.visuals = egui::Visuals::dark();
    style.visuals.window_fill = TOOLBAR;
    style.visuals.selection.bg_fill = Color32::from_rgb(110, 61, 37);
    for widget in [
        &mut style.visuals.widgets.inactive,
        &mut style.visuals.widgets.hovered,
        &mut style.visuals.widgets.active,
        &mut style.visuals.widgets.open,
    ] {
        widget.corner_radius = egui::CornerRadius::same(8);
    }
}
