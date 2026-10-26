//! Interfaz egui: historial a la izquierda, tableros a la derecha, búsqueda arriba.
//! egui solo redibuja cuando hay un evento, así que en reposo no gasta CPU.
//!
//! Clic = copiar (la ventana sigue abierta). Doble clic o Enter = pegar en la app anterior.

use crate::image;
use crate::settings::{Settings, Theme};
use crate::settings_ui::{Outcome, SettingsWindow};
use crate::state::{self, State};
use crate::win;
use egui::{Key, Modifiers, RichText, TextWrapMode, ViewportCommand};
use egui_software_backend::SoftwareBackend;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Qué se copia: el texto ya está en memoria; la imagen se lee de disco al usarla.
enum Item {
    Text(String),
    Image(i64),
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
    ImportPasteBar,
    SaveSettings(Settings),
    Hide,
    Quit,
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
    /// Aviso corto ("Copiado") y hasta cuándo se muestra.
    flash: Option<(String, f64)>,
    settings_window: Option<SettingsWindow>,
    /// Tema aplicado a egui; se reaplica solo si cambia en los ajustes.
    applied_theme: Option<Theme>,
}

impl PegaApp {
    pub fn new(state: Arc<Mutex<State>>) -> Self {
        Self {
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
        out.push_str(&format!("  (+{} líneas)", lines - 1));
    } else if first.chars().count() > 120 {
        out.push('…');
    }
    out
}

/// Texto del tooltip: de dónde y cuándo vino, y el contenido (recortado).
fn tooltip(text: &str, app: Option<&str>, used_at: i64) -> String {
    let mut t = match app {
        Some(app) => format!("Desde {app} · {}", ago(used_at)),
        None => ago(used_at),
    };
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
        0..60 => "hace un momento".into(),
        60..3600 => format!("hace {} min", s / 60),
        3600..86400 => format!("hace {} h", s / 3600),
        _ => format!("hace {} días", s / 86400),
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
        let st = state.lock().unwrap();

        // Cerrar la ventana la oculta (sigue escuchando); "Salir" la cierra de verdad.
        // Sin atajo global no habría forma de volver a abrirla, así que ahí sí se cierra.
        if ctx.input(|i| i.viewport().close_requested())
            && !self.quitting
            && win::can_reopen(st.hotkey_ok)
        {
            ctx.send_viewport_cmd(ViewportCommand::CancelClose);
            actions.push(Action::Hide);
        }

        if self.applied_theme != Some(st.settings.theme) {
            ctx.set_theme(match st.settings.theme {
                Theme::System => egui::ThemePreference::System,
                Theme::Dark => egui::ThemePreference::Dark,
                Theme::Light => egui::ThemePreference::Light,
            });
            self.applied_theme = Some(st.settings.theme);
        }
        let hotkey = st.settings.hotkey.label();
        let modal_open = self.editor.is_some() || self.settings_window.is_some();

        let just_shown = win::take_shown();
        if just_shown {
            self.query.clear();
            self.selected = 0;
            self.scroll_to = Some(0);
        }

        let terms: Vec<String> = state::fold(&self.query)
            .split_whitespace()
            .map(str::to_owned)
            .collect();
        // Fijados primero, luego por uso reciente.
        let mut hist: Vec<&state::Entry> = st
            .history
            .iter()
            .filter(|e| state::matches(&e.key, &terms))
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
            if esc {
                if self.query.is_empty() {
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
                    if ui.checkbox(&mut autostart, "Iniciar con Windows").changed() {
                        crate::autostart::set(autostart);
                    }
                    if ui.button("Importar de PasteBar").clicked() {
                        actions.push(Action::ImportPasteBar);
                    }
                    if ui.button("Ajustes…").clicked() {
                        self.settings_window = Some(SettingsWindow::new(&st.settings));
                    }
                    ui.separator();
                    if ui
                        .button("Salir")
                        .on_hover_text("Cierra Pega por completo")
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
                let hint = format!("Buscar en historial y tableros…   ({hotkey} abre/oculta)");
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
                    format!("No se pudo registrar {hotkey}: otra app lo usa. Cámbialo en Ajustes."),
                );
            }
            if let Some(err) = &st.error {
                ui.colored_label(ui.visuals().error_fg_color, err);
            }
            ui.add_space(4.0);
        });

        egui::Panel::left("history")
            .resizable(true)
            .default_size(340.0)
            .show_inside(ui, |ui| {
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.strong(format!("Historial ({})", st.history.len()));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if self.confirm_clear {
                            if ui.button("Cancelar").clicked() {
                                self.confirm_clear = false;
                            }
                            if ui
                                .button(
                                    RichText::new("Sí, borrar").color(ui.visuals().error_fg_color),
                                )
                                .clicked()
                            {
                                actions.push(Action::ClearHistory);
                                self.confirm_clear = false;
                            }
                        } else if ui
                            .small_button("Limpiar")
                            .on_hover_text("Borra todo menos lo fijado")
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
                                let selected = i == self.selected;
                                let pin = if e.pinned { "📌 " } else { "" };
                                let app = st.app_name(e.app);
                                let right = egui::RichText::new(app.map(short_app).unwrap_or(""))
                                    .small()
                                    .weak();
                                let resp = match &e.image {
                                    Some(img) => {
                                        let label =
                                            format!("{pin}Imagen {}×{}", img.width, img.height);
                                        match self.thumb(&ctx, e) {
                                            Some(tex) => ui.add(
                                                egui::Button::selectable(
                                                    selected,
                                                    (egui::Image::new(&tex), label),
                                                )
                                                .right_text(right),
                                            ),
                                            None => ui.add(
                                                egui::Button::selectable(selected, label)
                                                    .right_text(right),
                                            ),
                                        }
                                    }
                                    None => ui.add(
                                        egui::Button::selectable(
                                            selected,
                                            format!("{pin}{}", preview(&e.text)),
                                        )
                                        .right_text(right)
                                        .min_size(egui::vec2(0.0, row_h)),
                                    ),
                                }
                                .on_hover_text(tooltip(&e.text, app, e.used_at));
                                if scroll_to == Some(i) {
                                    resp.scroll_to_me(None);
                                }
                                if resp.double_clicked() {
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
                                    if ui.button("Pegar").clicked() {
                                        actions.push(Action::Copy {
                                            item: item_of(e),
                                            private: false,
                                            paste: true,
                                        });
                                    }
                                    if ui
                                        .button(if e.pinned { "Desfijar" } else { "Fijar" })
                                        .clicked()
                                    {
                                        actions.push(Action::TogglePin(e.id));
                                    }
                                    if e.image.is_none() && !st.boards.is_empty() {
                                        ui.menu_button("Guardar en tablero", |ui| {
                                            for b in &st.boards {
                                                if ui.button(&b.name).clicked() {
                                                    actions.push(Action::SaveToBoard {
                                                        board_id: b.id,
                                                        text: e.text.clone(),
                                                    });
                                                }
                                            }
                                        });
                                    }
                                    ui.separator();
                                    if let Some(app) = app {
                                        if ui
                                            .button(format!("No guardar copias de {app}"))
                                            .clicked()
                                        {
                                            actions.push(Action::ExcludeApp(app.to_owned()));
                                        }
                                    }
                                    if ui.button("Borrar").clicked() {
                                        actions.push(Action::DeleteEntry(e.id));
                                    }
                                });
                            }
                        });
                        if hist.is_empty() {
                            ui.weak(if terms.is_empty() {
                                "Copia algo y aparecerá aquí."
                            } else {
                                "Sin resultados."
                            });
                        }
                    });
            });

        egui::CentralPanel::default().show_inside(ui, |ui| {
            ui.horizontal(|ui| {
                ui.strong("Tableros");
                match &mut self.new_board {
                    Some(name) => {
                        let r = ui.add(
                            egui::TextEdit::singleline(name)
                                .hint_text("Nombre del tablero")
                                .desired_width(180.0),
                        );
                        r.request_focus();
                        let enter = r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                        if (ui.button("Crear").clicked() || enter) && !name.trim().is_empty() {
                            actions.push(Action::AddBoard(name.trim().to_owned()));
                            self.new_board = None;
                        } else if ui.button("Cancelar").clicked() {
                            self.new_board = None;
                        }
                    }
                    None => {
                        if ui.button("+ Tablero").clicked() {
                            self.new_board = Some(String::new());
                        }
                    }
                }
            });
            ui.separator();
            egui::ScrollArea::vertical().id_salt("boards").auto_shrink(false).show(ui, |ui| {
                if st.boards.is_empty() {
                    ui.weak("Crea un tablero para guardar textos fijos (datos, plantillas, respuestas…).");
                }
                for b in &st.boards {
                    let board_hit = state::matches(&state::fold(&b.name), &terms);
                    let clips: Vec<&state::Clip> = st
                        .clips
                        .iter()
                        .filter(|c| c.board_id == b.id && (board_hit || state::matches(&c.key, &terms)))
                        .collect();
                    if !terms.is_empty() && clips.is_empty() && !board_hit {
                        continue;
                    }
                    egui::Frame::group(ui.style()).show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            let editing = matches!(&self.renaming, Some((id, _)) if *id == b.id);
                            if editing {
                                let (id, name) = self.renaming.as_mut().unwrap();
                                let r = ui.add(egui::TextEdit::singleline(name).desired_width(180.0));
                                r.request_focus();
                                if r.lost_focus() {
                                    if !name.trim().is_empty() {
                                        actions.push(Action::RenameBoard(*id, name.trim().to_owned()));
                                    }
                                    self.renaming = None;
                                }
                            } else {
                                ui.strong(&b.name);
                            }
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                ui.menu_button("…", |ui| {
                                    if ui.button("Renombrar").clicked() {
                                        self.renaming = Some((b.id, b.name.clone()));
                                    }
                                    if ui.button("Borrar tablero").clicked() {
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
                                let resp = ui
                                    .button(&c.name)
                                    .on_hover_text("Clic: copiar · Doble clic: pegar");
                                if resp.double_clicked() {
                                    actions.push(Action::Copy {
                                        item: Item::Text(c.text.clone()),
                                        private: true,
                                        paste: true,
                                    });
                                } else if resp.clicked() {
                                    actions.push(Action::Copy {
                                        item: Item::Text(c.text.clone()),
                                        private: true,
                                        paste: false,
                                    });
                                }
                                resp.context_menu(|ui| {
                                    if ui.button("Editar").clicked() {
                                        self.editor = Some(Editor {
                                            id: Some(c.id),
                                            board_id: c.board_id,
                                            name: c.name.clone(),
                                            text: c.text.clone(),
                                        });
                                    }
                                    if ui.button("Borrar").clicked() {
                                        actions.push(Action::DeleteClip(c.id));
                                    }
                                });
                            }
                        });
                    });
                    ui.add_space(6.0);
                }
            });
        });

        if let Some(ed) = &mut self.editor {
            let mut open = true;
            let mut done = false;
            egui::Window::new(if ed.id.is_some() {
                "Editar clip"
            } else {
                "Nuevo clip"
            })
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .open(&mut open)
            .show(&ctx, |ui| {
                ui.label("Nombre");
                ui.add(egui::TextEdit::singleline(&mut ed.name).desired_width(360.0));
                ui.label("Contenido (se guarda cifrado)");
                ui.add(
                    egui::TextEdit::multiline(&mut ed.text)
                        .desired_width(360.0)
                        .desired_rows(6),
                );
                ui.horizontal(|ui| {
                    let valid = !ed.name.trim().is_empty() && !ed.text.is_empty();
                    if ui
                        .add_enabled(valid, egui::Button::new("Guardar"))
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
                    if ui.button("Cancelar").clicked() {
                        done = true;
                    }
                });
            });
            if done || !open {
                self.editor = None;
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
                    // El portapapeles va sin el candado: el hilo Win32 lo toma al ver el cambio.
                    let ok = match item {
                        Item::Text(text) => win::write_text(&text, private),
                        Item::Image(id) => {
                            let loaded = self.state.lock().unwrap().image_for_paste(id);
                            loaded.is_some_and(|(img, png)| win::write_image(&img, &png))
                        }
                    };
                    if !ok {
                        self.flash("No se pudo copiar", 1.5, ctx);
                    } else if paste {
                        win::paste_into_previous(ctx);
                    } else {
                        self.flash("Copiado", 1.5, ctx);
                    }
                }
                Action::ImportPasteBar => {
                    let result = crate::import::import_pastebar(&mut self.state.lock().unwrap());
                    let msg = match result {
                        Ok(s) if s.boards + s.clips == 0 => {
                            "PasteBar: nada nuevo que importar".to_owned()
                        }
                        Ok(s) => format!(
                            "Importados {} tableros y {} clips{}",
                            s.boards,
                            s.clips,
                            if s.skipped > 0 {
                                format!(" ({} omitidos)", s.skipped)
                            } else {
                                String::new()
                            }
                        ),
                        Err(e) => e,
                    };
                    self.flash(&msg, 5.0, ctx);
                }
                Action::Hide => win::hide(ctx),
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
                        Action::SaveSettings(s) => {
                            if st.save_settings(s) {
                                win::reload_hotkey();
                            }
                        }
                        Action::Copy { .. }
                        | Action::ImportPasteBar
                        | Action::Hide
                        | Action::Quit => unreachable!(),
                    }
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
