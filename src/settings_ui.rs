//! Ventana de ajustes. Edita una copia; nada se aplica hasta "Guardar".

use crate::settings::{self, Hotkey, Settings, Theme};

pub struct SettingsWindow {
    draft: Settings,
    /// Apps excluidas, una por línea, tal como se editan.
    apps_text: String,
    /// Palabras a ocultar, una por línea.
    words_text: String,
    pin_new: String,
    pin_confirm: String,
    changing_pin: bool,
    capturing: bool,
    warning: Option<String>,
}

pub enum Outcome {
    Open,
    Save(Settings),
    Cancel,
}

impl SettingsWindow {
    pub fn new(current: &Settings) -> Self {
        Self {
            draft: current.clone(),
            apps_text: current.excluded_apps.join("\n"),
            words_text: current.mask_words.join("\n"),
            pin_new: String::new(),
            pin_confirm: String::new(),
            changing_pin: false,
            capturing: false,
            warning: None,
        }
    }

    pub fn show(&mut self, ctx: &egui::Context) -> Outcome {
        let mut outcome = Outcome::Open;
        let mut open = true;
        if self.capturing {
            self.capture_hotkey(ctx);
        }
        egui::Window::new("Ajustes")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .open(&mut open)
            .show(ctx, |ui| {
                egui::Grid::new("ajustes")
                    .num_columns(2)
                    .spacing([16.0, 10.0])
                    .show(ui, |ui| {
                        ui.label("Atajo para abrir");
                        ui.horizontal(|ui| {
                            if self.capturing {
                                ui.label(
                                    egui::RichText::new("Presiona la combinación… (Esc cancela)")
                                        .italics(),
                                );
                            } else {
                                ui.monospace(self.draft.hotkey.label());
                                if ui.button("Cambiar").clicked() {
                                    self.capturing = true;
                                    self.warning = None;
                                }
                            }
                        });
                        ui.end_row();

                        ui.label("Tema");
                        ui.horizontal(|ui| {
                            ui.radio_value(&mut self.draft.theme, Theme::System, "Sistema");
                            ui.radio_value(&mut self.draft.theme, Theme::Dark, "Oscuro");
                            ui.radio_value(&mut self.draft.theme, Theme::Light, "Claro");
                        });
                        ui.end_row();

                        ui.label("Textos en el historial");
                        limit_editor(ui, &mut self.draft.max_texts, 1000);
                        ui.end_row();

                        ui.label("Imágenes en el historial");
                        limit_editor(ui, &mut self.draft.max_images, 100);
                        ui.end_row();

                        ui.label("Borrar copias con más de");
                        ui.horizontal(|ui| {
                            let mut forever = self.draft.keep_days == 0;
                            if !forever {
                                ui.add(
                                    egui::DragValue::new(&mut self.draft.keep_days)
                                        .range(1..=3650)
                                        .suffix(" días"),
                                );
                            }
                            if ui.checkbox(&mut forever, "Nunca").changed() {
                                self.draft.keep_days = if forever { 0 } else { 30 };
                            }
                        });
                        ui.end_row();

                        ui.label("No guardar copias de")
                            .on_hover_text("Un ejecutable por línea, por ejemplo keepass.exe");
                        ui.add(
                            egui::TextEdit::multiline(&mut self.apps_text)
                                .hint_text("keepass.exe")
                                .desired_rows(3)
                                .desired_width(220.0),
                        );
                        ui.end_row();
                    });
                ui.weak("Lo fijado nunca se borra por los límites ni por antigüedad.");
                ui.separator();
                ui.strong("Privacidad");
                egui::Grid::new("privacidad").num_columns(2).spacing([16.0, 10.0]).show(ui, |ui| {
                    ui.label("Ocultar estas palabras")
                        .on_hover_text("Una por línea. Se ven como ••• en la lista; al pegar sale el texto real.");
                    ui.add(
                        egui::TextEdit::multiline(&mut self.words_text)
                            .desired_rows(2)
                            .desired_width(220.0),
                    );
                    ui.end_row();

                    ui.label("");
                    ui.checkbox(&mut self.draft.mask_cards, "Ocultar números de tarjeta");
                    ui.end_row();

                    ui.label("PIN de bloqueo").on_hover_text(
                        "Bloquea la ventana. Los datos ya están cifrados en disco; esto evita que \
                         alguien frente a tu PC los vea.",
                    );
                    self.pin_editor(ui);
                    ui.end_row();

                    if !self.draft.pin.is_empty() {
                        ui.label("Bloquear tras");
                        ui.horizontal(|ui| {
                            let mut only_start = self.draft.lock_minutes == 0;
                            if !only_start {
                                ui.add(
                                    egui::DragValue::new(&mut self.draft.lock_minutes)
                                        .range(1..=240)
                                        .suffix(" min sin uso"),
                                );
                            }
                            if ui.checkbox(&mut only_start, "Solo al iniciar").changed() {
                                self.draft.lock_minutes = if only_start { 0 } else { 5 };
                            }
                        });
                        ui.end_row();
                    }
                });
                if let Some(w) = &self.warning {
                    ui.colored_label(ui.visuals().warn_fg_color, w);
                }
                ui.separator();
                ui.horizontal(|ui| {
                    if ui.button("Guardar").clicked() {
                        let mut s = self.draft.clone();
                        s.excluded_apps = self
                            .apps_text
                            .lines()
                            .map(|l| l.trim().to_lowercase())
                            .filter(|l| !l.is_empty())
                            .collect();
                        s.mask_words = self
                            .words_text
                            .lines()
                            .map(str::trim)
                            .filter(|l| !l.is_empty())
                            .map(str::to_owned)
                            .collect();
                        if self.changing_pin {
                            self.warning = Some("Termina o cancela el cambio de PIN.".into());
                        } else {
                            outcome = Outcome::Save(s);
                        }
                    }
                    if ui.button("Cancelar").clicked() {
                        outcome = Outcome::Cancel;
                    }
                });
            });
        if !open {
            outcome = Outcome::Cancel;
        }
        outcome
    }

    fn pin_editor(&mut self, ui: &mut egui::Ui) {
        ui.vertical(|ui| {
            if !self.changing_pin {
                ui.horizontal(|ui| {
                    if self.draft.pin.is_empty() {
                        ui.label("Sin PIN");
                        if ui.button("Activar").clicked() {
                            self.changing_pin = true;
                        }
                    } else {
                        ui.label("Activo");
                        if ui.button("Cambiar").clicked() {
                            self.changing_pin = true;
                        }
                        if ui.button("Quitar").clicked() {
                            self.draft.pin.clear();
                        }
                    }
                });
                return;
            }
            ui.add(pin_field(&mut self.pin_new, "Nuevo PIN (4 a 12 dígitos)"));
            ui.add(pin_field(&mut self.pin_confirm, "Repetir PIN"));
            ui.horizontal(|ui| {
                if ui.button("Aceptar").clicked() {
                    let valid = (4..=12).contains(&self.pin_new.len())
                        && self.pin_new.chars().all(|c| c.is_ascii_digit());
                    if !valid {
                        self.warning = Some("El PIN debe tener de 4 a 12 dígitos.".into());
                    } else if self.pin_new != self.pin_confirm {
                        self.warning = Some("Los PIN no coinciden.".into());
                    } else {
                        self.draft.pin = std::mem::take(&mut self.pin_new);
                        self.pin_confirm.clear();
                        self.changing_pin = false;
                        self.warning = None;
                    }
                }
                if ui.button("Cancelar").clicked() {
                    self.pin_new.clear();
                    self.pin_confirm.clear();
                    self.changing_pin = false;
                }
            });
        });
    }

    fn capture_hotkey(&mut self, ctx: &egui::Context) {
        let pressed = ctx.input_mut(|i| {
            let found = i.events.iter().find_map(|e| match e {
                egui::Event::Key {
                    key,
                    pressed: true,
                    modifiers,
                    ..
                } => Some((*key, *modifiers)),
                _ => None,
            });
            // Que la tecla no llegue a la búsqueda ni a otros controles.
            i.events
                .retain(|e| !matches!(e, egui::Event::Key { .. } | egui::Event::Text(_)));
            found
        });
        let Some((key, mods)) = pressed else { return };
        if key == egui::Key::Escape {
            self.capturing = false;
            return;
        }
        let Some(vk) = settings::vk_from_egui(key) else {
            self.warning = Some("Esa tecla no se puede usar en un atajo.".into());
            return;
        };
        let hk = Hotkey {
            ctrl: mods.ctrl,
            alt: mods.alt,
            shift: mods.shift,
            win: false,
            vk,
        };
        if hk.is_valid() {
            self.draft.hotkey = hk;
            self.capturing = false;
            self.warning = None;
        } else {
            self.warning = Some(
                "Usa Ctrl o Alt en la combinación, para no robarle la tecla a otros programas."
                    .into(),
            );
        }
    }
}

/// Número con casilla "sin límite" (0 en los ajustes).
fn limit_editor(ui: &mut egui::Ui, value: &mut usize, default: usize) {
    ui.horizontal(|ui| {
        let mut unlimited = *value == 0;
        if !unlimited {
            ui.add(egui::DragValue::new(value).range(1..=1_000_000).speed(10));
        }
        if ui.checkbox(&mut unlimited, "Sin límite").changed() {
            *value = if unlimited { 0 } else { default };
        }
    });
}

fn pin_field<'a>(text: &'a mut String, hint: &str) -> egui::TextEdit<'a> {
    egui::TextEdit::singleline(text)
        .password(true)
        .hint_text(hint.to_owned())
        .desired_width(120.0)
}
