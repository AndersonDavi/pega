// En release no abre consola.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod autostart;
mod backup;
mod code;
mod crypto;
mod i18n;
mod image;
mod import;
mod lock;
mod mask;
mod query;
mod settings;
mod settings_ui;
mod state;
mod store;
mod transform;
mod win;

use egui_software_backend::{SoftwareBackendAppConfiguration, run_app_with_software_backend};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

fn data_dir() -> PathBuf {
    std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("Pega")
}

/// Ícono de la ventana y la barra de tareas (el del .exe lo pone pega.rc).
fn window_icon() -> Arc<egui::IconData> {
    let img = image::decode_png(include_bytes!("../assets/pega-64.png")).expect("ícono");
    Arc::new(egui::IconData {
        rgba: img.pixels,
        width: img.width,
        height: img.height,
    })
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    // --restore <archivo>: lo lanza la instancia anterior justo antes de cerrarse.
    let restore = args
        .iter()
        .position(|a| a == backup::RESTORE_ARG)
        .and_then(|i| args.get(i + 1))
        .map(PathBuf::from);
    if restore.is_some() {
        if !win::wait_for_previous_instance() {
            return;
        }
    } else if win::already_running() {
        win::signal_existing();
        return;
    }
    autostart::refresh();
    let dir = data_dir();
    std::fs::create_dir_all(&dir).expect("no se pudo crear %APPDATA%\\Pega");
    let restored = restore.map(|b| backup::apply_restore(&dir, &b));
    let store =
        store::Store::open(&dir.join("pega.db")).expect("no se pudo abrir la base de datos");
    // Al cargar se aplica el idioma de los ajustes; los avisos se arman después.
    let mut loaded = state::State::load(store).expect("no se pudo leer la base de datos");
    loaded.notice = restored.map(|r| match r {
        Ok(kept) => tr!(
            "Respaldo restaurado. La base anterior quedó en {}",
            "Backup restored. The previous database is at {}",
            kept.display()
        ),
        Err(e) => tr!(
            "No se restauró el respaldo: {e}",
            "The backup wasn't restored: {e}"
        ),
    });
    let state = Arc::new(Mutex::new(loaded));

    // `--hidden` para arrancar con Windows sin mostrar la ventana.
    let hidden = std::env::args().any(|a| a == "--hidden");
    // Se dibuja por CPU (softbuffer + GDI): con OpenGL o Direct3D el driver de la
    // tarjeta de video se cargaba en el proceso y solo eso eran 120-350 MB.
    let config = SoftwareBackendAppConfiguration::new()
        // Sin caché de primitivas: en una ventana de este tamaño los cuadros tardan lo mismo
        // y la caché retenía ~4,5 MB (un búfer rasterizado por cada cosa dibujada).
        .caching(false)
        .viewport_builder(
            egui::ViewportBuilder::default()
                .with_title(win::WINDOW_TITLE)
                .with_inner_size([880.0, 540.0])
                .with_min_inner_size([520.0, 320.0])
                .with_icon(window_icon())
                .with_always_on_top()
                .with_visible(!hidden),
        );
    run_app_with_software_backend(config, move |ctx| {
        win::start(state.clone(), ctx.clone());
        app::PegaApp::new(state.clone())
    })
    .expect("no se pudo crear la ventana");
}
