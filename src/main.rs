//! Keynortia: turns the computer keyboard into a MIDI instrument.

// no console window behind the program on Windows
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;
mod devices;
mod engine;
mod keyboard_view;
mod keyd;
mod layout;
mod midi;
mod theme;

use std::sync::Arc;

use eframe::egui;

/// The icon of the window (taskbar, dock, title bar), the same as the one of the packages.
fn icon() -> egui::IconData {
    eframe::icon_data::from_png_bytes(include_bytes!("../assets/icon.png")).unwrap_or_default()
}

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Keynortia")
            .with_app_id("keynortia")
            .with_icon(Arc::new(icon()))
            .with_inner_size([1000.0, 860.0])
            .with_min_inner_size([760.0, 640.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Keynortia",
        options,
        Box::new(|cc| Ok(Box::new(app::KeynortiaApp::new(cc)))),
    )
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_window_icon_decodes() {
        // `icon()` falls back to an empty icon if the picture is broken: this makes that noticeable
        let icon = super::icon();
        assert_eq!((icon.width, icon.height), (256, 256));
        assert_eq!(icon.rgba.len(), 256 * 256 * 4);
        assert!(icon.rgba.chunks(4).any(|pixel| pixel[3] != 0));
    }
}
