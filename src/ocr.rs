//! Texto de las capturas con el OCR de Windows, en `pega-ocr.exe` (proceso aparte que nace,
//! responde y muere: Windows Runtime nunca se carga en Pega). Desactivado por defecto.

use crate::image::{self, Rgba};
use std::io::{Read, Write};
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;

/// El OCR de Windows no acepta imágenes más grandes (su máximo ronda los 2600 px).
pub const MAX_SIDE: u32 = 2500;
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
/// Un reconocimiento a la vez, aunque lleguen varias capturas seguidas.
static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

/// `pega-ocr.exe` junto a este ejecutable.
pub fn helper_path() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?.with_file_name("pega-ocr.exe");
    exe.exists().then_some(exe)
}

/// Lo que se manda por stdin: ancho, alto y píxeles BGRA.
pub fn payload(img: &Rgba) -> Vec<u8> {
    let small = image::fit(img, MAX_SIDE, MAX_SIDE);
    let mut out = Vec::with_capacity(8 + small.pixels.len());
    out.extend_from_slice(&small.width.to_le_bytes());
    out.extend_from_slice(&small.height.to_le_bytes());
    for p in small.pixels.chunks_exact(4) {
        out.extend_from_slice(&[p[2], p[1], p[0], p[3]]);
    }
    out
}

/// Texto reconocido, o `None` si no hay ayudante, falla o no encontró texto.
pub fn recognize_with(helper: &Path, img: &Rgba) -> Option<String> {
    let _turn = ONE_AT_A_TIME.lock().ok()?;
    let mut child = Command::new(helper)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .ok()?;
    child.stdin.take()?.write_all(&payload(img)).ok()?;
    let mut text = String::new();
    child.stdout.take()?.read_to_string(&mut text).ok()?;
    let ok = child.wait().ok()?.success();
    let text = text.trim();
    (ok && !text.is_empty()).then(|| text.to_owned())
}

pub fn recognize(img: &Rgba) -> Option<String> {
    recognize_with(&helper_path()?, img)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn payload_is_bgra_with_size_header() {
        let img = Rgba {
            width: 2,
            height: 1,
            pixels: vec![1, 2, 3, 4, 5, 6, 7, 8],
        };
        assert_eq!(
            payload(&img),
            [2, 0, 0, 0, 1, 0, 0, 0, 3, 2, 1, 4, 7, 6, 5, 8]
        );
    }

    #[test]
    fn big_images_are_reduced_to_the_ocr_limit() {
        let img = Rgba {
            width: 5000,
            height: 100,
            pixels: vec![255; 5000 * 100 * 4],
        };
        let p = payload(&img);
        assert_eq!(u32::from_le_bytes(p[0..4].try_into().unwrap()), MAX_SIDE);
        assert_eq!(u32::from_le_bytes(p[4..8].try_into().unwrap()), 50);
    }
}
