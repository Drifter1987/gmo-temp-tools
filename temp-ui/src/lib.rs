//! The shared visual language for the GMO TempTools desktop window.
//!
//! One palette and one set of primitives keep Mail, SMS, OTP and the shell
//! looking like one application instead of four stitched-together tools.

use eframe::egui::{
    self, Align, Align2, Color32, CornerRadius, CursorIcon, FontId, Frame, Id, Layout, Margin,
    Response, RichText, Sense, Stroke, TextStyle, Ui, Vec2,
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Theme {
    #[default]
    Dark,
    Light,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Palette {
    pub dark: bool,
    pub canvas: Color32,
    pub rail: Color32,
    pub surface: Color32,
    pub surface_alt: Color32,
    pub surface_hover: Color32,
    pub line: Color32,
    pub line_soft: Color32,
    pub text: Color32,
    pub muted: Color32,
    pub accent: Color32,
    pub accent_soft: Color32,
    pub on_accent: Color32,
    pub warn: Color32,
    pub danger: Color32,
    pub danger_soft: Color32,
}

impl Palette {
    pub const fn of(theme: Theme) -> Self {
        match theme {
            Theme::Dark => Self {
                dark: true,
                canvas: Color32::from_rgb(13, 18, 17),
                rail: Color32::from_rgb(9, 13, 12),
                surface: Color32::from_rgb(20, 27, 25),
                surface_alt: Color32::from_rgb(26, 35, 32),
                surface_hover: Color32::from_rgb(33, 45, 41),
                line: Color32::from_rgb(41, 55, 50),
                line_soft: Color32::from_rgb(28, 38, 35),
                text: Color32::from_rgb(232, 241, 237),
                muted: Color32::from_rgb(145, 162, 155),
                accent: Color32::from_rgb(64, 214, 155),
                accent_soft: Color32::from_rgb(22, 52, 43),
                on_accent: Color32::from_rgb(6, 18, 14),
                warn: Color32::from_rgb(242, 184, 114),
                danger: Color32::from_rgb(240, 120, 120),
                danger_soft: Color32::from_rgb(52, 28, 28),
            },
            Theme::Light => Self {
                dark: false,
                canvas: Color32::from_rgb(244, 248, 246),
                rail: Color32::from_rgb(252, 254, 253),
                surface: Color32::from_rgb(255, 255, 255),
                surface_alt: Color32::from_rgb(240, 246, 243),
                surface_hover: Color32::from_rgb(233, 240, 236),
                line: Color32::from_rgb(216, 228, 222),
                line_soft: Color32::from_rgb(231, 239, 235),
                text: Color32::from_rgb(18, 33, 28),
                muted: Color32::from_rgb(76, 96, 88),
                accent: Color32::from_rgb(10, 116, 83),
                accent_soft: Color32::from_rgb(222, 242, 233),
                on_accent: Color32::WHITE,
                warn: Color32::from_rgb(154, 91, 11),
                danger: Color32::from_rgb(179, 54, 54),
                danger_soft: Color32::from_rgb(250, 235, 235),
            },
        }
    }
}

const PALETTE_KEY: &str = "gmo.temptools.palette";

/// Apply the whole visual system to an egui context. Settings calls this when
/// the appearance changes, so native and web follow the same preference.
pub fn apply(ctx: &egui::Context, theme: Theme) {
    let p = Palette::of(theme);
    ctx.data_mut(|data| data.insert_temp(Id::new(PALETTE_KEY), p));

    let mut visuals = if p.dark {
        egui::Visuals::dark()
    } else {
        egui::Visuals::light()
    };
    visuals.panel_fill = p.canvas;
    visuals.window_fill = p.surface;
    visuals.faint_bg_color = p.surface_alt;
    visuals.extreme_bg_color = p.surface_alt;
    visuals.code_bg_color = p.surface_alt;
    visuals.selection.bg_fill = p.accent_soft;
    visuals.selection.stroke = Stroke::new(1.0_f32, p.accent);
    visuals.hyperlink_color = p.accent;
    visuals.warn_fg_color = p.warn;
    visuals.error_fg_color = p.danger;
    visuals.window_stroke = Stroke::new(1.0_f32, p.line);
    visuals.window_corner_radius = CornerRadius::same(14);
    visuals.menu_corner_radius = CornerRadius::same(10);
    visuals.striped = false;
    visuals.window_shadow = egui::epaint::Shadow {
        offset: [0, 14],
        blur: 36,
        spread: 0,
        color: Color32::from_black_alpha(if p.dark { 120 } else { 32 }),
    };
    visuals.popup_shadow = visuals.window_shadow;

    let widgets = &mut visuals.widgets;
    for state in [
        &mut widgets.noninteractive,
        &mut widgets.inactive,
        &mut widgets.hovered,
        &mut widgets.active,
        &mut widgets.open,
    ] {
        state.corner_radius = CornerRadius::same(9);
    }
    widgets.noninteractive.bg_fill = p.surface;
    widgets.noninteractive.weak_bg_fill = p.surface;
    widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, p.line);
    // egui derives the text-edit hint from this color. Black in the light
    // theme keeps that derived hint above the old washed-out gray.
    widgets.noninteractive.fg_stroke = Stroke::new(
        1.0_f32,
        if p.dark { p.text } else { Color32::BLACK },
    );
    widgets.inactive.bg_fill = p.surface_alt;
    widgets.inactive.weak_bg_fill = p.surface_alt;
    widgets.inactive.bg_stroke = Stroke::new(1.0_f32, p.line);
    widgets.inactive.fg_stroke = Stroke::new(1.0_f32, p.text);
    widgets.hovered.bg_fill = p.surface_hover;
    widgets.hovered.weak_bg_fill = p.surface_hover;
    widgets.hovered.bg_stroke = Stroke::new(1.0_f32, p.line);
    widgets.hovered.fg_stroke = Stroke::new(1.0_f32, p.text);
    widgets.active.bg_fill = p.accent_soft;
    widgets.active.weak_bg_fill = p.accent_soft;
    widgets.active.bg_stroke = Stroke::new(1.0_f32, p.accent);
    widgets.active.fg_stroke = Stroke::new(1.0_f32, p.text);
    widgets.open = widgets.hovered.clone();

    ctx.set_visuals(visuals);
    ctx.style_mut(|style| {
        style.spacing.item_spacing = Vec2::new(10.0, 8.0);
        style.spacing.button_padding = Vec2::new(12.0, 6.0);
        style.spacing.interact_size = Vec2::new(40.0, 30.0);
        style.spacing.indent = 18.0;
        style.spacing.scroll.bar_width = 8.0;
        style.spacing.scroll.floating = true;
        style.spacing.scroll.foreground_color = false;
        style.spacing.scroll.dormant_handle_opacity = 0.55;
        style.text_styles.insert(TextStyle::Heading, FontId::proportional(22.0));
        style.text_styles.insert(TextStyle::Body, FontId::proportional(13.5));
        style.text_styles.insert(TextStyle::Button, FontId::proportional(13.0));
        style.text_styles.insert(TextStyle::Small, FontId::proportional(11.5));
        style
            .text_styles
            .insert(TextStyle::Monospace, FontId::monospace(12.5));
    });
}

pub fn palette(ctx: &egui::Context) -> Palette {
    ctx.data(|data| data.get_temp::<Palette>(Id::new(PALETTE_KEY)))
        .unwrap_or_else(|| Palette::of(Theme::Dark))
}

pub fn eyebrow(ui: &mut Ui, label: &str) {
    let p = palette(ui.ctx());
    ui.label(
        RichText::new(label)
            .font(FontId::monospace(9.5))
            .color(p.muted),
    );
}

pub fn page_header(ui: &mut Ui, title: &str, subtitle: &str) {
    let p = palette(ui.ctx());
    ui.label(RichText::new(title).size(25.0).strong().color(p.text));
    ui.add_space(1.0);
    ui.label(RichText::new(subtitle).size(12.5).color(p.muted));
}

pub fn card<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> egui::InnerResponse<R> {
    let p = palette(ui.ctx());
    Frame::new()
        .fill(p.surface)
        .stroke(Stroke::new(1.0_f32, p.line))
        .corner_radius(14)
        .inner_margin(Margin::same(16))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui)
        })
}

pub fn divider(ui: &mut Ui) {
    let p = palette(ui.ctx());
    ui.add_space(2.0);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 1.0), Sense::hover());
    ui.painter().rect_filled(rect, CornerRadius::ZERO, p.line_soft);
    ui.add_space(2.0);
}

pub fn primary_button(ui: &mut Ui, label: &str) -> Response {
    let p = palette(ui.ctx());
    ui.add(
        egui::Button::new(RichText::new(label).size(12.5).strong().color(p.on_accent))
            .fill(p.accent)
            .stroke(Stroke::NONE)
            .corner_radius(9)
            .min_size(Vec2::new(0.0, 32.0)),
    )
}

pub fn secondary_button(ui: &mut Ui, label: &str) -> Response {
    let p = palette(ui.ctx());
    ui.add(
        egui::Button::new(RichText::new(label).size(12.5).color(p.text))
            .fill(p.surface_alt)
            .stroke(Stroke::new(1.0_f32, p.line))
            .corner_radius(9)
            .min_size(Vec2::new(0.0, 32.0)),
    )
}

pub fn ghost_button(ui: &mut Ui, label: &str) -> Response {
    let p = palette(ui.ctx());
    ui.add(
        egui::Button::new(RichText::new(label).size(12.0).color(p.muted))
            .fill(Color32::TRANSPARENT)
            .stroke(Stroke::NONE)
            .corner_radius(8)
            .min_size(Vec2::new(0.0, 28.0)),
    )
}

pub fn danger_button(ui: &mut Ui, label: &str) -> Response {
    let p = palette(ui.ctx());
    ui.add(
        egui::Button::new(RichText::new(label).size(12.5).color(p.danger))
            .fill(Color32::TRANSPARENT)
            .stroke(Stroke::new(1.0_f32, p.line))
            .corner_radius(9)
            .min_size(Vec2::new(0.0, 32.0)),
    )
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Accent,
    Warn,
    Danger,
    Neutral,
}

pub fn chip(ui: &mut Ui, label: &str, tone: Tone) {
    let p = palette(ui.ctx());
    let (fill, color) = match tone {
        Tone::Accent => (p.accent_soft, p.accent),
        Tone::Warn => (p.surface_alt, p.warn),
        Tone::Danger => (p.danger_soft, p.danger),
        Tone::Neutral => (p.surface_alt, p.muted),
    };
    Frame::new()
        .fill(fill)
        .corner_radius(100)
        .inner_margin(Margin::symmetric(9, 3))
        .show(ui, |ui| {
            ui.label(RichText::new(label).size(10.5).strong().color(color));
        });
}

pub fn banner(ui: &mut Ui, message: &str, error: bool) {
    let p = palette(ui.ctx());
    let (fill, mark, text) = if error {
        (p.danger_soft, p.danger, p.danger)
    } else {
        (p.accent_soft, p.accent, p.text)
    };
    Frame::new()
        .fill(fill)
        .corner_radius(10)
        .inner_margin(Margin::symmetric(12, 8))
        .show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                let (rect, _) = ui.allocate_exact_size(Vec2::splat(7.0), Sense::hover());
                ui.painter().circle_filled(rect.center(), 3.5, mark);
                ui.label(RichText::new(message).size(12.0).color(text));
            });
        });
}

pub fn code_card(ui: &mut Ui, label: &str, code: &str, copy_label: &str) -> bool {
    let p = palette(ui.ctx());
    let mut copied = false;
    Frame::new()
        .fill(p.accent_soft)
        .stroke(Stroke::new(1.0_f32, p.line))
        .corner_radius(12)
        .inner_margin(Margin::symmetric(14, 11))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    eyebrow(ui, label);
                    ui.label(
                        RichText::new(code)
                            .monospace()
                            .size(24.0)
                            .color(p.accent),
                    );
                });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if secondary_button(ui, copy_label).clicked() {
                        copied = true;
                    }
                });
            });
        });
    copied
}

pub fn toggle(ui: &mut Ui, on: &mut bool, enabled: bool) -> Response {
    let p = palette(ui.ctx());
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (rect, mut response) = ui.allocate_exact_size(Vec2::new(38.0, 22.0), sense);
    let animation = ui.ctx().animate_bool(response.id, *on);
    if enabled && response.clicked() {
        *on = !*on;
        response.mark_changed();
        ui.ctx().request_repaint();
    }
    if enabled && response.hovered() {
        ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
    }
    let (fill, knob) = if !enabled {
        (p.surface_alt, p.line)
    } else if *on {
        (p.accent, p.on_accent)
    } else {
        (p.surface_alt, p.muted)
    };
    ui.painter().rect_filled(rect, CornerRadius::same(11), fill);
    if !*on && enabled {
        ui.painter().rect_stroke(
            rect,
            CornerRadius::same(11),
            Stroke::new(1.0_f32, p.line),
            egui::StrokeKind::Inside,
        );
    }
    let knob_x = egui::lerp((rect.left() + 11.0)..=(rect.right() - 11.0), animation);
    ui.painter()
        .circle_filled(egui::pos2(knob_x, rect.center().y), 8.0, knob);
    response
}

pub fn segmented<T: PartialEq + Copy>(
    ui: &mut Ui,
    current: &mut T,
    options: &[(T, &str)],
) -> bool {
    let p = palette(ui.ctx());
    let mut changed = false;
    Frame::new()
        .fill(p.surface_alt)
        .corner_radius(10)
        .inner_margin(Margin::same(3))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                for &(option, label) in options {
                    let selected = *current == option;
                    let text_color = if selected { p.text } else { p.muted };
                    let text = RichText::new(label).size(12.0).color(text_color);
                    let text = if selected { text.strong() } else { text };
                    let response = ui.add(
                        egui::Button::new(text)
                        .fill(if selected {
                            p.surface
                        } else {
                            Color32::TRANSPARENT
                        })
                        .stroke(if selected {
                            Stroke::new(1.0_f32, p.line)
                        } else {
                            Stroke::NONE
                        })
                        .corner_radius(7)
                        .min_size(Vec2::new(0.0, 26.0)),
                    );
                    if response.clicked() && !selected {
                        *current = option;
                        changed = true;
                    }
                }
            });
        });
    changed
}

pub fn nav_item(ui: &mut Ui, label: &str, hint: &str, selected: bool) -> Response {
    let p = palette(ui.ctx());
    let width = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, 50.0), Sense::click());
    if selected {
        ui.painter()
            .rect_filled(rect, CornerRadius::same(10), p.accent_soft);
        ui.painter().rect_filled(
            egui::Rect::from_min_size(rect.min + Vec2::new(0.0, 14.0), Vec2::new(3.0, 22.0)),
            CornerRadius::same(2),
            p.accent,
        );
    } else if response.hovered() {
        ui.painter()
            .rect_filled(rect, CornerRadius::same(10), p.surface_hover);
    }
    if response.hovered() {
        ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
    }
    let color = if selected { p.accent } else { p.text };
    ui.painter().text(
        rect.left_top() + Vec2::new(16.0, 11.0),
        Align2::LEFT_TOP,
        label,
        FontId::proportional(13.5),
        color,
    );
    ui.painter().text(
        rect.left_top() + Vec2::new(16.0, 30.0),
        Align2::LEFT_TOP,
        hint,
        FontId::proportional(10.5),
        p.muted,
    );
    response
}

pub fn brand(ui: &mut Ui, name: &str, tagline: &str) {
    let p = palette(ui.ctx());
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(Vec2::splat(30.0), Sense::hover());
        ui.painter()
            .rect_filled(rect, CornerRadius::same(9), p.accent);
        ui.painter().text(
            rect.center(),
            Align2::CENTER_CENTER,
            "G",
            FontId::proportional(16.0),
            p.on_accent,
        );
        ui.add_space(1.0);
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            ui.label(RichText::new(name).size(13.5).strong().color(p.text));
            ui.label(RichText::new(tagline).size(10.0).color(p.muted));
        });
    });
}

pub fn empty_state(ui: &mut Ui, kicker: &str, title: &str, hint: &str) {
    let p = palette(ui.ctx());
    ui.add_space(46.0);
    ui.vertical_centered(|ui| {
        let galley = ui.painter().layout_no_wrap(
            kicker.to_string(),
            FontId::monospace(10.0),
            p.accent,
        );
        let text_size = galley.size();
        let (rect, _) = ui.allocate_exact_size(
            text_size + Vec2::new(24.0, 10.0),
            Sense::hover(),
        );
        ui.painter()
            .rect_filled(rect, CornerRadius::same(9), p.accent_soft);
        ui.painter()
            .galley(rect.center() - text_size / 2.0, galley, p.accent);
        ui.add_space(12.0);
        ui.label(RichText::new(title).size(19.0).strong().color(p.text));
        ui.add_space(3.0);
        ui.label(RichText::new(hint).size(12.5).color(p.muted));
    });
}

/// A small theme preview used by Settings. Draws a miniature window in the
/// target palette so the choice is visible before it is applied.
pub fn theme_tile(ui: &mut Ui, theme: Theme, selected: bool) -> Response {
    let p = Palette::of(theme);
    let here = palette(ui.ctx());
    let (rect, response) = ui.allocate_exact_size(Vec2::new(164.0, 104.0), Sense::click());
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same(12), p.canvas);
    if selected {
        painter.rect_stroke(
            rect,
            CornerRadius::same(12),
            Stroke::new(2.0_f32, here.accent),
            egui::StrokeKind::Outside,
        );
    } else {
        painter.rect_stroke(
            rect,
            CornerRadius::same(12),
            Stroke::new(1.0_f32, here.line),
            egui::StrokeKind::Outside,
        );
    }
    let rail = egui::Rect::from_min_size(
        rect.min + Vec2::new(1.0, 1.0),
        Vec2::new(38.0, rect.height() - 2.0),
    );
    painter.rect_filled(rail, CornerRadius::same(0), p.rail);
    painter.circle_filled(rail.center_top() + Vec2::new(0.0, 17.0), 4.0, p.accent);
    for index in 0..3 {
        painter.rect_filled(
            egui::Rect::from_min_size(
                rail.min + Vec2::new(9.0, 32.0 + index as f32 * 16.0),
                Vec2::new(20.0, 5.0),
            ),
            CornerRadius::same(2),
            if index == 0 { p.accent_soft } else { p.surface_alt },
        );
    }
    painter.rect_filled(
        egui::Rect::from_min_size(
            rect.min + Vec2::new(50.0, 14.0),
            Vec2::new(rect.width() - 62.0, 34.0),
        ),
        CornerRadius::same(8),
        p.surface,
    );
    painter.rect_filled(
        egui::Rect::from_min_size(
            rect.min + Vec2::new(58.0, 22.0),
            Vec2::new(54.0, 5.0),
        ),
        CornerRadius::same(2),
        p.text,
    );
    painter.rect_filled(
        egui::Rect::from_min_size(
            rect.min + Vec2::new(58.0, 32.0),
            Vec2::new(76.0, 4.0),
        ),
        CornerRadius::same(2),
        p.muted,
    );
    painter.rect_filled(
        egui::Rect::from_min_size(
            rect.min + Vec2::new(50.0, 56.0),
            Vec2::new(rect.width() - 62.0, 18.0),
        ),
        CornerRadius::same(6),
        p.surface_alt,
    );
    painter.text(
        rect.left_bottom() + Vec2::new(12.0, -26.0),
        Align2::LEFT_TOP,
        theme_name(theme),
        FontId::proportional(11.5),
        p.text,
    );
    if selected {
        painter.circle_filled(
            rect.right_top() + Vec2::new(-14.0, 14.0),
            5.0,
            here.accent,
        );
    }
    if response.hovered() {
        ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
    }
    response
}

pub fn theme_name(theme: Theme) -> &'static str {
    match theme {
        Theme::Dark => "Dark",
        Theme::Light => "Light",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn light_and_dark_stay_legible() {
        for theme in [Theme::Dark, Theme::Light] {
            let p = Palette::of(theme);
            assert_ne!(p.text, p.canvas);
            assert_ne!(p.on_accent, p.accent);
            assert_ne!(p.muted, p.canvas);
        }
        assert_ne!(Palette::of(Theme::Dark).canvas, Palette::of(Theme::Light).canvas);
    }
}
