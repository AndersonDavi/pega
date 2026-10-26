//! Prueba el OCR real: la imagen va al pega-ocr.exe que Cargo compila para el test.
//! En CI se omite si la máquina no tiene el OCR de Windows con un idioma instalado.

use pega::{image, mask, ocr};
use std::path::Path;

/// Texto que el OCR lee en `tests/fixtures/<name>`, o None en una máquina de CI sin OCR.
fn read(name: &str) -> Option<String> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name);
    let img = image::decode_png(&std::fs::read(path).unwrap()).expect("PNG RGBA de 8 bits");
    let text = ocr::recognize_with(Path::new(env!("CARGO_BIN_EXE_pega-ocr")), &img);
    if text.is_none() && std::env::var_os("CI").is_some() {
        eprintln!("OCR de Windows no disponible en esta máquina de CI: se omite");
        return None;
    }
    Some(text.expect("el OCR no devolvió texto"))
}

#[test]
fn reads_text_from_an_image() {
    let Some(text) = read("ocr.png") else { return };
    assert!(text.contains("Pega"), "{text}");
    assert!(text.contains("2026"), "{text}");
}

#[test]
fn finds_a_card_in_a_small_dark_capture() {
    // Recorte de 138x25, letra clara sobre fondo oscuro: sin agrandarlo, el OCR no ve nada.
    let Some(text) = read("ocr-card-small.png") else {
        return;
    };
    assert_eq!(
        mask::find_sensitive(&text, &[]),
        Some(mask::Found::Card),
        "{text}"
    );
}
