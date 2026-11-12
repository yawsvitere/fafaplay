// В release под Windows не показываем консоль.
#![cfg_attr(
    all(not(debug_assertions), target_os = "windows"),
    windows_subsystem = "windows"
)]

mod gui;
mod library;
mod player;

use eframe::egui;

fn main() -> eframe::Result {
    let icon_image = image::load_from_memory_with_format(
        include_bytes!("images/windows_app_icon.ico"),
        image::ImageFormat::Ico,
    )
    .expect("embedded Windows application icon is valid")
    .into_rgba8();
    let (icon_width, icon_height) = icon_image.dimensions();
    let icon = egui::IconData {
        rgba: icon_image.into_raw(),
        width: icon_width,
        height: icon_height,
    };

    let viewport = egui::ViewportBuilder::default()
        .with_title("fafaplay")
        .with_icon(icon)
        .with_inner_size([1220.0, 780.0])
        .with_min_inner_size([880.0, 540.0]);
    let viewport = if cfg!(target_os = "windows") {
        viewport.with_transparent(true)
    } else {
        viewport
    };

    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };

    eframe::run_native(
        "fafaplay",
        options,
        Box::new(|cc| Ok(Box::new(gui::App::new(cc)))),
    )
}
