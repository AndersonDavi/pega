//! Bloqueo con PIN. Es un candado de la interfaz: los datos ya están cifrados en
//! disco con DPAPI; esto evita que alguien frente a tu PC desbloqueado los vea.
//!
//! No hay temporizador corriendo: se pide un único repintado para cuando se cumpla
//! el tiempo sin uso, y si la ventana estaba oculta se revisa al volver a mostrarla.

use crate::settings::Settings;
use std::time::Duration;

/// Intentos fallidos antes de hacer esperar.
const MAX_FAILURES: u32 = 5;
const WAIT_SECS: f64 = 30.0;

pub struct Lock {
    locked: bool,
    /// Último momento con actividad (segundos de egui).
    last_input: f64,
    entry: String,
    error: Option<String>,
    failures: u32,
    retry_at: f64,
}

impl Lock {
    /// Con PIN configurado, Pega arranca bloqueado.
    pub fn new(settings: &Settings) -> Self {
        Self {
            locked: !settings.pin.is_empty(),
            last_input: 0.0,
            entry: String::new(),
            error: None,
            failures: 0,
            retry_at: 0.0,
        }
    }

    pub fn lock_now(&mut self) {
        self.locked = true;
        self.entry.clear();
    }

    /// Se llama al principio de cada cuadro, antes de dibujar contenido. Devuelve si está bloqueado.
    pub fn update(&mut self, ctx: &egui::Context, settings: &Settings) -> bool {
        let now = ctx.input(|i| i.time);
        if settings.pin.is_empty() {
            self.locked = false;
            self.last_input = now;
            return false;
        }
        if !self.locked && settings.lock_minutes > 0 {
            let limit = settings.lock_minutes as f64 * 60.0;
            let idle = now - self.last_input;
            if idle >= limit {
                self.lock_now();
            } else {
                ctx.request_repaint_after(Duration::from_secs_f64(limit - idle + 0.25));
            }
        }
        let active = ctx.input(|i| !i.events.is_empty() || i.pointer.is_moving());
        if active && !self.locked {
            self.last_input = now;
        }
        self.locked
    }

    pub fn show(&mut self, ui: &mut egui::Ui, settings: &Settings) {
        let now = ui.input(|i| i.time);
        egui::CentralPanel::default().show_inside(ui, |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space(ui.available_height() * 0.3);
                ui.heading("Pega está bloqueado");
                ui.add_space(12.0);
                let waiting = now < self.retry_at;
                let field = ui.add_enabled(
                    !waiting,
                    egui::TextEdit::singleline(&mut self.entry)
                        .password(true)
                        .hint_text("PIN")
                        .desired_width(160.0),
                );
                if !waiting {
                    field.request_focus();
                }
                let enter = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                let clicked = ui
                    .add_enabled(!waiting, egui::Button::new("Desbloquear"))
                    .clicked();
                if (enter || clicked) && !waiting {
                    self.try_unlock(settings, now);
                }
                if waiting {
                    let secs = (self.retry_at - now).ceil();
                    ui.colored_label(
                        ui.visuals().warn_fg_color,
                        format!("Demasiados intentos. Espera {secs} s."),
                    );
                    ui.ctx().request_repaint_after(Duration::from_secs(1));
                } else if let Some(e) = &self.error {
                    ui.colored_label(ui.visuals().error_fg_color, e);
                }
            });
        });
    }

    fn try_unlock(&mut self, settings: &Settings, now: f64) {
        if self.entry == settings.pin {
            self.locked = false;
            self.failures = 0;
            self.error = None;
            self.last_input = now;
        } else {
            self.failures += 1;
            self.error = Some("PIN incorrecto".into());
            if self.failures >= MAX_FAILURES {
                self.failures = 0;
                self.retry_at = now + WAIT_SECS;
            }
        }
        self.entry.clear();
    }
}
