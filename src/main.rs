// En release no abre consola.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod crypto;
mod image;
mod state;
mod store;
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

fn main() {
    if win::already_running() {
        win::signal_existing();
        return;
    }
    let dir = data_dir();
    std::fs::create_dir_all(&dir).expect("no se pudo crear %APPDATA%\\Pega");
    let store =
        store::Store::open(&dir.join("pega.db")).expect("no se pudo abrir la base de datos");
    let state = Arc::new(Mutex::new(
        state::State::load(store).expect("no se pudo leer la base de datos"),
    ));

    // `--hidden` para arrancar con Windows sin mostrar la ventana.
    let hidden = std::env::args().any(|a| a == "--hidden");
    // Se dibuja por CPU (softbuffer + GDI): con OpenGL o Direct3D el driver de la
    // tarjeta de video se cargaba en el proceso y solo eso eran 120-350 MB.
    let config = SoftwareBackendAppConfiguration::new().viewport_builder(
        egui::ViewportBuilder::default()
            .with_title(win::WINDOW_TITLE)
            .with_inner_size([880.0, 540.0])
            .with_min_inner_size([520.0, 320.0])
            .with_always_on_top()
            .with_visible(!hidden),
    );
    run_app_with_software_backend(config, move |ctx| {
        win::start(state.clone(), ctx.clone());
        app::PegaApp::new(state.clone())
    })
    .expect("no se pudo crear la ventana");
}
