//! Ajustes. Se guardan como texto "clave=valor" cifrado entero con DPAPI, porque
//! algunos (palabras a enmascarar, PIN) son tan sensibles como el contenido.

/// Separa los elementos de una lista dentro de un valor (no aparece en texto normal).
const LIST_SEP: char = '\u{1f}';

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Hotkey {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub win: bool,
    /// Código de tecla virtual de Windows (VK_*).
    pub vk: u16,
}

impl Default for Hotkey {
    fn default() -> Self {
        Self {
            ctrl: true,
            alt: true,
            shift: false,
            win: false,
            vk: 0x56,
        } // Ctrl+Alt+V
    }
}

impl Hotkey {
    pub fn label(&self) -> String {
        let mut parts = Vec::new();
        if self.ctrl {
            parts.push("Ctrl".to_owned());
        }
        if self.alt {
            parts.push("Alt".to_owned());
        }
        if self.shift {
            parts.push("Shift".to_owned());
        }
        if self.win {
            parts.push("Win".to_owned());
        }
        parts.push(vk_name(self.vk));
        parts.join("+")
    }

    fn encode(&self) -> String {
        format!(
            "{},{},{},{},{}",
            self.ctrl as u8, self.alt as u8, self.shift as u8, self.win as u8, self.vk
        )
    }

    fn decode(s: &str) -> Option<Self> {
        let v: Vec<u16> = s
            .split(',')
            .map(|p| p.parse().ok())
            .collect::<Option<_>>()?;
        let [c, a, s, w, vk] = v[..] else { return None };
        Some(Self {
            ctrl: c == 1,
            alt: a == 1,
            shift: s == 1,
            win: w == 1,
            vk,
        })
    }

    /// Una combinación sin Ctrl, Alt ni Win robaría la tecla al resto de programas.
    pub fn is_valid(&self) -> bool {
        (self.ctrl || self.alt || self.win) && self.vk != 0
    }
}

/// Tecla de egui a código VK de Windows (solo las que tienen sentido en un atajo).
pub fn vk_from_egui(key: egui::Key) -> Option<u16> {
    use egui::Key::*;
    let letters = [
        A, B, C, D, E, F, G, H, I, J, K, L, M, N, O, P, Q, R, S, T, U, V, W, X, Y, Z,
    ];
    if let Some(i) = letters.iter().position(|k| *k == key) {
        return Some(0x41 + i as u16);
    }
    let digits = [Num0, Num1, Num2, Num3, Num4, Num5, Num6, Num7, Num8, Num9];
    if let Some(i) = digits.iter().position(|k| *k == key) {
        return Some(0x30 + i as u16);
    }
    let fkeys = [F1, F2, F3, F4, F5, F6, F7, F8, F9, F10, F11, F12];
    if let Some(i) = fkeys.iter().position(|k| *k == key) {
        return Some(0x70 + i as u16);
    }
    Some(match key {
        Space => 0x20,
        Insert => 0x2D,
        Home => 0x24,
        End => 0x23,
        PageUp => 0x21,
        PageDown => 0x22,
        Period => 0xBE,
        Comma => 0xBC,
        Minus => 0xBD,
        _ => return None,
    })
}

fn vk_name(vk: u16) -> String {
    match vk {
        0x41..=0x5A | 0x30..=0x39 => char::from(vk as u8).to_string(),
        0x70..=0x7B => format!("F{}", vk - 0x6F),
        0x20 => "Espacio".into(),
        0x2D => "Insert".into(),
        0x24 => "Inicio".into(),
        0x23 => "Fin".into(),
        0x21 => "RePág".into(),
        0x22 => "AvPág".into(),
        0xBE => ".".into(),
        0xBC => ",".into(),
        0xBD => "-".into(),
        _ => format!("0x{vk:02X}"),
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Theme {
    System,
    Dark,
    Light,
}

#[derive(Clone, PartialEq)]
pub struct Settings {
    pub hotkey: Hotkey,
    pub theme: Theme,
    /// 0 = sin límite.
    pub max_texts: usize,
    /// 0 = sin límite.
    pub max_images: usize,
    /// Borra del historial lo no fijado con más de estos días. 0 = nunca.
    pub keep_days: u32,
    /// Ejecutables cuyas copias no se guardan, en minúsculas ("keepass.exe").
    pub excluded_apps: Vec<String>,
    /// Palabras que se muestran como puntos.
    pub mask_words: Vec<String>,
    /// Oculta números de tarjeta (menos los últimos 4 dígitos).
    pub mask_cards: bool,
    /// PIN para desbloquear la ventana; vacío = sin bloqueo.
    pub pin: String,
    /// Minutos sin usar Pega para bloquearlo. 0 = solo al iniciar.
    pub lock_minutes: u32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            hotkey: Hotkey::default(),
            theme: Theme::System,
            max_texts: 1000,
            max_images: 100,
            keep_days: 0,
            excluded_apps: Vec::new(),
            mask_words: Vec::new(),
            mask_cards: false,
            pin: String::new(),
            lock_minutes: 5,
        }
    }
}

impl Settings {
    pub fn encode(&self) -> String {
        let theme = match self.theme {
            Theme::System => "system",
            Theme::Dark => "dark",
            Theme::Light => "light",
        };
        [
            format!("hotkey={}", self.hotkey.encode()),
            format!("theme={theme}"),
            format!("max_texts={}", self.max_texts),
            format!("max_images={}", self.max_images),
            format!("keep_days={}", self.keep_days),
            format!("excluded_apps={}", encode_list(&self.excluded_apps)),
            format!("mask_words={}", encode_list(&self.mask_words)),
            format!("mask_cards={}", self.mask_cards as u8),
            format!("pin={}", self.pin),
            format!("lock_minutes={}", self.lock_minutes),
        ]
        .join("\n")
    }

    /// Claves desconocidas o dañadas se ignoran y quedan en su valor por defecto.
    pub fn decode(text: &str) -> Self {
        let mut s = Self::default();
        for line in text.lines() {
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            match key {
                "hotkey" => {
                    if let Some(h) = Hotkey::decode(value).filter(Hotkey::is_valid) {
                        s.hotkey = h;
                    }
                }
                "theme" => {
                    s.theme = match value {
                        "dark" => Theme::Dark,
                        "light" => Theme::Light,
                        _ => Theme::System,
                    }
                }
                "max_texts" => s.max_texts = value.parse().unwrap_or(s.max_texts),
                "max_images" => s.max_images = value.parse().unwrap_or(s.max_images),
                "keep_days" => s.keep_days = value.parse().unwrap_or(s.keep_days),
                "excluded_apps" => s.excluded_apps = decode_list(value),
                "mask_words" => s.mask_words = decode_list(value),
                "mask_cards" => s.mask_cards = value == "1",
                "pin" => s.pin = value.chars().filter(char::is_ascii_digit).collect(),
                "lock_minutes" => s.lock_minutes = value.parse().unwrap_or(s.lock_minutes),
                _ => {}
            }
        }
        s
    }
}

fn encode_list(items: &[String]) -> String {
    items.join(&LIST_SEP.to_string())
}

fn decode_list(value: &str) -> Vec<String> {
    value
        .split(LIST_SEP)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect()
}

impl Settings {
    pub fn is_excluded(&self, app: &str) -> bool {
        self.excluded_apps
            .iter()
            .any(|a| a.eq_ignore_ascii_case(app))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let s = Settings {
            hotkey: Hotkey {
                ctrl: true,
                alt: false,
                shift: true,
                win: false,
                vk: 0x42,
            },
            theme: Theme::Light,
            max_texts: 0,
            max_images: 7,
            keep_days: 30,
            excluded_apps: vec!["keepass.exe".into(), "1password.exe".into()],
            mask_words: vec!["clave secreta".into(), "Bancolombia".into()],
            mask_cards: true,
            pin: "4321".into(),
            lock_minutes: 10,
        };
        assert!(Settings::decode(&s.encode()) == s);
    }

    #[test]
    fn damaged_values_fall_back_to_defaults() {
        let s = Settings::decode("hotkey=basura\nmax_texts=x\nnada\nkeep_days=3");
        assert!(s.hotkey == Hotkey::default());
        assert_eq!(s.max_texts, 1000);
        assert_eq!(s.keep_days, 3);
    }

    #[test]
    fn hotkey_needs_a_modifier() {
        assert!(
            !Hotkey {
                ctrl: false,
                alt: false,
                shift: true,
                win: false,
                vk: 0x41
            }
            .is_valid()
        );
        assert_eq!(Hotkey::default().label(), "Ctrl+Alt+V");
    }
}
