//! The keyboard drawn on screen: every key with its note, lit up while it is pressed.

use std::collections::HashSet;

use eframe::egui::{
    self, Align2, Color32, CornerRadius, FontId, Rect, Sense, Stroke, StrokeKind, pos2, vec2,
};

use crate::devices::KeyboardInfo;
use crate::layout::{self, Instrument, KEYS, NoteNames};
use crate::theme::{self, BONE, BRONZE, DIM, FAINT, INK, SOOT, TERRACOTTA};

/// Left edge of the `1` key, and the total width up to the right edge of the `]` key, in key widths.
const LEFT: f32 = 1.0;
const WIDTH_UNITS: f32 = 12.5;

/// How a key is drawn.
enum Look {
    /// Held down right now.
    Pressed,
    /// A C on an accordion: the "marked" buttons of a real one.
    Mark,
    /// A sharp or flat note: dark, with a light outline.
    Sharp,
    /// A natural note: a block of light.
    Natural,
    /// The keyboard has the key, but it plays nothing in this mode.
    Silent,
    /// The keyboard does not report this key.
    Absent,
}

/// Draws the 45 keys in their physical position.
///
/// * `info`: the keyboard being shown; the keys it does not report stay off (`None` = all present);
/// * `held`: codes of the keys held down right now.
pub fn paint(
    ui: &mut egui::Ui,
    instrument: Instrument,
    info: Option<&KeyboardInfo>,
    octave: i8,
    names: NoteNames,
    held: &HashSet<u16>,
) {
    let unit = (ui.available_width() / WIDTH_UNITS).clamp(34.0, 50.0);
    let (full, _) = ui.allocate_exact_size(vec2(ui.available_width(), 4.0 * unit), Sense::hover());
    // the keyboard sits in the middle of the available width
    let area = Rect::from_min_size(
        pos2(
            full.left() + ((full.width() - WIDTH_UNITS * unit) / 2.0).max(0.0),
            full.top(),
        ),
        vec2(WIDTH_UNITS * unit, 4.0 * unit),
    );
    let painter = ui.painter_at(full);
    let ppp = ui.ctx().pixels_per_point();
    let snap = |v: f32| (v * ppp).round() / ppp;
    let legend_font = FontId::monospace(10.5);

    for key in &KEYS {
        let center = pos2(
            area.left() + (key.x() - LEFT) * unit,
            area.top() + (key.row as f32 + 0.5) * unit,
        );
        let cell = Rect::from_center_size(center, vec2(unit * 0.9, unit * 0.9));
        let rect = Rect::from_min_max(
            pos2(snap(cell.left()), snap(cell.top())),
            pos2(snap(cell.right()), snap(cell.bottom())),
        );
        let present = info.is_none_or(|i| i.can_play(key.code));
        let note = instrument
            .note_for_code(key.code, octave)
            .filter(|_| present);

        let look = match note {
            None if present => Look::Silent,
            None => Look::Absent,
            Some(_) if held.contains(&key.code) => Look::Pressed,
            // on an accordion the Cs are "the marked keys", in bronze like the buttons of the real thing
            Some(n) if instrument != Instrument::Piano && n % 12 == 0 => Look::Mark,
            Some(n) if layout::is_black(n) => Look::Sharp,
            Some(_) => Look::Natural,
        };
        // (fill, text, outline, dashed outline)
        let (fill, ink, edge, dashed) = match look {
            Look::Pressed => (TERRACOTTA, INK, TERRACOTTA, false),
            Look::Mark => (BRONZE, INK, BRONZE, false),
            Look::Sharp => (SOOT, BONE, BONE, false),
            Look::Natural => (BONE, INK, BONE, false),
            Look::Silent => (Color32::TRANSPARENT, DIM, DIM, true),
            Look::Absent => (Color32::TRANSPARENT, FAINT, FAINT, true),
        };
        if dashed {
            // a dashed outline is centred on its line, so it goes half a pixel inside
            theme::dashed_rect(&painter, rect.shrink(0.5 / ppp), Stroke::new(1.0, edge));
        } else {
            painter.rect(
                rect,
                CornerRadius::ZERO,
                fill,
                Stroke::new(1.0, edge),
                StrokeKind::Inside,
            );
        }
        painter.text(
            rect.left_top() + vec2(4.0, 2.0),
            Align2::LEFT_TOP,
            key.label,
            legend_font.clone(),
            ink,
        );
        if let Some(n) = note {
            let name = layout::note_name(n, names);
            // IBM Plex Mono is 0.6 em wide: shrink the text until it fits inside the key
            let size = (rect.width() * 0.88 / (name.chars().count() as f32 * 0.6))
                .min(unit * 0.3)
                .clamp(8.0, 15.0);
            painter.text(
                rect.center() + vec2(0.0, 4.0),
                Align2::CENTER_CENTER,
                name,
                FontId::monospace(size),
                ink,
            );
        }

        let tip = match (note, present) {
            (Some(n), _) => format!(
                "Key {} = {} (MIDI note {n})",
                key.label,
                layout::note_name(n, names)
            ),
            (None, false) => format!("Key {}: this keyboard does not report it", key.label),
            (None, true) => format!("Key {}: does not play in this mode", key.label),
        };
        ui.interact(rect, ui.scope_id().with(("key", key.code)), Sense::hover())
            .on_hover_text(tip);
    }
}
