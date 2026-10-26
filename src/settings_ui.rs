//! Ventana de ajustes. Edita una copia; nada se aplica hasta t("Guardar", "Save").

use crate::i18n::t;
use crate::settings::{self, Hotkey, Settings, Theme, UiLang};

pub struct SettingsWindow {
    draft: Settings,
    /// Apps excluidas, una por línea, tal como se editan.
    apps_text: String,
    /// Palabras a ocultar, una por línea.
    words_text: String,
    /// Apps sensibles, una por línea.
    sensitive_text: String,
    pin_new: String,
    pin_confirm: String,
    changing_pin: bool,
    /// Qué atajo se está capturando (el de abrir o el de pegar sin formato).
    capturing: Option<Target>,
    warning: Option<String>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Target {
    Open,
    Plain,
}

fn capture_hint(ui: &mut egui::Ui) {
    ui.label(
        egui::RichText::new(t(
            "Presiona la combinación… (Esc cancela)",
            "Press the combination… (Esc cancels)",
        ))
        .italics(),
    );
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
            sensitive_text: current.sensitive_apps.join("\n"),
            pin_new: String::new(),
            pin_confirm: String::new(),
            changing_pin: false,
            capturing: None,
            warning: None,
        }
    }

    pub fn show(&mut self, ctx: &egui::Context) -> Outcome {
        let mut outcome = Outcome::Open;
        let mut open = true;
        if let Some(target) = self.capturing {
            self.capture_hotkey(ctx, target);
        }
        egui::Window::new(t("Ajustes", "Settings"))
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .open(&mut open)
            .show(ctx, |ui| {
                egui::Grid::new("ajustes")
                    .num_columns(2)
                    .spacing([16.0, 10.0])
                    .show(ui, |ui| {
                        ui.label(t("Atajo para abrir", "Shortcut to open"));
                        ui.horizontal(|ui| {
                            if self.capturing == Some(Target::Open) {
                                capture_hint(ui);
                            } else {
                                ui.monospace(self.draft.hotkey.label());
                                if ui.button(t("Cambiar", "Change")).clicked() {
                                    self.capturing = Some(Target::Open);
                                    self.warning = None;
                                }
                            }
                        });
                        ui.end_row();

                        ui.label(t("Pegar sin formato", "Paste as plain text")).on_hover_text(t(
                            "Desde cualquier app: pega lo copiado como texto plano.",
                            "From any app: pastes what you copied as plain text.",
                        ));
                        ui.horizontal(|ui| {
                            if self.capturing == Some(Target::Plain) {
                                capture_hint(ui);
                            } else {
                                match self.draft.plain_hotkey {
                                    Some(hk) => {
                                        ui.monospace(hk.label());
                                    }
                                    None => {
                                        ui.weak(t("Desactivado", "Off"));
                                    }
                                }
                                if ui.button(t("Cambiar", "Change")).clicked() {
                                    self.capturing = Some(Target::Plain);
                                    self.warning = None;
                                }
                                if self.draft.plain_hotkey.is_some() && ui.button(t("Quitar", "Remove")).clicked() {
                                    self.draft.plain_hotkey = None;
                                }
                            }
                        });
                        ui.end_row();

                        ui.label(t("Idioma", "Language"));
                        ui.horizontal(|ui| {
                            ui.radio_value(&mut self.draft.language, UiLang::System, t("Sistema", "System"));
                            ui.radio_value(&mut self.draft.language, UiLang::Spanish, "Español");
                            ui.radio_value(&mut self.draft.language, UiLang::English, "English");
                        });
                        ui.end_row();

                        ui.label(t("Tema", "Theme"));
                        ui.horizontal(|ui| {
                            ui.radio_value(&mut self.draft.theme, Theme::System, t("Sistema", "System"));
                            ui.radio_value(&mut self.draft.theme, Theme::Dark, t("Oscuro", "Dark"));
                            ui.radio_value(&mut self.draft.theme, Theme::Light, t("Claro", "Light"));
                        });
                        ui.end_row();

                        ui.label(t("Textos en el historial", "Texts in history"));
                        limit_editor(ui, &mut self.draft.max_texts, 1000);
                        ui.end_row();

                        ui.label(t("Imágenes en el historial", "Images in history"));
                        limit_editor(ui, &mut self.draft.max_images, 100);
                        ui.end_row();

                        ui.label(t("Borrar copias con más de", "Delete copies older than"));
                        ui.horizontal(|ui| {
                            let mut forever = self.draft.keep_days == 0;
                            if !forever {
                                ui.add(
                                    egui::DragValue::new(&mut self.draft.keep_days)
                                        .range(1..=3650)
                                        .suffix(t(" días", " days")),
                                );
                            }
                            if ui.checkbox(&mut forever, t("Nunca", "Never")).changed() {
                                self.draft.keep_days = if forever { 0 } else { 30 };
                            }
                        });
                        ui.end_row();

                        ui.label(t("No guardar copias de", "Don't save copies from"))
                            .on_hover_text(t("Un ejecutable por línea, por ejemplo keepass.exe", "One executable per line, e.g. keepass.exe"));
                        ui.add(
                            egui::TextEdit::multiline(&mut self.apps_text)
                                .hint_text("keepass.exe")
                                .desired_rows(3)
                                .desired_width(220.0),
                        );
                        ui.end_row();
                    });
                ui.weak(t("Lo fijado nunca se borra por los límites ni por antigüedad.", "Pinned items are never deleted by limits or age."));
                ui.separator();
                ui.strong(t("Privacidad", "Privacy"));
                egui::Grid::new("privacidad").num_columns(2).spacing([16.0, 10.0]).show(ui, |ui| {
                    ui.label(t("Ocultar estas palabras", "Hide these words"))
                        .on_hover_text(t("Una por línea. Se ven como ••• en la lista; al pegar sale el texto real.", "One per line. Shown as ••• in the list; pasting gives the real text."));
                    ui.add(
                        egui::TextEdit::multiline(&mut self.words_text)
                            .desired_rows(2)
                            .desired_width(220.0),
                    );
                    ui.end_row();

                    ui.label(t("Accesibilidad", "Accessibility"));
                    ui.checkbox(
                        &mut self.draft.accessibility,
                        t("Lectores de pantalla (Narrador, NVDA)", "Screen readers (Narrator, NVDA)"),
                    )
                    .on_hover_text(t(
                        "Se aplica al reiniciar Pega. Experimental: con un lector conectado puede usar algo de CPU.",
                        "Applies after restarting Pega. Experimental: may use some CPU while a reader is connected.",
                    ));
                    ui.end_row();

                    ui.label(t("Texto de las capturas", "Text in screenshots"));
                    ui.checkbox(&mut self.draft.ocr, t("Leer con el OCR de Windows", "Read with Windows OCR"))
                        .on_hover_text(t(
                            "Para buscar por el texto que aparece en las imágenes. Corre en un proceso aparte \
                             (pega-ocr.exe) que termina al responder: no suma memoria a Pega.",
                            "To search by the text shown in images. Runs in a separate process (pega-ocr.exe) \
                             that exits after answering: it adds no memory to Pega.",
                        ));
                    ui.end_row();

                    ui.label("");
                    ui.add_enabled(
                        self.draft.ocr,
                        egui::Checkbox::new(
                            &mut self.draft.ocr_sensitive,
                            t("Avisar y borrar capturas con datos sensibles", "Warn about and delete screenshots with sensitive data"),
                        ),
                    )
                    .on_hover_text(t(
                        "Si una captura muestra un número de tarjeta, una clave o token, o una de tus palabras \
                         ocultas, te avisa y la borra sola tras los minutos de abajo (fíjala para conservarla).",
                        "If a screenshot shows a card number, a key or token, or one of your hidden words, Pega \
                         warns you and deletes it after the minutes below (pin it to keep it).",
                    ));
                    ui.end_row();

                    ui.label(t("Borrar solos tras", "Auto-delete after")).on_hover_text(t(
                        "Códigos de un solo uso (4 a 6 dígitos), lo copiado desde las apps sensibles y las capturas con datos sensibles.",
                        "One-time codes (4 to 6 digits), copies from sensitive apps and screenshots with sensitive data.",
                    ));
                    ui.horizontal(|ui| {
                        let mut never = self.draft.expire_minutes == 0;
                        if !never {
                            ui.add(
                                egui::DragValue::new(&mut self.draft.expire_minutes)
                                    .range(1..=1440)
                                    .suffix(" min"),
                            );
                        }
                        if ui.checkbox(&mut never, t("Nunca", "Never")).changed() {
                            self.draft.expire_minutes = if never { 0 } else { 5 };
                        }
                    });
                    ui.end_row();

                    ui.label(t("Apps sensibles", "Sensitive apps")).on_hover_text(t(
                        "Un ejecutable por línea: sus copias se guardan, pero se borran solas.",
                        "One executable per line: their copies are kept, but delete themselves.",
                    ));
                    ui.add(
                        egui::TextEdit::multiline(&mut self.sensitive_text)
                            .hint_text("banco.exe")
                            .desired_rows(2)
                            .desired_width(220.0),
                    );
                    ui.end_row();

                    ui.label("");
                    ui.checkbox(&mut self.draft.mask_cards, t("Ocultar números de tarjeta", "Hide card numbers"));
                    ui.end_row();

                    ui.label(t("PIN de bloqueo", "Lock PIN")).on_hover_text(
                        t(
                            "Bloquea la ventana. Los datos ya están cifrados en disco; esto evita que alguien frente a tu PC los vea.",
                            "Locks the window. Data is already encrypted on disk; this keeps someone at your PC from seeing it.",
                        ),
                    );
                    self.pin_editor(ui);
                    ui.end_row();

                    if !self.draft.pin.is_empty() {
                        ui.label(t("Bloquear tras", "Lock after"));
                        ui.horizontal(|ui| {
                            let mut only_start = self.draft.lock_minutes == 0;
                            if !only_start {
                                ui.add(
                                    egui::DragValue::new(&mut self.draft.lock_minutes)
                                        .range(1..=240)
                                        .suffix(t(" min sin uso", " min idle")),
                                );
                            }
                            if ui.checkbox(&mut only_start, t("Solo al iniciar", "Only at startup")).changed() {
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
                    if ui.button(t("Guardar", "Save")).clicked() {
                        let mut s = self.draft.clone();
                        s.excluded_apps = self
                            .apps_text
                            .lines()
                            .map(|l| l.trim().to_lowercase())
                            .filter(|l| !l.is_empty())
                            .collect();
                        s.sensitive_apps = self
                            .sensitive_text
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
                            self.warning = Some(t("Termina o cancela el cambio de PIN.", "Finish or cancel the PIN change.").into());
                        } else {
                            outcome = Outcome::Save(s);
                        }
                    }
                    if ui.button(t("Cancelar", "Cancel")).clicked() {
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
                        ui.label(t("Sin PIN", "No PIN"));
                        if ui.button(t("Activar", "Enable")).clicked() {
                            self.changing_pin = true;
                        }
                    } else {
                        ui.label(t("Activo", "On"));
                        if ui.button(t("Cambiar", "Change")).clicked() {
                            self.changing_pin = true;
                        }
                        if ui.button(t("Quitar", "Remove")).clicked() {
                            self.draft.pin.clear();
                        }
                    }
                });
                return;
            }
            ui.add(pin_field(
                &mut self.pin_new,
                t("Nuevo PIN (4 a 12 dígitos)", "New PIN (4 to 12 digits)"),
            ));
            ui.add(pin_field(
                &mut self.pin_confirm,
                t("Repetir PIN", "Repeat PIN"),
            ));
            ui.horizontal(|ui| {
                if ui.button(t("Aceptar", "OK")).clicked() {
                    let valid = (4..=12).contains(&self.pin_new.len())
                        && self.pin_new.chars().all(|c| c.is_ascii_digit());
                    if !valid {
                        self.warning = Some(
                            t(
                                "El PIN debe tener de 4 a 12 dígitos.",
                                "The PIN must have 4 to 12 digits.",
                            )
                            .into(),
                        );
                    } else if self.pin_new != self.pin_confirm {
                        self.warning =
                            Some(t("Los PIN no coinciden.", "The PINs don't match.").into());
                    } else {
                        self.draft.pin = std::mem::take(&mut self.pin_new);
                        self.pin_confirm.clear();
                        self.changing_pin = false;
                        self.warning = None;
                    }
                }
                if ui.button(t("Cancelar", "Cancel")).clicked() {
                    self.pin_new.clear();
                    self.pin_confirm.clear();
                    self.changing_pin = false;
                }
            });
        });
    }

    fn capture_hotkey(&mut self, ctx: &egui::Context, target: Target) {
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
            self.capturing = None;
            return;
        }
        let Some(vk) = settings::vk_from_egui(key) else {
            self.warning = Some(
                t(
                    "Esa tecla no se puede usar en un atajo.",
                    "That key can't be used in a shortcut.",
                )
                .into(),
            );
            return;
        };
        let hk = Hotkey {
            ctrl: mods.ctrl,
            alt: mods.alt,
            shift: mods.shift,
            win: false,
            vk,
        };
        let other = match target {
            Target::Open => self.draft.plain_hotkey,
            Target::Plain => Some(self.draft.hotkey),
        };
        if other == Some(hk) {
            self.warning = Some(
                t(
                    "Ese atajo ya está en uso en Pega.",
                    "That shortcut is already used in Pega.",
                )
                .into(),
            );
        } else if hk.is_valid() {
            match target {
                Target::Open => self.draft.hotkey = hk,
                Target::Plain => self.draft.plain_hotkey = Some(hk),
            }
            self.capturing = None;
            self.warning = None;
        } else {
            self.warning = Some(
                t(
                    "Usa Ctrl o Alt en la combinación, para no robarle la tecla a otros programas.",
                    "Use Ctrl or Alt in the combination, so other programs keep the key.",
                )
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
        if ui
            .checkbox(&mut unlimited, t("Sin límite", "Unlimited"))
            .changed()
        {
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
