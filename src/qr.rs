//! Código QR de un texto, para pasarlo al celular con la cámara: sin nube ni red.

use qrcodegen::{QrCode, QrCodeEcc};

/// Más que esto no cabe en un QR legible con la cámara de un teléfono.
pub const MAX_BYTES: usize = 1200;
/// Módulos de margen blanco alrededor (el estándar pide 4).
const QUIET: i32 = 4;

/// Imagen del QR, `scale` píxeles por módulo, negro sobre blanco. `None` si el texto
/// está vacío o es demasiado largo.
pub fn image(text: &str, scale: usize) -> Option<egui::ColorImage> {
    if text.is_empty() || text.len() > MAX_BYTES {
        return None;
    }
    let qr = QrCode::encode_text(text, QrCodeEcc::Medium).ok()?;
    let modules = (qr.size() + 2 * QUIET) as usize;
    let side = modules * scale;
    let mut pixels = vec![egui::Color32::WHITE; side * side];
    for y in 0..qr.size() {
        for x in 0..qr.size() {
            if !qr.get_module(x, y) {
                continue;
            }
            let (px, py) = ((x + QUIET) as usize * scale, (y + QUIET) as usize * scale);
            for row in py..py + scale {
                pixels[row * side + px..row * side + px + scale].fill(egui::Color32::BLACK);
            }
        }
    }
    Some(egui::ColorImage::new([side, side], pixels))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_square_image_with_quiet_zone() {
        let img = image("https://example.com", 4).unwrap();
        assert_eq!(img.size[0], img.size[1]);
        assert_eq!(img.size[0] % 4, 0);
        // El borde (zona tranquila) es blanco; la esquina del patrón de posición, negra.
        assert_eq!(img.pixels[0], egui::Color32::WHITE);
        let side = img.size[0];
        let corner = (QUIET as usize * 4) * side + QUIET as usize * 4;
        assert_eq!(img.pixels[corner], egui::Color32::BLACK);
    }

    #[test]
    fn rejects_empty_and_too_long() {
        assert!(image("", 4).is_none());
        assert!(image(&"x".repeat(MAX_BYTES + 1), 4).is_none());
        assert!(image("Cédula 1098765432", 4).is_some());
    }
}
