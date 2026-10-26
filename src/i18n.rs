//! Español / inglés. Cada texto lleva sus dos versiones en el mismo sitio donde se usa:
//! no hay archivos de traducción que cargar ni tablas en memoria.

use crate::settings::UiLang;
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use windows_sys::Win32::Globalization::GetUserDefaultUILanguage;

static ENGLISH: AtomicBool = AtomicBool::new(false);

pub fn english() -> bool {
    ENGLISH.load(Relaxed)
}

/// Aplica el idioma de los ajustes; "Sistema" sigue el idioma de Windows.
pub fn apply(lang: UiLang) {
    let english = match lang {
        UiLang::Spanish => false,
        UiLang::English => true,
        // LANG_SPANISH = 0x0A en los 10 bits bajos del LANGID.
        UiLang::System => (unsafe { GetUserDefaultUILanguage() } & 0x3FF) != 0x0A,
    };
    ENGLISH.store(english, Relaxed);
}

/// Texto según el idioma de la interfaz.
pub fn t(es: &'static str, en: &'static str) -> &'static str {
    if english() { en } else { es }
}

/// Como `format!`, con una plantilla por idioma: `tr!("Hola {nombre}", "Hello {nombre}")`.
#[macro_export]
macro_rules! tr {
    ($es:literal, $en:literal $(,)?) => {
        if $crate::i18n::english() { format!($en) } else { format!($es) }
    };
    ($es:literal, $en:literal, $($arg:tt)*) => {
        if $crate::i18n::english() { format!($en, $($arg)*) } else { format!($es, $($arg)*) }
    };
}
