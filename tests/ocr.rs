//! Prueba el OCR real: la imagen va al pega-ocr.exe que Cargo compila para el test.

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
    let text = ocr::recognize_with(Path::new(env!("CARGO_BIN_EXE_pega-ocr")), &img)
        .expect("el OCR devolvió texto");
    assert!(text.contains("Pega"), "{text}");
    assert!(text.contains("2026"), "{text}");
}
