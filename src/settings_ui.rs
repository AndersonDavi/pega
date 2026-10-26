//! Ventana de ajustes. Edita una copia; nada se aplica hasta "Guardar".

use crate::settings::{self, Hotkey, Settings, Theme};

pub struct SettingsWindow {
    draft: Settings,
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
                    });
                ui.weak("Lo fijado nunca se borra por los límites.");
                if let Some(w) = &self.warning {
                    ui.colored_label(ui.visuals().warn_fg_color, w);
                }
                ui.separator();
                ui.horizontal(|ui| {
                    if ui.button("Guardar").clicked() {
                        outcome = Outcome::Save(self.draft.clone());
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
