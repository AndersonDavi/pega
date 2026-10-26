//! Prueba el OCR real: la imagen va al pega-ocr.exe que Cargo compila para el test.
//! En CI se omite si la máquina no tiene el OCR de Windows con un idioma instalado.

use pega::{image, ocr};
use std::path::Path;

#[test]
fn reads_text_from_an_image() {
    let png = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/ocr.png"
    ))
    .unwrap();
    let img = image::decode_png(&png).expect("PNG RGBA de 8 bits");
    let Some(text) = ocr::recognize_with(Path::new(env!("CARGO_BIN_EXE_pega-ocr")), &img) else {
        if std::env::var_os("CI").is_some() {
            eprintln!("OCR de Windows no disponible en esta máquina de CI: se omite");
            return;
        }
        panic!("el OCR no devolvió texto");
    };
    assert!(text.contains("Pega"), "{text}");
    assert!(text.contains("2026"), "{text}");
}
