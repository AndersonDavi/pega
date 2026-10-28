// En release no abre consola.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use egui_software_backend::{SoftwareBackendAppConfiguration, run_app_with_software_backend};
use pega::{app, autostart, backup, image, paths, shortcut, state, store, tr, win};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

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
    let dir = paths::data_dir();
    if let Err(e) = std::fs::create_dir_all(&dir) {
        win::fatal(&format!("No se pudo crear {}: {e}", dir.display()));
    }
    let restored = restore.map(|b| backup::apply_restore(&dir, &b));
    // Sin consola, un error al abrir la base cerraba la app sin decir nada: ahora se avisa.
    let db = dir.join("pega.db");
    let loaded = store::Store::open(&db).and_then(|store| {
        shortcut::refresh(&store);
        state::State::load(store)
    });
    // Al cargar se aplica el idioma de los ajustes; los avisos se arman después.
    let mut loaded = loaded.unwrap_or_else(|e| {
        win::fatal(&tr!(
            "No se pudo abrir la base de datos de Pega ({}): {e}\n\nSi se dañó, puedes restaurar un respaldo moviendo ese archivo y abriendo Pega de nuevo.",
            "Couldn't open Pega's database ({}): {e}\n\nIf it's damaged, move that file away and open Pega again to restore a backup.",
            db.display()
        ))
    });
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
    let accessibility = loaded.settings.accessibility;
    let state = Arc::new(Mutex::new(loaded));

    // `--hidden` para arrancar con Windows sin mostrar la ventana.
    let hidden = std::env::args().any(|a| a == "--hidden");
    // Se dibuja por CPU (softbuffer + GDI): con OpenGL o Direct3D el driver de la
    // tarjeta de video se cargaba en el proceso y solo eso eran 120-350 MB.
    let config = SoftwareBackendAppConfiguration::new()
        // Sin caché de primitivas: en una ventana de este tamaño los cuadros tardan lo mismo
        // y la caché retenía ~4,5 MB (un búfer rasterizado por cada cosa dibujada).
        .caching(false)
        // Lectores de pantalla: opcional (con AccessKit activo vimos ráfagas de redibujo).
        .accesskit(accessibility)
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
        if hidden {
            state.lock().unwrap().release_memory();
            win::trim_memory_after(std::time::Duration::from_secs(3));
        }
        app::PegaApp::new(state.clone())
    })
    .unwrap_or_else(|e| win::fatal(&format!("No se pudo crear la ventana: {e}")));
}
