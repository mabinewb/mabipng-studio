#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;

use std::path::PathBuf;

fn main() -> eframe::Result {
    let mut paths = Vec::new();
    let mut screenshot = None;
    let mut painting = false;
    let mut framing = false;
    let mut author = None;
    let mut new_canvas = None;
    let mut zoom = None;
    let mut size = [1280.0, 860.0];
    let mut args = std::env::args_os().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--screenshot" {
            screenshot = args.next().map(PathBuf::from);
        } else if arg == "--compact" {
            size = [1000.0, 740.0];
        } else if arg == "--paint" {
            painting = true;
        } else if arg == "--frame" {
            framing = true;
        } else if arg == "--new" {
            new_canvas = args
                .next()
                .and_then(|value| value.to_string_lossy().parse::<u32>().ok());
        } else if arg == "--zoom" {
            zoom = args
                .next()
                .and_then(|value| value.to_string_lossy().parse::<f32>().ok());
        } else if arg == "--author" {
            author = args
                .next()
                .map(|value| value.to_string_lossy().into_owned());
        } else {
            paths.push(PathBuf::from(arg));
        }
    }
    let icon = image::load_from_memory_with_format(
        include_bytes!("../assets/app_icon.ico"),
        image::ImageFormat::Ico,
    )
    .ok()
    .map(|image| {
        let image = image.to_rgba8();
        eframe::egui::IconData {
            width: image.width(),
            height: image.height(),
            rgba: image.into_raw(),
        }
    });
    let mut viewport = eframe::egui::ViewportBuilder::default()
        .with_inner_size(size)
        .with_min_inner_size([1000.0, 700.0])
        .with_drag_and_drop(true);
    if let Some(icon) = icon {
        viewport = viewport.with_icon(icon);
    }
    eframe::run_native(
        "MabiPNG Studio",
        eframe::NativeOptions {
            viewport,
            renderer: eframe::Renderer::Glow,
            ..Default::default()
        },
        Box::new(move |cc| {
            let mut studio = app::Studio::new(cc, paths, screenshot, painting, author);
            if let Some(width) = new_canvas {
                studio.start_new(&cc.egui_ctx, width);
            }
            if let Some(zoom) = zoom {
                studio.set_zoom(zoom);
            }
            if framing {
                studio.start_framing();
            }
            Ok(Box::new(studio))
        }),
    )
}
