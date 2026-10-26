//! Texto de las capturas con el OCR de Windows, en `pega-ocr.exe` (proceso aparte que nace,
//! responde y muere: Windows Runtime nunca se carga en Pega). Desactivado por defecto.

use crate::image::{self, Rgba};
use std::io::{Read, Write};
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Mutex, mpsc};
use std::thread;
use std::time::Duration;

/// El OCR de Windows no acepta imágenes más grandes (su máximo ronda los 2600 px).
pub const MAX_SIDE: u32 = 2500;
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const TIMEOUT: Duration = Duration::from_secs(30);
/// Un reconocimiento a la vez, aunque lleguen varias capturas seguidas.
static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

/// `pega-ocr.exe` junto a este ejecutable.
pub fn helper_path() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?.with_file_name("pega-ocr.exe");
    exe.exists().then_some(exe)
}

/// Cuántas veces se agranda una captura antes del OCR: las chicas (un recorte de una línea,
/// un número) tienen letra demasiado pequeña para que Windows la vea. Medido: una línea de
/// 25 px de alto no se lee; agrandada 2 o 3 veces, sí.
fn upscale_factor(img: &Rgba) -> u32 {
    match img.width.max(img.height) {
        0..=600 => 3,
        601..=1250 => 2,
        _ => 1,
    }
}

/// Lo que se manda por stdin: ancho, alto y píxeles BGRA.
pub fn payload(img: &Rgba) -> Vec<u8> {
    let small = match upscale_factor(img) {
        1 => image::fit(img, MAX_SIDE, MAX_SIDE),
        k => image::upscale(img, k),
    };
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
    recognize_payload(helper, payload(img))
}

/// Igual, con el `payload` ya armado (así la imagen completa no espera en memoria su turno).
/// Si el OCR de Windows se cuelga, a los 30 s se cierra el ayudante y se sigue.
pub fn recognize_payload(helper: &Path, data: Vec<u8>) -> Option<String> {
    let _turn = ONE_AT_A_TIME.lock().ok()?;
    let mut child = Command::new(helper)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .ok()?;
    let (mut stdin, mut stdout) = (child.stdin.take()?, child.stdout.take()?);
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let _ = stdin.write_all(&data);
        drop(stdin);
        let mut text = String::new();
        let ok = stdout.read_to_string(&mut text).is_ok();
        let _ = tx.send(ok.then_some(text));
    });
    match rx.recv_timeout(TIMEOUT) {
        Ok(Some(text)) => {
            let ok = child.wait().ok()?.success();
            let text = text.trim();
            (ok && !text.is_empty()).then(|| text.to_owned())
        }
        _ => {
            let _ = child.kill();
            let _ = child.wait();
            None
        }
    }
}

pub fn recognize(img: &Rgba) -> Option<String> {
    recognize_with(&helper_path()?, img)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn payload_is_bgra_with_size_header() {
        // 1x1 se agranda a 3x3 (captura chica): cabecera con 3 y 3, píxeles en BGRA.
        let img = Rgba {
            width: 1,
            height: 1,
            pixels: vec![1, 2, 3, 4],
        };
        let p = payload(&img);
        assert_eq!(&p[..8], [3, 0, 0, 0, 3, 0, 0, 0]);
        assert_eq!(p.len(), 8 + 3 * 3 * 4);
        assert!(p[8..].chunks(4).all(|px| px == [3, 2, 1, 4]));
    }

    #[test]
    fn small_captures_are_enlarged_for_the_ocr() {
        let header = |w: u32, h: u32| {
            let p = payload(&Rgba {
                width: w,
                height: h,
                pixels: vec![0; (w * h * 4) as usize],
            });
            (
                u32::from_le_bytes(p[0..4].try_into().unwrap()),
                u32::from_le_bytes(p[4..8].try_into().unwrap()),
            )
        };
        assert_eq!(header(138, 25), (414, 75), "un recorte de una línea: x3");
        assert_eq!(header(1000, 600), (2000, 1200), "mediana: x2");
        assert_eq!(header(1920, 1080), (1920, 1080), "pantalla completa: igual");
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
