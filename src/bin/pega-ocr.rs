//! `pega-ocr.exe`: lee el texto de una imagen con el OCR que trae Windows y termina.
//! Va en un proceso aparte para que Windows Runtime (decenas de MB) nunca se cargue en
//! Pega: este proceso nace, responde y muere.
//!
//! Entrada (stdin): ancho u32 LE, alto u32 LE y los píxeles en BGRA.
//! Salida (stdout): el texto en UTF-8, una línea por renglón reconocido.

use std::io::{Read, Write};
use windows::Graphics::Imaging::{BitmapPixelFormat, SoftwareBitmap};
use windows::Media::Ocr::OcrEngine;
use windows::Security::Cryptography::CryptographicBuffer;

fn main() {
    let mut input = Vec::new();
    if std::io::stdin().read_to_end(&mut input).is_err() || input.len() < 8 {
        std::process::exit(2);
    }
    let width = u32::from_le_bytes(input[0..4].try_into().unwrap());
    let height = u32::from_le_bytes(input[4..8].try_into().unwrap());
    if input.len() != 8 + (width as usize * height as usize * 4) {
        std::process::exit(2);
    }
    match recognize(&input[8..], width, height) {
        Ok(text) => {
            let _ = std::io::stdout().write_all(text.as_bytes());
        }
        Err(_) => std::process::exit(1),
    }
}

fn recognize(bgra: &[u8], width: u32, height: u32) -> windows::core::Result<String> {
    let buffer = CryptographicBuffer::CreateFromByteArray(bgra)?;
    let bitmap = SoftwareBitmap::CreateCopyFromBuffer(
        &buffer,
        BitmapPixelFormat::Bgra8,
        width as i32,
        height as i32,
    )?;
    // Idiomas del perfil de Windows (español, inglés…).
    let engine = OcrEngine::TryCreateFromUserProfileLanguages()?;
    let result = engine.RecognizeAsync(&bitmap)?.join()?;
    let lines: Vec<String> = result
        .Lines()?
        .into_iter()
        .filter_map(|l| l.Text().ok())
        .map(|t| t.to_string())
        .collect();
    Ok(lines.join("\n"))
}
