//! Pega: gestor de portapapeles nativo para Windows. La librería la comparten la app con
//! ventana (`pega.exe`) y la de consola (`pega-cli.exe`).

pub mod app;
pub mod autostart;
pub mod backup;
pub mod cli;
pub mod code;
pub mod crypto;
pub mod export;
pub mod i18n;
pub mod image;
pub mod import;
pub mod lock;
pub mod mask;
pub mod ocr;
pub mod paths;
pub mod qr;
pub mod query;
pub mod settings;
pub mod settings_ui;
pub mod state;
pub mod store;
pub mod template;
pub mod transform;
pub mod win;
