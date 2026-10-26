//! Interfaz egui: historial a la izquierda, tableros a la derecha, búsqueda arriba.
//! egui solo redibuja cuando hay un evento, así que en reposo no gasta CPU.
//!
//! Clic = copiar (la ventana sigue abierta). Doble clic o Enter = pegar en la app anterior.

use crate::backup;
use crate::code;
use crate::export;
use crate::i18n::t;
use crate::image;
use crate::lock::Lock;
use crate::mask::mask;
use crate::qr;
use crate::query::Query;
use crate::settings::{Settings, Theme};
use crate::settings_ui::{Outcome, SettingsWindow};
use crate::state::{self, State};
use crate::template;
use crate::tr;
use crate::transform;
use crate::win;
use egui::{AtomExt, Key, Modifiers, RichText, TextWrapMode, ViewportCommand};
use egui_software_backend::SoftwareBackend;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Qué se copia: el texto ya está en memoria; la imagen se lee de disco al usarla.
enum Item {
    Text(String),
    Image(i64),
    /// Texto de un clip de tablero: si tiene variables o campos se completa antes de copiar.
    Template(String),
}

/// Cambios que se aplican después de dibujar, cuando ya no se está recorriendo el estado.
enum Action {
    Copy {
        item: Item,
        /// Clips de tablero: se marcan para que Windows (Win+V) ni Pega los guarden.
        private: bool,
        paste: bool,
    },
    TogglePin(i64),
    DeleteEntry(i64),
    ExcludeApp(String),
    ClearHistory,
    SaveToBoard {
        board_id: i64,
        text: String,
    },
    AddBoard(String),
    RenameBoard(i64, String),
    DeleteBoard(i64),
    SaveClip {
        id: Option<i64>,
        board_id: i64,
        name: String,
        text: String,
    },
    DeleteClip(i64),
    SetBoardColor(i64, Option<u32>),
    MoveBoard {
        id: i64,
        before: Option<i64>,
    },
    MoveClip {
        id: i64,
        board_id: i64,
        before: Option<i64>,
    },
    ImportPasteBar,
    SaveSettings(Settings),
    Lock,
    PasteSequence(Vec<(String, bool)>),
    ShowQr(String),
    StartExport,
    StartImport,
    Export(String),
    Import(PathBuf, String),
    Backup,
    PickRestore,
    Restore(PathBuf),
    Hide,
    Quit,
}

/// Ancho reservado a la derecha del título de un tablero para "+ Clip" y "…".
const BOARD_BUTTONS_W: f32 = 96.0;

/// Etiquetas y números de las filas: blancos sobre la fila seleccionada (azul) y un gris
/// legible en las demás (`weak` quedaba con muy poco contraste).
fn tag_color(ui: &egui::Ui, selected: bool, strength: f32) -> egui::Color32 {
    if selected {
        ui.visuals().selection.stroke.color.gamma_multiply(0.9)
    } else {
        ui.visuals().text_color().gamma_multiply(strength)
    }
}

/// Elemento de la selección múltiple (Ctrl+clic).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Sel {
    Hist(i64),
    Clip(i64),
}

/// Agrega o quita de la selección; guarda el texto para pegarlo en ese orden.
fn toggle_multi(multi: &mut Vec<(Sel, String, bool)>, sel: Sel, text: &str, private: bool) {
    match multi.iter().position(|(s, ..)| *s == sel) {
        Some(i) => {
            multi.remove(i);
        }
        None => multi.push((sel, text.to_owned(), private)),
    }
}

const QUICK_KEYS: [Key; 9] = [
    Key::Num1,
    Key::Num2,
    Key::Num3,
    Key::Num4,
    Key::Num5,
    Key::Num6,
    Key::Num7,
    Key::Num8,
    Key::Num9,
];

/// Lo que se arrastra: un clip o un tablero (por id).
struct DragClip(i64);
struct DragBoard(i64);

/// Colores para los tableros (0xRRGGBB).
const BOARD_COLORS: &[u32] = &[
    0xEF4444, 0xF97316, 0xEAB308, 0x22C55E, 0x14B8A6, 0x3B82F6, 0x8B5CF6, 0xEC4899,
];

/// Submenú "Pegar como…" con las transformaciones que aplican a este texto.
fn paste_as_menu(ui: &mut egui::Ui, text: &str, private: bool, actions: &mut Vec<Action>) {
    ui.menu_button(t("Pegar como…", "Paste as…"), |ui| {
        for tr in transform::ALL {
            if let Some(out) = tr.apply(text) {
                if ui.button(tr.label()).clicked() {
                    actions.push(Action::Copy {
                        item: Item::Text(out),
                        private,
                        paste: true,
                    });
                }
            }
        }
    });
}

/// Botón de acción que borra o reemplaza datos: rojo relleno con texto blanco.
fn danger_button(text: &str) -> egui::Button<'_> {
    egui::Button::new(RichText::new(text).color(egui::Color32::WHITE).strong())
        .fill(egui::Color32::from_rgb(0xDC, 0x26, 0x26))
}

fn rgb(c: u32) -> egui::Color32 {
    egui::Color32::from_rgb((c >> 16) as u8, (c >> 8) as u8, c as u8)
}

/// Contraseña para exportar (con confirmación) o para abrir una exportación (`import`).
#[derive(Default)]
struct PasswordDialog {
    import: Option<PathBuf>,
    pass: String,
    confirm: String,
    error: Option<String>,
    focus: bool,
}

#[allow(non_snake_case)]
fn EXPORT_FILTER() -> &'static str {
    t(
        "Exportación de Pega (*.pega)\0*.pega\0Todos los archivos\0*.*\0\0",
        "Pega export (*.pega)\0*.pega\0All files\0*.*\0\0",
    )
}

/// Campos de una plantilla que se piden antes de pegar.
struct TemplateForm {
    text: String,
    private: bool,
    paste: bool,
    values: Vec<(String, String)>,
    focus_first: bool,
}

/// Fecha, hora y el texto que hay ahora en el portapapeles.
fn template_vars() -> template::Vars {
    template::Vars::now(win::current_text().unwrap_or_default())
}

struct Editor {
    id: Option<i64>,
    board_id: i64,
    name: String,
    text: String,
}

pub struct PegaApp {
    state: Arc<Mutex<State>>,
    query: String,
    selected: usize,
    scroll_to: Option<usize>,
    new_board: Option<String>,
    renaming: Option<(i64, String)>,
    editor: Option<Editor>,
    confirm_clear: bool,
    quitting: bool,
    /// Miniaturas ya subidas a egui, por id de entrada.
    thumbs: HashMap<i64, egui::TextureHandle>,
    /// Aviso corto (t("Copiado", "Copied")) y hasta cuándo se muestra.
    flash: Option<(String, f64)>,
    settings_window: Option<SettingsWindow>,
    /// Tema aplicado a egui; se reaplica solo si cambia en los ajustes.
    applied_theme: Option<Theme>,
    lock: Lock,
    styled: bool,
    /// Respaldo elegido, esperando confirmación para restaurar.
    confirm_restore: Option<PathBuf>,
    template_form: Option<TemplateForm>,
    /// Selección múltiple con Ctrl+clic, en el orden en que se eligió.
    multi: Vec<(Sel, String, bool)>,
    /// QR abierto; la textura se suelta al cerrarlo.
    qr: Option<egui::TextureHandle>,
    password: Option<PasswordDialog>,
}

/// Espaciado, esquinas y el azul del ícono como acento, para ambos temas.
fn apply_style(ctx: &egui::Context) {
    ctx.all_styles_mut(|s| {
        s.spacing.item_spacing = egui::vec2(8.0, 6.0);
        s.spacing.button_padding = egui::vec2(8.0, 4.0);
        s.visuals.window_corner_radius = 10.into();
        s.visuals.menu_corner_radius = 8.into();
        let w = &mut s.visuals.widgets;
        for v in [
            &mut w.noninteractive,
            &mut w.inactive,
            &mut w.hovered,
            &mut w.active,
            &mut w.open,
        ] {
            v.corner_radius = 6.into();
        }
        s.visuals.selection.bg_fill = egui::Color32::from_rgb(0x25, 0x63, 0xEB);
        s.visuals.selection.stroke.color = egui::Color32::WHITE;
        // Cursor fijo: el parpadeo obligaba a redibujar ~4 veces por segundo con la ventana abierta.
        s.visuals.text_cursor.blink = false;
    });
}

impl PegaApp {
    pub fn new(state: Arc<Mutex<State>>) -> Self {
        let lock = Lock::new(&state.lock().unwrap().settings);
        Self {
            lock,
            styled: false,
            confirm_restore: None,
            template_form: None,
            multi: Vec::new(),
            qr: None,
            password: None,
            state,
            query: String::new(),
            selected: 0,
            scroll_to: None,
            new_board: None,
            renaming: None,
            editor: None,
            confirm_clear: false,
            quitting: false,
            thumbs: HashMap::new(),
            flash: None,
            settings_window: None,
            applied_theme: None,
        }
    }

    fn thumb(&mut self, ctx: &egui::Context, e: &state::Entry) -> Option<egui::TextureHandle> {
        if let Some(t) = self.thumbs.get(&e.id) {
            return Some(t.clone());
        }
        let img = image::decode_png(&e.image.as_ref()?.thumb_png)?;
        let color = egui::ColorImage::from_rgba_unmultiplied(
            [img.width as usize, img.height as usize],
            &img.pixels,
        );
        let tex = ctx.load_texture(
            format!("thumb{}", e.id),
            color,
            egui::TextureOptions::LINEAR,
        );
        self.thumbs.insert(e.id, tex.clone());
        Some(tex)
    }
}

/// Primera línea, recortada, para la lista.
fn preview(text: &str) -> String {
    let trimmed = text.trim();
    let first = trimmed.lines().next().unwrap_or("");
    let mut out: String = first.chars().take(120).collect();
    let lines = trimmed.lines().count();
    if lines > 1 {
        out.push_str(&tr!("  (+{} líneas)", "  (+{} lines)", lines - 1));
    } else if first.chars().count() > 120 {
        out.push('…');
    }
    out
}

/// "Desde chrome.exe · hace 5 min".
fn origin(app: Option<&str>, used_at: i64) -> String {
    match app {
        Some(app) => tr!("Desde {app} · {}", "From {app} · {}", ago(used_at)),
        None => ago(used_at),
    }
}

/// Texto del tooltip: de dónde y cuándo vino, y el contenido (recortado).
fn tooltip(text: &str, app: Option<&str>, used_at: i64) -> String {
    let mut t = origin(app, used_at);
    if !text.is_empty() {
        t.push_str("\n\n");
        t.extend(text.chars().take(600));
        if text.chars().count() > 600 {
            t.push('…');
        }
    }
    t
}

/// "hace 5 min", a partir de milisegundos Unix.
fn ago(ms: i64) -> String {
    let s = ((state::now() - ms) / 1000).max(0);
    match s {
        0..60 => t("hace un momento", "just now").into(),
        60..3600 => tr!("hace {} min", "{} min ago", s / 60),
        3600..86400 => tr!("hace {} h", "{} h ago", s / 3600),
        _ => tr!("hace {} días", "{} days ago", s / 86400),
    }
}

/// "chrome.exe" -> "chrome", para la columna de la derecha.
fn short_app(app: &str) -> &str {
    app.strip_suffix(".exe").unwrap_or(app)
}

fn item_of(e: &state::Entry) -> Item {
    match e.image {
        Some(_) => Item::Image(e.id),
        None => Item::Text(e.text.clone()),
    }
}

impl egui_software_backend::App for PegaApp {
    fn ui(&mut self, ui: &mut egui::Ui, _backend: &mut SoftwareBackend) {
        let ctx = ui.ctx().clone();
        let mut actions: Vec<Action> = Vec::new();
        let state = self.state.clone();
        let mut st = state.lock().unwrap();
        if let Some(notice) = st.notice.take() {
            self.flash(&notice, 8.0, &ctx);
        }

        // Cerrar la ventana la oculta (sigue escuchando); t("Salir", "Quit") la cierra de verdad.
        // Sin atajo global no habría forma de volver a abrirla, así que ahí sí se cierra.
        if ctx.input(|i| i.viewport().close_requested())
            && !self.quitting
            && win::can_reopen(st.hotkey_ok)
        {
            ctx.send_viewport_cmd(ViewportCommand::CancelClose);
            actions.push(Action::Hide);
        }

        if !self.styled {
            apply_style(&ctx);
            self.styled = true;
        }
        if self.applied_theme != Some(st.settings.theme) {
            ctx.set_theme(match st.settings.theme {
                Theme::System => egui::ThemePreference::System,
                Theme::Dark => egui::ThemePreference::Dark,
                Theme::Light => egui::ThemePreference::Light,
            });
            self.applied_theme = Some(st.settings.theme);
        }
        // Bloqueado: solo la pantalla del PIN, sin dibujar nada del contenido.
        if self.lock.update(&ctx, &st.settings) {
            if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
                actions.push(Action::Hide);
            }
            self.lock.show(ui, &st.settings);
            drop(st);
            self.apply(&ctx, actions);
            return;
        }
        let hotkey = st.settings.hotkey.label();
        let masked = |t: &str| mask(t, &st.settings.mask_words, st.settings.mask_cards);
        let modal_open = self.editor.is_some()
            || self.settings_window.is_some()
            || self.template_form.is_some()
            || self.qr.is_some()
            || self.password.is_some();

        let just_shown = win::take_shown();
        if just_shown {
            self.query.clear();
            self.selected = 0;
            self.scroll_to = Some(0);
        }

        let query = Query::parse(&self.query);
        let now = state::now();
        // Fijados primero, luego por uso reciente.
        let mut hist: Vec<&state::Entry> = st
            .history
            .iter()
            .filter(|e| query.matches_entry(e, st.app_name(e.app), now))
            .collect();
        hist.sort_by_key(|e| !e.pinned);
        if self.selected >= hist.len() {
            self.selected = hist.len().saturating_sub(1);
        }
        // Miniaturas de entradas que ya no existen.
        if self.thumbs.len() > st.history.len() {
            self.thumbs
                .retain(|id, _| st.history.iter().any(|e| e.id == *id));
        }

        // Teclado: flechas para moverse, Enter pega, Shift+Enter copia, Esc limpia/oculta.
        if !modal_open {
            let (down, up, enter, shift_enter, esc) = ctx.input_mut(|i| {
                (
                    i.consume_key(Modifiers::NONE, Key::ArrowDown),
                    i.consume_key(Modifiers::NONE, Key::ArrowUp),
                    i.consume_key(Modifiers::NONE, Key::Enter),
                    i.consume_key(Modifiers::SHIFT, Key::Enter),
                    i.consume_key(Modifiers::NONE, Key::Escape),
                )
            });
            if down && self.selected + 1 < hist.len() {
                self.selected += 1;
                self.scroll_to = Some(self.selected);
            }
            if up && self.selected > 0 {
                self.selected -= 1;
                self.scroll_to = Some(self.selected);
            }
            if let Some(e) = hist.get(self.selected) {
                if enter || shift_enter {
                    actions.push(Action::Copy {
                        item: item_of(e),
                        private: false,
                        paste: enter,
                    });
                }
            }
            // Ctrl+1…9: pega directo esa entrada del historial (la fila muestra su número).
            let quick = ctx.input_mut(|i| {
                QUICK_KEYS
                    .iter()
                    .position(|k| i.consume_key(Modifiers::CTRL, *k))
            });
            if let Some(e) = quick.and_then(|n| hist.get(n)) {
                actions.push(Action::Copy {
                    item: item_of(e),
                    private: false,
                    paste: true,
                });
            }
            if esc {
                if !self.multi.is_empty() {
                    self.multi.clear();
                } else if self.query.is_empty() {
                    actions.push(Action::Hide);
                } else {
                    self.query.clear();
                }
            }
        }

        egui::Panel::top("top").show_inside(ui, |ui| {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.menu_button("…", |ui| {
                    let mut autostart = crate::autostart::is_enabled();
                    if ui
                        .checkbox(
                            &mut autostart,
                            t("Iniciar con Windows", "Start with Windows"),
                        )
                        .changed()
                    {
                        crate::autostart::set(autostart);
                    }
                    if ui
                        .button(t("Importar de PasteBar", "Import from PasteBar"))
                        .clicked()
                    {
                        actions.push(Action::ImportPasteBar);
                    }
                    if !st.settings.pin.is_empty()
                        && ui.button(t("Bloquear ahora", "Lock now")).clicked()
                    {
                        actions.push(Action::Lock);
                    }
                    if ui.button(t("Crear respaldo…", "Create backup…")).clicked() {
                        actions.push(Action::Backup);
                    }
                    if ui
                        .button(t("Restaurar respaldo…", "Restore backup…"))
                        .clicked()
                    {
                        actions.push(Action::PickRestore);
                    }
                    if ui
                        .button(t("Exportar con contraseña…", "Export with password…"))
                        .on_hover_text(t(
                            "Para otro PC u otro usuario: el respaldo normal solo sirve con tu usuario de Windows.",
                            "For another PC or user: normal backups only work with your Windows user.",
                        ))
                        .clicked()
                    {
                        actions.push(Action::StartExport);
                    }
                    if ui.button(t("Importar exportación…", "Import export…")).clicked() {
                        actions.push(Action::StartImport);
                    }
                    if ui.button(t("Ajustes…", "Settings…")).clicked() {
                        self.settings_window = Some(SettingsWindow::new(&st.settings));
                    }
                    ui.separator();
                    if ui
                        .button(t("Salir", "Quit"))
                        .on_hover_text(t("Cierra Pega por completo", "Closes Pega completely"))
                        .clicked()
                    {
                        actions.push(Action::Quit);
                    }
                });
                let now = ctx.input(|i| i.time);
                match &self.flash {
                    Some((msg, until)) if *until > now => {
                        ui.label(RichText::new(msg).color(ui.visuals().selection.bg_fill));
                    }
                    _ => self.flash = None,
                }
                let hint = tr!(
                    "Buscar en historial y tableros…   ({hotkey} abre/oculta)",
                    "Search history and boards…   ({hotkey} shows/hides)"
                );
                let search = ui.add(
                    egui::TextEdit::singleline(&mut self.query)
                        .hint_text(hint)
                        .desired_width(f32::INFINITY),
                );
                if just_shown || ctx.memory(|m| m.focused().is_none()) && !modal_open {
                    search.request_focus();
                }
                if search.changed() {
                    self.selected = 0;
                    self.scroll_to = Some(0);
                }
            });
            if !st.hotkey_ok {
                ui.colored_label(
                    ui.visuals().warn_fg_color,
                    tr!(
                        "No se pudo registrar {hotkey}: otra app lo usa. Cámbialo en Ajustes.",
                        "Couldn't register {hotkey}: another app uses it. Change it in Settings."
                    ),
                );
            }
            if !st.plain_hotkey_ok {
                ui.colored_label(
                    ui.visuals().warn_fg_color,
                    t(
                        "El atajo de pegar sin formato está ocupado por otra app. Cámbialo en Ajustes.",
                        "The paste-as-plain-text shortcut is taken by another app. Change it in Settings.",
                    ),
                );
            }
            if !self.multi.is_empty() {
                ui.horizontal(|ui| {
                    ui.strong(tr!("{} seleccionados", "{} selected", self.multi.len()));
                    if ui
                        .button(t("Pegar en secuencia (Tab entre cada uno)", "Paste in sequence (Tab between)"))
                        .clicked()
                    {
                        let items = self.multi.iter().map(|(_, text, private)| (text.clone(), *private)).collect();
                        actions.push(Action::PasteSequence(items));
                    }
                    if ui.button(t("Pegar unidos", "Paste joined")).clicked() {
                        let text = self.multi.iter().map(|(_, t, _)| t.as_str()).collect::<Vec<_>>().join("\n");
                        let private = self.multi.iter().any(|(_, _, p)| *p);
                        actions.push(Action::Copy { item: Item::Text(text), private, paste: true });
                        self.multi.clear();
                    }
                    if ui.button(t("Quitar selección", "Clear selection")).clicked() {
                        self.multi.clear();
                    }
                });
            }
            if let Some(err) = &st.error {
                ui.colored_label(ui.visuals().error_fg_color, err);
            }
            ui.add_space(4.0);
        });

        // Los tableros siempre conservan al menos 280 px, aunque se agrande el historial.
        let max_history = (ui.available_width() - 280.0).max(220.0);
        egui::Panel::left("history")
            .resizable(true)
            .default_size(340.0_f32.min(max_history))
            .size_range(220.0..=max_history)
            .show_inside(ui, |ui| {
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.strong(tr!("Historial ({})", "History ({})", st.history.len()));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if self.confirm_clear {
                            if ui.button(t("Cancelar", "Cancel")).clicked() {
                                self.confirm_clear = false;
                            }
                            if ui
                                .add(danger_button(t("Sí, borrar", "Yes, delete")))
                                .clicked()
                            {
                                actions.push(Action::ClearHistory);
                                self.confirm_clear = false;
                            }
                        } else if ui
                            .small_button(t("Limpiar", "Clear"))
                            .on_hover_text(t(
                                "Borra todo menos lo fijado",
                                "Deletes everything except pinned",
                            ))
                            .clicked()
                        {
                            self.confirm_clear = true;
                        }
                    });
                });
                ui.separator();
                ui.style_mut().wrap_mode = Some(TextWrapMode::Truncate);

                let row_h = ui.spacing().interact_size.y;
                let scroll_to = self.scroll_to.take();
                egui::ScrollArea::vertical()
                    .id_salt("hist")
                    .auto_shrink(false)
                    .show(ui, |ui| {
                        // Filas a todo el ancho con el texto a la izquierda.
                        ui.with_layout(egui::Layout::top_down_justified(egui::Align::LEFT), |ui| {
                            for (i, e) in hist.iter().enumerate() {
                                let in_multi =
                                    self.multi.iter().any(|(s, ..)| *s == Sel::Hist(e.id));
                                let selected = i == self.selected || in_multi;
                                // Número de Ctrl+1…9 en las primeras filas.
                                let num = egui::RichText::new(if i < 9 {
                                    format!("{}", i + 1)
                                } else {
                                    " ".into()
                                })
                                .small()
                                .color(tag_color(ui, selected, 0.55));
                                let pin = if e.pinned { "📌 " } else { "" };
                                let app = st.app_name(e.app);
                                // "rust · chrome": lenguaje y app de origen.
                                let expires = st.expiry_of(e);
                                let tags: Vec<&str> = expires
                                    .map(|_| t("temporal", "temporary"))
                                    .into_iter()
                                    .chain(e.lang.map(code::Lang::name))
                                    .chain(app.map(short_app))
                                    .collect();
                                let right = egui::RichText::new(tags.join(" · "))
                                    .small()
                                    .color(tag_color(ui, selected, 0.7));
                                let resp = match &e.image {
                                    Some(img) => {
                                        let label = tr!(
                                            "{pin}Imagen {}×{}",
                                            "{pin}Image {}×{}",
                                            img.width,
                                            img.height
                                        );
                                        match self.thumb(&ctx, e) {
                                            Some(tex) => ui.add(
                                                egui::Button::selectable(
                                                    selected,
                                                    (
                                                        num,
                                                        egui::Image::new(&tex),
                                                        label.atom_shrink(true),
                                                    ),
                                                )
                                                .right_text(right),
                                            ),
                                            None => ui.add(
                                                egui::Button::selectable(
                                                    selected,
                                                    (num, label.atom_shrink(true)),
                                                )
                                                .right_text(right),
                                            ),
                                        }
                                    }
                                    None => ui.add(
                                        egui::Button::selectable(
                                            selected,
                                            // El texto se recorta con "…": si no, una URL larga ensancha el panel.
                                            (
                                                num,
                                                format!("{pin}{}", preview(&masked(&e.text)))
                                                    .atom_shrink(true),
                                            ),
                                        )
                                        .right_text(right)
                                        .min_size(egui::vec2(0.0, row_h)),
                                    ),
                                }
                                .on_hover_ui(|ui| {
                                    let text = masked(&e.text);
                                    match e.lang {
                                        Some(lang) => {
                                            ui.label(
                                                egui::RichText::new(origin(app, e.used_at)).weak(),
                                            );
                                            ui.label(code::highlight(
                                                &text,
                                                lang,
                                                ui.visuals().dark_mode,
                                            ));
                                        }
                                        None => {
                                            ui.label(tooltip(&text, app, e.used_at));
                                        }
                                    }
                                    if let Some(at) = expires {
                                        let mins = ((at - now) / 60_000).max(0) + 1;
                                        ui.colored_label(
                                            ui.visuals().warn_fg_color,
                                            tr!(
                                                "Se borra sola en {mins} min",
                                                "Deletes itself in {mins} min"
                                            ),
                                        );
                                    }
                                });
                                if scroll_to == Some(i) {
                                    resp.scroll_to_me(None);
                                }
                                let ctrl = ui.input(|inp| inp.modifiers.ctrl);
                                if resp.clicked() && ctrl {
                                    // Ctrl+clic: selección múltiple (solo texto).
                                    if e.image.is_none() {
                                        toggle_multi(
                                            &mut self.multi,
                                            Sel::Hist(e.id),
                                            &e.text,
                                            false,
                                        );
                                    }
                                } else if resp.double_clicked() {
                                    actions.push(Action::Copy {
                                        item: item_of(e),
                                        private: false,
                                        paste: true,
                                    });
                                } else if resp.clicked() {
                                    self.selected = i;
                                    actions.push(Action::Copy {
                                        item: item_of(e),
                                        private: false,
                                        paste: false,
                                    });
                                }
                                resp.context_menu(|ui| {
                                    if e.image.is_none() {
                                        paste_as_menu(ui, &e.text, false, &mut actions);
                                        if e.text.len() <= qr::MAX_BYTES
                                            && ui.button(t("Mostrar QR", "Show QR")).clicked()
                                        {
                                            actions.push(Action::ShowQr(e.text.clone()));
                                        }
                                    }
                                    if ui.button(t("Pegar", "Paste")).clicked() {
                                        actions.push(Action::Copy {
                                            item: item_of(e),
                                            private: false,
                                            paste: true,
                                        });
                                    }
                                    if ui
                                        .button(if e.pinned {
                                            t("Desfijar", "Unpin")
                                        } else {
                                            t("Fijar", "Pin")
                                        })
                                        .clicked()
                                    {
                                        actions.push(Action::TogglePin(e.id));
                                    }
                                    if e.image.is_none() && !st.boards.is_empty() {
                                        ui.menu_button(
                                            t("Guardar en tablero", "Save to board"),
                                            |ui| {
                                                for b in &st.boards {
                                                    if ui.button(&b.name).clicked() {
                                                        actions.push(Action::SaveToBoard {
                                                            board_id: b.id,
                                                            text: e.text.clone(),
                                                        });
                                                    }
                                                }
                                            },
                                        );
                                    }
                                    ui.separator();
                                    if let Some(app) = app {
                                        if ui
                                            .button(tr!(
                                                "No guardar copias de {app}",
                                                "Don't save copies from {app}"
                                            ))
                                            .clicked()
                                        {
                                            actions.push(Action::ExcludeApp(app.to_owned()));
                                        }
                                    }
                                    if ui.button(t("Borrar", "Delete")).clicked() {
                                        actions.push(Action::DeleteEntry(e.id));
                                    }
                                });
                            }
                        });
                        if hist.is_empty() {
                            ui.weak(if self.query.trim().is_empty() {
                                t(
                                    "Copia algo y aparecerá aquí.",
                                    "Copy something and it will show up here.",
                                )
                            } else {
                                t("Sin resultados.", "No results.")
                            });
                        }
                    });
            });

        egui::CentralPanel::default().show_inside(ui, |ui| {
            ui.horizontal(|ui| {
                ui.strong(t("Tableros", "Boards"));
                match &mut self.new_board {
                    Some(name) => {
                        let r = ui.add(
                            egui::TextEdit::singleline(name)
                                .hint_text(t("Nombre del tablero", "Board name"))
                                .desired_width(180.0),
                        );
                        r.request_focus();
                        let enter = r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                        if (ui.button(t("Crear", "Create")).clicked() || enter) && !name.trim().is_empty() {
                            actions.push(Action::AddBoard(name.trim().to_owned()));
                            self.new_board = None;
                        } else if ui.button(t("Cancelar", "Cancel")).clicked() {
                            self.new_board = None;
                        }
                    }
                    None => {
                        if ui.button(t("+ Tablero", "+ Board")).clicked() {
                            self.new_board = Some(String::new());
                        }
                    }
                }
            });
            ui.separator();
            egui::ScrollArea::vertical().id_salt("boards").auto_shrink(false).show(ui, |ui| {
                if st.boards.is_empty() {
                    ui.weak(t("Crea un tablero para guardar textos fijos (datos, plantillas, respuestas…).", "Create a board to keep fixed texts (details, templates, replies…)."));
                }
                let dragging_clip = egui::DragAndDrop::has_payload_of_type::<DragClip>(&ctx);
                let dragging_board = egui::DragAndDrop::has_payload_of_type::<DragBoard>(&ctx);
                for b in &st.boards {
                    // Filtros de historial (app:, tipo:, hace:…): los tableros no aplican.
                    if query.has_history_filters() {
                        break;
                    }
                    let board_hit = query.matches_text(&state::fold(&b.name));
                    let clips: Vec<&state::Clip> = st
                        .clips
                        .iter()
                        .filter(|c| c.board_id == b.id && (board_hit || query.matches_text(&c.key)))
                        .collect();
                    if !query.terms.is_empty() && clips.is_empty() && !board_hit {
                        continue;
                    }
                    let accent = b.color.map(rgb);
                    let mut frame = egui::Frame::group(ui.style()).corner_radius(8.0);
                    if let Some(color) = accent {
                        frame = frame
                            .stroke(egui::Stroke::new(1.5, color))
                            .fill(color.gamma_multiply(0.08));
                    }
                    let board_resp = frame.show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            let editing = matches!(&self.renaming, Some((id, _)) if *id == b.id);
                            if editing {
                                let (id, name) = self.renaming.as_mut().unwrap();
                                let width = (ui.available_width() - BOARD_BUTTONS_W).clamp(60.0, 220.0);
                                let r = ui.add(egui::TextEdit::singleline(name).desired_width(width));
                                r.request_focus();
                                if r.lost_focus() {
                                    if !name.trim().is_empty() {
                                        actions.push(Action::RenameBoard(*id, name.trim().to_owned()));
                                    }
                                    self.renaming = None;
                                }
                            } else {
                                // El título es el asa para arrastrar el tablero.
                                let title = egui::RichText::new(&b.name).strong();
                                let title = match accent {
                                    Some(color) => title.color(color),
                                    None => title,
                                };
                                // Se recorta con "…" para dejar siempre lugar a "+ Clip" y al menú.
                                let room = (ui.available_width() - BOARD_BUTTONS_W).max(40.0);
                                ui.scope(|ui| {
                                    ui.set_max_width(room);
                                    let handle = ui
                                        .add(egui::Label::new(title).truncate().sense(egui::Sense::drag()))
                                        .on_hover_cursor(egui::CursorIcon::Grab)
                                        .on_hover_text(tr!(
                                            "{} · arrastra para reordenar",
                                            "{} · drag to reorder",
                                            b.name
                                        ));
                                    handle.dnd_set_drag_payload(DragBoard(b.id));
                                });
                            }
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                ui.menu_button("…", |ui| {
                                    if ui.button(t("Renombrar", "Rename")).clicked() {
                                        self.renaming = Some((b.id, b.name.clone()));
                                    }
                                    ui.label("Color");
                                    ui.horizontal(|ui| {
                                        for &color in BOARD_COLORS {
                                            let (rect, swatch) =
                                                ui.allocate_exact_size(egui::vec2(18.0, 18.0), egui::Sense::click());
                                            ui.painter().rect_filled(rect, 4.0, rgb(color));
                                            if b.color == Some(color) {
                                                ui.painter().rect_stroke(
                                                    rect,
                                                    4.0,
                                                    egui::Stroke::new(2.0, ui.visuals().strong_text_color()),
                                                    egui::StrokeKind::Outside,
                                                );
                                            }
                                            if swatch.clicked() {
                                                actions.push(Action::SetBoardColor(b.id, Some(color)));
                                            }
                                        }
                                    });
                                    if b.color.is_some() && ui.button(t("Sin color", "No color")).clicked() {
                                        actions.push(Action::SetBoardColor(b.id, None));
                                    }
                                    ui.separator();
                                    if ui.button(t("Borrar tablero", "Delete board")).clicked() {
                                        actions.push(Action::DeleteBoard(b.id));
                                    }
                                });
                                if ui.small_button("+ Clip").clicked() {
                                    self.editor = Some(Editor {
                                        id: None,
                                        board_id: b.id,
                                        name: String::new(),
                                        text: String::new(),
                                    });
                                }
                            });
                        });
                        ui.horizontal_wrapped(|ui| {
                            for c in &clips {
                                let mut chip = egui::Button::new(&c.name).corner_radius(12.0);
                                if let Some(color) = accent {
                                    chip = chip.fill(color.gamma_multiply(0.22));
                                }
                                if self.multi.iter().any(|(s, ..)| *s == Sel::Clip(c.id)) {
                                    chip = chip
                                        .fill(ui.visuals().selection.bg_fill)
                                        .stroke(egui::Stroke::new(1.5, egui::Color32::WHITE));
                                }
                                let resp = ui
                                    .add(chip)
                                    .interact(egui::Sense::drag())
                                    .on_hover_text(t("Clic: copiar · Doble clic: pegar · Arrastra para mover", "Click: copy · Double-click: paste · Drag to move"));
                                resp.dnd_set_drag_payload(DragClip(c.id));
                                // Soltar un clip sobre otro lo pone antes de ese.
                                if dragging_clip && resp.dnd_hover_payload::<DragClip>().is_some() {
                                    let r = resp.rect;
                                    ui.painter().vline(
                                        r.left() - 3.0,
                                        r.y_range(),
                                        egui::Stroke::new(2.0, ui.visuals().selection.bg_fill),
                                    );
                                }
                                if let Some(p) = resp.dnd_release_payload::<DragClip>() {
                                    actions.push(Action::MoveClip { id: p.0, board_id: b.id, before: Some(c.id) });
                                }
                                if resp.clicked() && ui.input(|inp| inp.modifiers.ctrl) {
                                    toggle_multi(&mut self.multi, Sel::Clip(c.id), &c.text, true);
                                } else if resp.double_clicked() {
                                    actions.push(Action::Copy {
                                        item: Item::Template(c.text.clone()),
                                        private: true,
                                        paste: true,
                                    });
                                } else if resp.clicked() {
                                    actions.push(Action::Copy {
                                        item: Item::Template(c.text.clone()),
                                        private: true,
                                        paste: false,
                                    });
                                }
                                resp.context_menu(|ui| {
                                    paste_as_menu(ui, &c.text, true, &mut actions);
                                    if c.text.len() <= qr::MAX_BYTES && ui.button(t("Mostrar QR", "Show QR")).clicked() {
                                        actions.push(Action::ShowQr(c.text.clone()));
                                    }
                                    if ui.button(t("Editar", "Edit")).clicked() {
                                        self.editor = Some(Editor {
                                            id: Some(c.id),
                                            board_id: c.board_id,
                                            name: c.name.clone(),
                                            text: c.text.clone(),
                                        });
                                    }
                                    if ui.button(t("Borrar", "Delete")).clicked() {
                                        actions.push(Action::DeleteClip(c.id));
                                    }
                                });
                            }
                            if clips.is_empty() {
                                ui.weak(if dragging_clip { t("Suelta aquí", "Drop here") } else { t("Sin clips", "No clips") });
                            }
                        });
                    });
                    // Soltar en el espacio libre del tablero: clip al final, o tablero antes de este.
                    let area = &board_resp.response;
                    if (dragging_clip || dragging_board) && area.contains_pointer() {
                        ui.painter().rect_stroke(
                            area.rect,
                            8.0,
                            egui::Stroke::new(1.5, ui.visuals().selection.bg_fill),
                            egui::StrokeKind::Inside,
                        );
                    }
                    if let Some(p) = area.dnd_release_payload::<DragClip>() {
                        actions.push(Action::MoveClip { id: p.0, board_id: b.id, before: None });
                    }
                    if let Some(p) = area.dnd_release_payload::<DragBoard>() {
                        actions.push(Action::MoveBoard { id: p.0, before: Some(b.id) });
                    }
                    ui.add_space(6.0);
                }
                // Debajo del último tablero: soltar un tablero lo manda al final.
                let rest = ui.allocate_response(ui.available_size().max(egui::vec2(0.0, 40.0)), egui::Sense::hover());
                if let Some(p) = rest.dnd_release_payload::<DragBoard>() {
                    actions.push(Action::MoveBoard { id: p.0, before: None });
                }
            });
        });
        if let Some(ed) = &mut self.editor {
            let mut open = true;
            let mut done = false;
            egui::Window::new(if ed.id.is_some() {
                t("Editar clip", "Edit clip")
            } else {
                t("Nuevo clip", "New clip")
            })
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .open(&mut open)
            .show(&ctx, |ui| {
                ui.label(t("Nombre", "Name"));
                ui.add(egui::TextEdit::singleline(&mut ed.name).desired_width(360.0));
                ui.label(t(
                    "Contenido (se guarda cifrado)",
                    "Content (stored encrypted)",
                ));
                ui.add(
                    egui::TextEdit::multiline(&mut ed.text)
                        .desired_width(360.0)
                        .desired_rows(6),
                );
                ui.weak(t(
                    "Variables: {fecha} {fecha_iso} {hora} {portapapeles} {campo:Nombre}",
                    "Variables: {date} {date_iso} {time} {clipboard} {field:Name}",
                ));
                ui.horizontal(|ui| {
                    let valid = !ed.name.trim().is_empty() && !ed.text.is_empty();
                    if ui
                        .add_enabled(valid, egui::Button::new(t("Guardar", "Save")))
                        .clicked()
                    {
                        actions.push(Action::SaveClip {
                            id: ed.id,
                            board_id: ed.board_id,
                            name: ed.name.trim().to_owned(),
                            text: ed.text.clone(),
                        });
                        done = true;
                    }
                    if ui.button(t("Cancelar", "Cancel")).clicked() {
                        done = true;
                    }
                });
            });
            if done || !open {
                self.editor = None;
            }
        }

        if let Some(dlg) = &mut self.password {
            let mut close = false;
            let title = match dlg.import {
                None => t("Exportar con contraseña", "Export with password"),
                Some(_) => t("Contraseña de la exportación", "Export password"),
            };
            egui::Window::new(title)
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(&ctx, |ui| {
                    let first = ui.add(
                        egui::TextEdit::singleline(&mut dlg.pass)
                            .password(true)
                            .hint_text(t("Contraseña (mín. 8)", "Password (min. 8)"))
                            .desired_width(260.0),
                    );
                    if dlg.focus {
                        first.request_focus();
                        dlg.focus = false;
                    }
                    if dlg.import.is_none() {
                        ui.add(
                            egui::TextEdit::singleline(&mut dlg.confirm)
                                .password(true)
                                .hint_text(t("Repetir contraseña", "Repeat password"))
                                .desired_width(260.0),
                        );
                        ui.weak(t(
                            "Si la olvidas, no hay forma de recuperar la exportación.",
                            "If you forget it, the export can't be recovered.",
                        ));
                    }
                    if let Some(e) = &dlg.error {
                        ui.colored_label(ui.visuals().error_fg_color, e);
                    }
                    let (enter, esc) =
                        ui.input(|i| (i.key_pressed(Key::Enter), i.key_pressed(Key::Escape)));
                    ui.horizontal(|ui| {
                        let ok_label = if dlg.import.is_some() {
                            t("Importar", "Import")
                        } else {
                            t("Exportar…", "Export…")
                        };
                        if ui.button(ok_label).clicked() || enter {
                            match &dlg.import {
                                Some(path) => {
                                    actions.push(Action::Import(
                                        path.clone(),
                                        std::mem::take(&mut dlg.pass),
                                    ));
                                    close = true;
                                }
                                None if dlg.pass.chars().count() < 8 => {
                                    dlg.error = Some(
                                        t(
                                            "Usa al menos 8 caracteres.",
                                            "Use at least 8 characters.",
                                        )
                                        .into(),
                                    );
                                }
                                None if dlg.pass != dlg.confirm => {
                                    dlg.error = Some(
                                        t(
                                            "Las contraseñas no coinciden.",
                                            "Passwords don't match.",
                                        )
                                        .into(),
                                    );
                                }
                                None => {
                                    actions.push(Action::Export(std::mem::take(&mut dlg.pass)));
                                    close = true;
                                }
                            }
                        }
                        if ui.button(t("Cancelar", "Cancel")).clicked() || esc {
                            close = true;
                        }
                    });
                });
            if close {
                self.password = None;
            }
        }

        if let Some(tex) = &self.qr {
            let mut close = false;
            egui::Window::new(t("QR para el celular", "QR for your phone"))
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(&ctx, |ui| {
                    ui.vertical_centered(|ui| {
                        ui.image((tex.id(), tex.size_vec2()));
                        ui.label(t(
                            "Escanéalo con la cámara del celular.",
                            "Scan it with your phone camera.",
                        ));
                        ui.colored_label(
                            ui.visuals().warn_fg_color,
                            t(
                                "Cualquiera que vea tu pantalla puede leerlo.",
                                "Anyone who can see your screen can read it.",
                            ),
                        );
                        close = ui.button(t("Cerrar", "Close")).clicked()
                            || ui.input(|i| i.key_pressed(Key::Escape));
                    });
                });
            if close {
                self.qr = None;
            }
        }

        if let Some(form) = &mut self.template_form {
            let mut close = false;
            let mut submit = false;
            egui::Window::new(t("Completar plantilla", "Fill in template"))
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(&ctx, |ui| {
                    egui::Grid::new("campos")
                        .num_columns(2)
                        .spacing([12.0, 8.0])
                        .show(ui, |ui| {
                            for (i, (name, value)) in form.values.iter_mut().enumerate() {
                                ui.label(name.as_str());
                                let r =
                                    ui.add(egui::TextEdit::singleline(value).desired_width(260.0));
                                if i == 0 && form.focus_first {
                                    r.request_focus();
                                    form.focus_first = false;
                                }
                                ui.end_row();
                            }
                        });
                    let (enter, esc) =
                        ui.input(|i| (i.key_pressed(Key::Enter), i.key_pressed(Key::Escape)));
                    ui.horizontal(|ui| {
                        let label = if form.paste {
                            t("Pegar", "Paste")
                        } else {
                            t("Copiar", "Copy")
                        };
                        submit = ui.button(label).clicked() || enter;
                        close = ui.button(t("Cancelar", "Cancel")).clicked() || esc;
                    });
                });
            if submit {
                let values: HashMap<String, String> = form.values.iter().cloned().collect();
                actions.push(Action::Copy {
                    item: Item::Text(template::render(&form.text, &template_vars(), &values)),
                    private: form.private,
                    paste: form.paste,
                });
                close = true;
            }
            if close {
                self.template_form = None;
            }
        }

        if let Some(path) = &self.confirm_restore {
            let mut close = false;
            egui::Window::new(t("Restaurar respaldo", "Restore backup"))
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(&ctx, |ui| {
                    ui.label(tr!(
                        "Se reemplazarán el historial, los tableros y los ajustes por los de:\n{}",
                        "History, boards and settings will be replaced by those in:\n{}",
                        path.display()
                    ));
                    ui.weak(t(
                        "La base actual se guarda aparte, por si acaso. Pega se reiniciará.",
                        "Your current database is kept aside, just in case. Pega will restart.",
                    ));
                    ui.horizontal(|ui| {
                        if ui.add(danger_button(t("Restaurar", "Restore"))).clicked() {
                            actions.push(Action::Restore(path.clone()));
                            close = true;
                        }
                        if ui.button(t("Cancelar", "Cancel")).clicked() {
                            close = true;
                        }
                    });
                });
            if close {
                self.confirm_restore = None;
            }
        }

        if let Some(win) = &mut self.settings_window {
            match win.show(&ctx) {
                Outcome::Open => {}
                Outcome::Save(s) => {
                    actions.push(Action::SaveSettings(s));
                    self.settings_window = None;
                }
                Outcome::Cancel => self.settings_window = None,
            }
        }

        drop(hist);
        drop(st);
        self.apply(&ctx, actions);
        // Ocultándose: las miniaturas se sueltan (egui libera sus texturas en este mismo
        // cuadro) y se devuelve la RAM física. Al volver se recrean las que se vean.
        if win::take_hiding() {
            self.thumbs.clear();
            win::trim_memory();
        }
    }

    fn on_exit(&mut self, _ctx: &egui::Context) {
        win::remove_tray();
    }
}

impl PegaApp {
    fn apply(&mut self, ctx: &egui::Context, actions: Vec<Action>) {
        for action in actions {
            match action {
                Action::Copy {
                    item,
                    private,
                    paste,
                } => {
                    let item = match item {
                        Item::Template(text) if template::is_template(&text) => {
                            let fields = template::fields(&text);
                            if fields.is_empty() {
                                Item::Text(template::render(
                                    &text,
                                    &template_vars(),
                                    &HashMap::new(),
                                ))
                            } else {
                                // Hay campos que pedir: se pega al completar el formulario.
                                self.template_form = Some(TemplateForm {
                                    text,
                                    private,
                                    paste,
                                    values: fields
                                        .into_iter()
                                        .map(|f| (f, String::new()))
                                        .collect(),
                                    focus_first: true,
                                });
                                continue;
                            }
                        }
                        other => other,
                    };
                    // El portapapeles va sin el candado: el hilo Win32 lo toma al ver el cambio.
                    let ok = match item {
                        Item::Text(text) | Item::Template(text) => win::write_text(&text, private),
                        Item::Image(id) => {
                            let loaded = self.state.lock().unwrap().image_for_paste(id);
                            loaded.is_some_and(|(img, png)| win::write_image(&img, &png))
                        }
                    };
                    if !ok {
                        self.flash(t("No se pudo copiar", "Couldn't copy"), 1.5, ctx);
                    } else if paste {
                        win::paste_into_previous(ctx);
                    } else {
                        self.flash(t("Copiado", "Copied"), 1.5, ctx);
                    }
                }
                Action::ImportPasteBar => {
                    let result = crate::import::import_pastebar(&mut self.state.lock().unwrap());
                    let msg = match result {
                        Ok(s) if s.boards + s.clips == 0 => t(
                            "PasteBar: nada nuevo que importar",
                            "PasteBar: nothing new to import",
                        )
                        .to_owned(),
                        Ok(s) => tr!(
                            "Importados {} tableros y {} clips{}",
                            "Imported {} boards and {} clips{}",
                            s.boards,
                            s.clips,
                            if s.skipped > 0 {
                                tr!(" ({} omitidos)", " ({} skipped)", s.skipped)
                            } else {
                                String::new()
                            }
                        ),
                        Err(e) => e,
                    };
                    self.flash(&msg, 5.0, ctx);
                }
                Action::Hide => win::hide(ctx),
                Action::Lock => self.lock.lock_now(),
                Action::ShowQr(text) => {
                    if let Some(img) = qr::image(&text, 5) {
                        self.qr = Some(ctx.load_texture("qr", img, egui::TextureOptions::NEAREST));
                    }
                }
                Action::StartExport => {
                    self.password = Some(PasswordDialog {
                        import: None,
                        focus: true,
                        ..Default::default()
                    });
                }
                Action::StartImport => {
                    if let Some(path) = backup::native_dialog(
                        win::app_hwnd(),
                        false,
                        "",
                        EXPORT_FILTER(),
                        export::EXTENSION,
                    ) {
                        self.password = Some(PasswordDialog {
                            import: Some(path),
                            focus: true,
                            ..Default::default()
                        });
                    }
                }
                Action::Export(pass) => {
                    let name = backup::default_name().replace(".pegabak", ".pega");
                    let Some(path) = backup::native_dialog(
                        win::app_hwnd(),
                        true,
                        &name,
                        EXPORT_FILTER(),
                        export::EXTENSION,
                    ) else {
                        continue;
                    };
                    let result = export::export(&mut self.state.lock().unwrap(), &pass)
                        .and_then(|bytes| std::fs::write(&path, bytes).map_err(|e| e.to_string()));
                    match result {
                        Ok(()) => self.flash(
                            &tr!("Exportado a {}", "Exported to {}", path.display()),
                            5.0,
                            ctx,
                        ),
                        Err(e) => self.flash(&e, 6.0, ctx),
                    }
                }
                Action::Import(path, pass) => {
                    let result =
                        std::fs::read(&path)
                            .map_err(|e| e.to_string())
                            .and_then(|bytes| {
                                export::import(&mut self.state.lock().unwrap(), &bytes, &pass)
                            });
                    match result {
                        Ok(s) => {
                            if s.hotkeys_changed {
                                win::reload_hotkey();
                            }
                            win::reschedule_expiry();
                            self.flash(
                                &tr!(
                                    "Importado: {} del historial, {} tableros, {} clips",
                                    "Imported: {} history items, {} boards, {} clips",
                                    s.history,
                                    s.boards,
                                    s.clips
                                ),
                                6.0,
                                ctx,
                            );
                        }
                        Err(e) => self.flash(&e, 6.0, ctx),
                    }
                }
                Action::PasteSequence(items) => {
                    self.multi.clear();
                    win::paste_sequence(ctx, items);
                }
                Action::Backup => {
                    let Some(path) =
                        backup::file_dialog(win::app_hwnd(), true, &backup::default_name())
                    else {
                        continue;
                    };
                    let result = self.state.lock().unwrap().backup_to(&path);
                    match result {
                        Ok(()) => self.flash(
                            &tr!(
                                "Respaldo guardado en {}",
                                "Backup saved to {}",
                                path.display()
                            ),
                            5.0,
                            ctx,
                        ),
                        Err(e) => self.flash(
                            &tr!(
                                "No se pudo crear el respaldo: {e}",
                                "Couldn't create the backup: {e}"
                            ),
                            6.0,
                            ctx,
                        ),
                    }
                }
                Action::PickRestore => {
                    let Some(path) = backup::file_dialog(win::app_hwnd(), false, "") else {
                        continue;
                    };
                    match backup::validate(&path) {
                        Ok(()) => self.confirm_restore = Some(path),
                        Err(e) => self.flash(&e, 6.0, ctx),
                    }
                }
                Action::Restore(path) => {
                    self.state.lock().unwrap().flush();
                    match backup::restart_with_restore(&path) {
                        Ok(()) => win::quit(),
                        Err(e) => self.flash(&e, 6.0, ctx),
                    }
                }
                Action::Quit => {
                    self.quitting = true;
                    win::quit();
                }
                other => {
                    let mut st = self.state.lock().unwrap();
                    match other {
                        Action::TogglePin(id) => st.toggle_pin(id),
                        Action::DeleteEntry(id) => st.delete_entry(id),
                        Action::ExcludeApp(app) => st.exclude_app(&app),
                        Action::ClearHistory => st.clear_history(),
                        Action::SaveToBoard { board_id, text } => {
                            let name: String = text
                                .trim()
                                .lines()
                                .next()
                                .unwrap_or("")
                                .chars()
                                .take(30)
                                .collect();
                            st.save_clip(None, board_id, &name, &text);
                        }
                        Action::AddBoard(name) => {
                            st.add_board(&name);
                        }
                        Action::RenameBoard(id, name) => st.rename_board(id, &name),
                        Action::DeleteBoard(id) => st.delete_board(id),
                        Action::SaveClip {
                            id,
                            board_id,
                            name,
                            text,
                        } => st.save_clip(id, board_id, &name, &text),
                        Action::DeleteClip(id) => st.delete_clip(id),
                        Action::SetBoardColor(id, color) => st.set_board_color(id, color),
                        Action::MoveBoard { id, before } => st.move_board(id, before),
                        Action::MoveClip {
                            id,
                            board_id,
                            before,
                        } => st.move_clip(id, board_id, before),
                        Action::SaveSettings(s) => {
                            if st.save_settings(s) {
                                win::reload_hotkey();
                            }
                        }
                        Action::Copy { .. }
                        | Action::ImportPasteBar
                        | Action::Lock
                        | Action::PasteSequence(_)
                        | Action::ShowQr(_)
                        | Action::StartExport
                        | Action::StartImport
                        | Action::Export(_)
                        | Action::Import(..)
                        | Action::Backup
                        | Action::PickRestore
                        | Action::Restore(_)
                        | Action::Hide
                        | Action::Quit => unreachable!(),
                    }
                    // Fijar, borrar o cambiar ajustes mueve el próximo vencimiento.
                    win::reschedule_expiry();
                }
            }
        }
    }

    fn flash(&mut self, msg: &str, secs: f64, ctx: &egui::Context) {
        self.flash = Some((msg.to_owned(), ctx.input(|i| i.time) + secs));
        // Un repintado más para que el aviso desaparezca solo.
        ctx.request_repaint_after(Duration::from_secs_f64(secs + 0.05));
    }
}
