//! The look of Keynortia: a terminal-style (TUI) skin for egui, drawn after the logo.
//!
//! Cream line art on near-black, frames made of dashes like the dashed strokes of the drawing, a nail
//! ornament, flat square widgets that invert their colours when hovered (like a terminal selection)
//! and a touch of Etruscan lettering: a Roman-inscription title font and the real Etruscan alphabet.

use std::sync::Arc;

use eframe::egui::text::{LayoutJob, TextFormat};
use eframe::egui::{
    self, Align, Button, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId,
    Margin, Painter, Pos2, Rect, Response, RichText, Sense, Shadow, Shape, Stroke, TextStyle, Ui,
    pos2, vec2,
};

/// The near-black background of the logo.
pub const INK: Color32 = Color32::from_rgb(9, 9, 8);
/// A slightly raised surface (the sharp keys, input boxes).
pub const SOOT: Color32 = Color32::from_rgb(28, 26, 23);
/// The cream of the logo's line art: text and frames.
pub const BONE: Color32 = Color32::from_rgb(227, 209, 180);
/// Dashed frames: the same cream, a little softer.
const BONE_SOFT: Color32 = Color32::from_rgb(168, 153, 130);
/// Secondary text.
pub const DIM: Color32 = Color32::from_rgb(146, 128, 106);
/// Quiet lines: things that are idle or absent.
pub const FAINT: Color32 = Color32::from_rgb(78, 69, 59);
/// Bronze, the colour of the nails: highlights.
pub const BRONZE: Color32 = Color32::from_rgb(201, 150, 82);
/// Etruscan terracotta: pressed keys and errors.
pub const TERRACOTTA: Color32 = Color32::from_rgb(214, 100, 62);

/// The name in the Etruscan alphabet (Old Italic letters; Etruscan has no Y, so U stands in for it).
pub const ETRUSCAN_NAME: &str = "𐌊𐌄𐌖𐌍𐌏𐌓𐌕𐌉𐌀";

const DASH: f32 = 5.0;
const GAP: f32 = 3.0;

/// Heavier monospace, for titles of panes and sections.
pub fn bold_family() -> FontFamily {
    FontFamily::Name("bold".into())
}

/// Roman-inscription capitals, for the title.
pub fn title_family() -> FontFamily {
    FontFamily::Name("title".into())
}

/// Secondary text.
pub fn dim(text: impl Into<String>) -> RichText {
    RichText::new(text).color(DIM)
}

/// Heavier text.
pub fn bold(text: impl Into<String>) -> RichText {
    RichText::new(text).family(bold_family())
}

/// Installs the fonts, the colours and the spacing. Call once at start-up.
pub fn install(ctx: &egui::Context) {
    ctx.set_fonts(fonts());
    ctx.set_theme(egui::ThemePreference::Dark);
    ctx.all_styles_mut(skin);
}

fn fonts() -> FontDefinitions {
    let mut fonts = FontDefinitions::default();
    let mut add = |name: &str, data: FontData| {
        fonts.font_data.insert(name.to_owned(), Arc::new(data));
    };
    add(
        "plex",
        FontData::from_static(include_bytes!("../assets/fonts/IBMPlexMono-Regular.ttf")),
    );
    add(
        "plex-bold",
        FontData::from_static(include_bytes!("../assets/fonts/IBMPlexMono-Bold.ttf")),
    );
    add(
        "cinzel",
        FontData::from_static(include_bytes!("../assets/fonts/Cinzel-VF.ttf")).tweak(
            egui::FontTweak {
                coords: egui::epaint::text::VariationCoords::new([(b"wght", 700.0)]),
                ..Default::default()
            },
        ),
    );
    add(
        "etruscan",
        FontData::from_static(include_bytes!(
            "../assets/fonts/NotoSansOldItalic-Regular.ttf"
        )),
    );

    // Everything is monospace, like in a terminal. The built-in fonts stay behind as fallbacks
    // for the symbols IBM Plex Mono lacks, and the Etruscan letters come last.
    let mono = fonts.families.entry(FontFamily::Monospace).or_default();
    mono.insert(0, "plex".to_owned());
    mono.push("etruscan".to_owned());
    let mono = mono.clone();
    fonts.families.insert(FontFamily::Proportional, mono);
    fonts.families.insert(
        bold_family(),
        ["plex-bold", "plex", "Hack", "etruscan"]
            .map(String::from)
            .to_vec(),
    );
    fonts.families.insert(
        title_family(),
        ["cinzel", "plex", "Hack", "etruscan"]
            .map(String::from)
            .to_vec(),
    );
    fonts
}

fn skin(style: &mut egui::Style) {
    style.visuals = visuals();

    let mono = |size: f32| FontId::new(size, FontFamily::Monospace);
    style.text_styles = [
        (TextStyle::Heading, FontId::new(20.0, title_family())),
        (TextStyle::Body, mono(13.5)),
        (TextStyle::Monospace, mono(13.5)),
        (TextStyle::Button, mono(13.5)),
        (TextStyle::Small, mono(11.5)),
    ]
    .into();

    style.spacing.item_spacing = vec2(8.0, 5.0);
    style.spacing.button_padding = vec2(7.0, 2.0);
    style.spacing.interact_size = vec2(24.0, 22.0);
    style.spacing.slider_width = 140.0;
    style.spacing.scroll = egui::style::ScrollStyle::solid();
    // a terminal does not fade or slide: everything switches at once
    style.animation_time = 0.0;
}

fn visuals() -> egui::Visuals {
    let mut v = egui::Visuals::dark();
    v.panel_fill = INK;
    v.window_fill = INK;
    v.extreme_bg_color = Color32::BLACK;
    v.faint_bg_color = SOOT;
    v.code_bg_color = SOOT;
    v.hyperlink_color = BRONZE;
    v.warn_fg_color = BRONZE;
    v.error_fg_color = TERRACOTTA;

    v.window_corner_radius = CornerRadius::ZERO;
    v.menu_corner_radius = CornerRadius::ZERO;
    v.window_shadow = Shadow::NONE;
    v.popup_shadow = Shadow::NONE;
    v.window_stroke = Stroke::new(1.0, BONE_SOFT);

    // the selection is a block of inverted colours, like in a terminal
    v.selection.bg_fill = BONE;
    v.selection.stroke = Stroke::new(1.0, INK);

    v.slider_trailing_fill = true;
    v.handle_shape = egui::style::HandleShape::Rect { aspect_ratio: 0.5 };
    v.collapsing_header_frame = false;

    let w = &mut v.widgets;
    for state in [
        &mut w.noninteractive,
        &mut w.inactive,
        &mut w.hovered,
        &mut w.active,
        &mut w.open,
    ] {
        state.corner_radius = CornerRadius::ZERO;
        state.expansion = 0.0;
    }
    // labels, separators, frames
    w.noninteractive.bg_fill = INK;
    w.noninteractive.weak_bg_fill = INK;
    w.noninteractive.bg_stroke = Stroke::new(1.0, FAINT);
    w.noninteractive.fg_stroke = Stroke::new(1.0, BONE);
    // widgets at rest: an outline and no fill
    w.inactive.bg_fill = SOOT;
    w.inactive.weak_bg_fill = Color32::TRANSPARENT;
    w.inactive.bg_stroke = Stroke::new(1.0, FAINT);
    w.inactive.fg_stroke = Stroke::new(1.0, BONE);
    // under the pointer: reverse video
    w.hovered.bg_fill = BONE;
    w.hovered.weak_bg_fill = BONE;
    w.hovered.bg_stroke = Stroke::new(1.0, BONE);
    w.hovered.fg_stroke = Stroke::new(1.0, INK);
    // pressed
    w.active.bg_fill = BRONZE;
    w.active.weak_bg_fill = BRONZE;
    w.active.bg_stroke = Stroke::new(1.0, BRONZE);
    w.active.fg_stroke = Stroke::new(1.0, INK);
    // a drop-down that is open
    w.open.bg_fill = SOOT;
    w.open.weak_bg_fill = SOOT;
    w.open.bg_stroke = Stroke::new(1.0, BONE_SOFT);
    w.open.fg_stroke = Stroke::new(1.0, BONE);
    v
}

/// Frame of the top and bottom panels: flat, on the logo's black.
pub fn panel_frame() -> egui::Frame {
    egui::Frame::NONE
        .fill(INK)
        .inner_margin(Margin::symmetric(14, 6))
}

/// Centre of the physical pixel that contains `v`: a one-point line drawn there stays sharp.
fn crisp(ppp: f32, v: f32) -> f32 {
    ((v * ppp).floor() + 0.5) / ppp
}

fn outline(painter: &Painter, dashed: bool, stroke: Stroke, path: &[Pos2]) {
    if dashed {
        painter.extend(Shape::dashed_line(path, stroke, DASH, GAP));
    } else {
        painter.add(Shape::line(path.to_vec(), stroke));
    }
}

/// How the border of a pane is drawn.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Edge {
    /// Dashes, like the strokes of the logo.
    Dashed,
    /// A solid line: the pane that has the focus.
    Solid,
}

/// A framed pane: the title sits on the top edge as a block of inverted colours, like the label of a
/// window in a text-mode program.
pub fn pane<R>(ui: &mut Ui, title: &str, edge: Edge, add_contents: impl FnOnce(&mut Ui) -> R) -> R {
    let tag = ui
        .painter()
        .layout_no_wrap(title.to_owned(), FontId::new(12.5, bold_family()), INK);
    let tag_size = tag.size() + vec2(14.0, 4.0);
    let inner = egui::Frame::NONE
        .inner_margin(Margin {
            left: 12,
            right: 12,
            top: (tag_size.y / 2.0).ceil() as i8 + 8,
            bottom: 10,
        })
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add_contents(ui)
        });

    let ppp = ui.ctx().pixels_per_point();
    let outer = inner.response.rect;
    let (left, right) = (crisp(ppp, outer.left()), crisp(ppp, outer.right() - 1.0));
    let (top, bottom) = (
        crisp(ppp, outer.top() + tag_size.y / 2.0),
        crisp(ppp, outer.bottom() - 1.0),
    );
    let color = match edge {
        Edge::Dashed => BONE_SOFT,
        Edge::Solid => BONE,
    };
    let tag_left = left + 12.0;
    let tag_right = tag_left + tag_size.x;
    // one path all the way round, with a gap where the title sits, so that the dashes run on at the corners
    outline(
        ui.painter(),
        edge == Edge::Dashed,
        Stroke::new(1.0, color),
        &[
            pos2(tag_right + 4.0, top),
            pos2(right, top),
            pos2(right, bottom),
            pos2(left, bottom),
            pos2(left, top),
            pos2(tag_left - 4.0, top),
        ],
    );
    let tag_rect = Rect::from_min_size(pos2(tag_left, (top - tag_size.y / 2.0).round()), tag_size);
    ui.painter()
        .rect_filled(tag_rect, CornerRadius::ZERO, color);
    ui.painter()
        .galley(tag_rect.center() - tag.size() / 2.0, tag, INK);
    inner.inner
}

/// A section title followed by a dashed rule that runs to the right edge.
pub fn rule(ui: &mut Ui, text: &str) {
    ui.horizontal(|ui| {
        ui.label(bold(text).color(BONE));
        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 16.0), Sense::hover());
        let y = ui.painter().round_to_pixel_center(rect.center().y);
        ui.painter().extend(Shape::dashed_line(
            &[pos2(rect.left(), y), pos2(rect.right(), y)],
            Stroke::new(1.0, FAINT),
            DASH,
            GAP,
        ));
    });
}

/// The ornament of the logo: a dashed rule with a nail driven through the middle.
pub fn nail_rule(ui: &mut Ui) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 22.0), Sense::hover());
    let painter = ui.painter();
    let cx = painter.round_to_pixel_center(rect.center().x);
    let y = painter.round_to_pixel_center(rect.center().y + 1.0);
    let rule = Stroke::new(1.0, BONE_SOFT);
    for (from, to) in [(rect.left(), cx - 16.0), (cx + 16.0, rect.right())] {
        painter.extend(Shape::dashed_line(
            &[pos2(from, y), pos2(to, y)],
            rule,
            DASH,
            GAP,
        ));
    }
    let head = rect.top() + 3.0;
    let point = rect.bottom() - 5.0;
    painter.line_segment(
        [pos2(cx - 6.0, head), pos2(cx + 6.0, head)],
        Stroke::new(2.5, BONE),
    );
    painter.line_segment([pos2(cx, head), pos2(cx, point)], Stroke::new(2.0, BONE));
    painter.add(Shape::convex_polygon(
        vec![
            pos2(cx - 1.0, point),
            pos2(cx + 1.0, point),
            pos2(cx, rect.bottom()),
        ],
        BONE,
        Stroke::NONE,
    ));
}

/// The title block: the name in inscription capitals, the same name in Etruscan letters, the tagline.
pub fn banner(ui: &mut Ui) {
    ui.vertical_centered(|ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        ui.label(
            RichText::new("KEYNORTIA")
                .font(FontId::new(34.0, title_family()))
                .extra_letter_spacing(7.0)
                .color(BONE),
        );
        let mut line = LayoutJob::default();
        line.append(
            ETRUSCAN_NAME,
            0.0,
            TextFormat {
                font_id: FontId::monospace(17.0),
                color: BRONZE,
                extra_letter_spacing: 3.0,
                valign: Align::Center,
                ..Default::default()
            },
        );
        line.append(
            "   ·   turn your computer keyboard into a MIDI instrument",
            0.0,
            TextFormat {
                font_id: FontId::monospace(13.5),
                color: DIM,
                valign: Align::Center,
                ..Default::default()
            },
        );
        ui.label(line);
    });
    nail_rule(ui);
}

/// A dashed outline, for the keys that do not play.
pub fn dashed_rect(painter: &Painter, rect: Rect, stroke: Stroke) {
    painter.extend(Shape::dashed_line(
        &[
            rect.left_top(),
            rect.right_top(),
            rect.right_bottom(),
            rect.left_bottom(),
            rect.left_top(),
        ],
        stroke,
        4.0,
        3.0,
    ));
}

/// A button written as `[ label ]`; under the pointer it turns into a block of inverted colours.
pub fn button(ui: &mut Ui, label: &str) -> Response {
    ui.add(Button::new(format!("[ {label} ]")).frame_when_inactive(false))
}

/// Like [`button`], greyed out and inert when `enabled` is false.
pub fn button_if(ui: &mut Ui, enabled: bool, label: &str) -> Response {
    ui.add_enabled(
        enabled,
        Button::new(format!("[ {label} ]")).frame_when_inactive(false),
    )
}

/// A check box written as `[x] label` or `[ ] label`.
pub fn checkbox(ui: &mut Ui, value: &mut bool, label: &str) -> Response {
    let mark = if *value { 'x' } else { ' ' };
    let mut response = ui.add(Button::new(format!("[{mark}] {label}")).frame_when_inactive(false));
    if response.clicked() {
        *value = !*value;
        response.mark_changed();
    }
    response
}
