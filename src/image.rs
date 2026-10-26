//! Imágenes del portapapeles: DIB de Windows <-> RGBA <-> PNG, miniaturas y hash.

use std::io::Cursor;

pub struct Rgba {
    pub width: u32,
    pub height: u32,
    /// 4 bytes por píxel, fila por fila de arriba hacia abajo.
    pub pixels: Vec<u8>,
}

/// Caja máxima de la miniatura que se ve en el historial.
pub const THUMB_W: u32 = 120;
pub const THUMB_H: u32 = 72;
/// Más grande que 8K se ignora.
const MAX_PIXELS: u64 = 40_000_000;

/// FNV-1a: estable entre versiones, sirve para no guardar dos veces la misma imagen.
pub fn hash(img: &Rgba) -> i64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let dims = [img.width.to_le_bytes(), img.height.to_le_bytes()].concat();
    for &b in dims.iter().chain(&img.pixels) {
        h ^= b as u64;
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h as i64
}

pub fn encode_png(img: &Rgba, compression: png::Compression) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let mut enc = png::Encoder::new(&mut out, img.width, img.height);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.set_compression(compression);
    let mut writer = enc.write_header().ok()?;
    writer.write_image_data(&img.pixels).ok()?;
    writer.finish().ok()?;
    Some(out)
}

/// Solo decodifica los PNG que guarda Pega (RGBA de 8 bits).
pub fn decode_png(bytes: &[u8]) -> Option<Rgba> {
    let mut reader = png::Decoder::new(Cursor::new(bytes)).read_info().ok()?;
    let mut pixels = vec![0; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut pixels).ok()?;
    if info.color_type != png::ColorType::Rgba || info.bit_depth != png::BitDepth::Eight {
        return None;
    }
    pixels.truncate(info.buffer_size());
    Some(Rgba {
        width: info.width,
        height: info.height,
        pixels,
    })
}

/// Miniatura del historial: cabe en THUMB_W x THUMB_H.
pub fn thumbnail(img: &Rgba) -> Rgba {
    fit(img, THUMB_W, THUMB_H)
}

/// Reduce promediando bloques (sin dientes de sierra) hasta caber en `max_w` x `max_h`.
/// Si ya cabe, devuelve una copia del mismo tamaño.
pub fn fit(img: &Rgba, max_w: u32, max_h: u32) -> Rgba {
    let scale = (max_w as f64 / img.width as f64)
        .min(max_h as f64 / img.height as f64)
        .min(1.0);
    let tw = ((img.width as f64 * scale).round() as u32).max(1);
    let th = ((img.height as f64 * scale).round() as u32).max(1);
    let mut pixels = Vec::with_capacity((tw * th * 4) as usize);
    // En u64: con imágenes muy anchas (40 millones x 1) `t * ancho` no cabe en u32.
    let at = |t: u32, len: u32, parts: u32| (t as u64 * len as u64 / parts as u64) as u32;
    for ty in 0..th {
        let y0 = at(ty, img.height, th);
        let y1 = at(ty + 1, img.height, th).max(y0 + 1);
        for tx in 0..tw {
            let x0 = at(tx, img.width, tw);
            let x1 = at(tx + 1, img.width, tw).max(x0 + 1);
            let mut sum = [0u64; 4];
            for y in y0..y1 {
                let row = (y * img.width) as usize * 4;
                for x in x0..x1 {
                    let p = row + x as usize * 4;
                    for (s, &v) in sum.iter_mut().zip(&img.pixels[p..p + 4]) {
                        *s += v as u64;
                    }
                }
            }
            let n = ((y1 - y0) * (x1 - x0)) as u64;
            pixels.extend(sum.iter().map(|s| (s / n) as u8));
        }
    }
    Rgba {
        width: tw,
        height: th,
        pixels,
    }
}

/// Agranda `k` veces con interpolación bilineal: el OCR de Windows no ve texto de menos
/// de ~20 px de alto, y en una captura chica la letra suele medir 10-12 px.
pub fn upscale(img: &Rgba, k: u32) -> Rgba {
    let (sw, sh) = (img.width as usize, img.height as usize);
    let k = k.max(1) as usize;
    let (w, h) = (sw * k, sh * k);
    let mut pixels = Vec::with_capacity(w * h * 4);
    // Posición en la imagen original del centro del píxel nuevo, y vecinos para mezclar.
    let source = |t: usize, len: usize| {
        let f = ((t as f32 + 0.5) / k as f32 - 0.5).max(0.0);
        let a = (f as usize).min(len - 1);
        (a, (a + 1).min(len - 1), f - a as f32)
    };
    let px = |x: usize, y: usize, c: usize| img.pixels[(y * sw + x) * 4 + c] as f32;
    for y in 0..h {
        let (y0, y1, ty) = source(y, sh);
        for x in 0..w {
            let (x0, x1, tx) = source(x, sw);
            for c in 0..4 {
                let top = px(x0, y0, c) * (1.0 - tx) + px(x1, y0, c) * tx;
                let bottom = px(x0, y1, c) * (1.0 - tx) + px(x1, y1, c) * tx;
                pixels.push((top * (1.0 - ty) + bottom * ty).round() as u8);
            }
        }
    }
    Rgba {
        width: w as u32,
        height: h as u32,
        pixels,
    }
}

fn u16_at(b: &[u8], i: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(i..i + 2)?.try_into().ok()?))
}

fn u32_at(b: &[u8], i: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(i..i + 4)?.try_into().ok()?))
}

/// Bytes de cabecera que alcanzan para validar un DIB (BITMAPINFOHEADER + máscaras).
pub const DIB_HEADER_PEEK: usize = 52;

/// Lo que dice la cabecera de un DIB, ya validado.
struct DibLayout {
    width: u32,
    height: u32,
    bottom_up: bool,
    bytes_pp: usize,
    stride: usize,
    offset: usize,
    /// Rojo y azul invertidos (máscaras de BI_BITFIELDS en orden RGB en vez de BGR).
    swap_rb: bool,
}

impl DibLayout {
    fn len(&self) -> usize {
        self.offset + self.stride * self.height as usize
    }
}

fn dib_layout(b: &[u8]) -> Option<DibLayout> {
    let header_size = u32_at(b, 0)? as usize;
    let width = u32_at(b, 4)? as i32;
    let raw_height = u32_at(b, 8)? as i32;
    let bit_count = u16_at(b, 14)?;
    let compression = u32_at(b, 16)?;
    let colors_used = u32_at(b, 32)? as usize;
    const BI_RGB: u32 = 0;
    const BI_BITFIELDS: u32 = 3;
    if header_size < 40 || width <= 0 || raw_height == 0 || !(bit_count == 24 || bit_count == 32) {
        return None;
    }
    let bitfields = compression == BI_BITFIELDS && bit_count == 32;
    if compression != BI_RGB && !bitfields {
        return None;
    }
    let (width, height) = (width as u32, raw_height.unsigned_abs());
    if width as u64 * height as u64 > MAX_PIXELS {
        return None;
    }
    // Las máscaras van en el byte 40: justo después de la cabecera clásica de 40 bytes, o
    // dentro de las cabeceras V4/V5. Solo se aceptan BGR (lo normal) y RGB (se invierte).
    let mut swap_rb = false;
    if bitfields {
        let (r, g, bl) = (u32_at(b, 40)?, u32_at(b, 44)?, u32_at(b, 48)?);
        match (r, g, bl) {
            (0x00FF_0000, 0x0000_FF00, 0x0000_00FF) => {}
            (0x0000_00FF, 0x0000_FF00, 0x00FF_0000) => swap_rb = true,
            _ => return None,
        }
    }
    let masks = if bitfields && header_size == 40 {
        12
    } else {
        0
    };
    Some(DibLayout {
        width,
        height,
        bottom_up: raw_height > 0,
        bytes_pp: bit_count as usize / 8,
        stride: (width as usize * bit_count as usize).div_ceil(32) * 4,
        offset: header_size
            .checked_add(masks)?
            .checked_add(colors_used.checked_mul(4)?)?,
        swap_rb,
    })
}

/// Tamaño total que necesita el DIB según su cabecera, o None si no es uno que Pega lea.
/// Sirve para no copiar del portapapeles más de lo necesario (ni imágenes inválidas).
pub fn dib_len(header: &[u8]) -> Option<usize> {
    dib_layout(header).map(|l| l.len())
}

/// CF_DIB (BITMAPINFOHEADER + píxeles) de 24 o 32 bits a RGBA.
pub fn from_dib(b: &[u8]) -> Option<Rgba> {
    let l = dib_layout(b)?;
    let data = b.get(l.offset..l.len())?;
    let (width, height) = (l.width as usize, l.height as usize);
    let (r, bl) = if l.swap_rb { (0, 2) } else { (2, 0) };
    let mut pixels = Vec::with_capacity(width * height * 4);
    let mut any_alpha = false;
    for y in 0..height {
        // Altura positiva = filas de abajo hacia arriba.
        let src_y = if l.bottom_up { height - 1 - y } else { y };
        let row = &data[src_y * l.stride..];
        for x in 0..width {
            let p = &row[x * l.bytes_pp..];
            let a = if l.bytes_pp == 4 { p[3] } else { 255 };
            any_alpha |= a != 0;
            pixels.extend_from_slice(&[p[r], p[1], p[bl], a]);
        }
    }
    // Muchas apps dejan el alfa en 0 aunque la imagen sea opaca.
    if !any_alpha {
        pixels.chunks_exact_mut(4).for_each(|p| p[3] = 255);
    }
    Some(Rgba {
        width: l.width,
        height: l.height,
        pixels,
    })
}
/// RGBA a CF_DIB de 32 bits, de abajo hacia arriba (lo que más apps aceptan).
pub fn to_dib(img: &Rgba) -> Vec<u8> {
    let mut out = Vec::with_capacity(40 + img.pixels.len());
    out.extend_from_slice(&40u32.to_le_bytes());
    out.extend_from_slice(&(img.width as i32).to_le_bytes());
    out.extend_from_slice(&(img.height as i32).to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&32u16.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes()); // BI_RGB
    out.extend_from_slice(&(img.pixels.len() as u32).to_le_bytes());
    out.extend_from_slice(&[0u8; 16]); // resolución y paleta
    let row_len = img.width as usize * 4;
    for row in img.pixels.chunks_exact(row_len).rev() {
        for p in row.chunks_exact(4) {
            out.extend_from_slice(&[p[2], p[1], p[0], p[3]]);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Rgba {
        // 3x2: colores distintos y un píxel semitransparente.
        let pixels = vec![
            255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, //
            10, 20, 30, 128, 200, 200, 200, 255, 0, 0, 0, 255,
        ];
        Rgba {
            width: 3,
            height: 2,
            pixels,
        }
    }

    #[test]
    fn dib_roundtrip_keeps_pixels_and_alpha() {
        let img = sample();
        let back = from_dib(&to_dib(&img)).unwrap();
        assert_eq!((back.width, back.height), (3, 2));
        assert_eq!(back.pixels, img.pixels);
    }

    #[test]
    fn dib_without_alpha_becomes_opaque() {
        // 24 bits, abajo hacia arriba, filas alineadas a 4 bytes: 1x1 rojo.
        let mut dib = vec![0u8; 40];
        dib[0] = 40;
        dib[4] = 1; // ancho
        dib[8] = 1; // alto
        dib[12] = 1; // planos
        dib[14] = 24; // bits
        dib.extend_from_slice(&[0, 0, 255, 0]); // BGR + relleno
        let img = from_dib(&dib).unwrap();
        assert_eq!(img.pixels, [255, 0, 0, 255]);
    }

    /// DIB de 32 bits con BI_BITFIELDS y las máscaras dadas (R, G, B): 1x1.
    fn bitfields(r: u32, g: u32, b: u32, pixel: [u8; 4]) -> Vec<u8> {
        let mut dib = vec![0u8; 40];
        dib[0] = 40;
        dib[4] = 1;
        dib[8] = 1;
        dib[12] = 1;
        dib[14] = 32;
        dib[16] = 3; // BI_BITFIELDS
        for m in [r, g, b] {
            dib.extend_from_slice(&m.to_le_bytes());
        }
        dib.extend_from_slice(&pixel);
        dib
    }

    #[test]
    fn bitfield_masks_decide_channel_order() {
        // BGR (lo normal) y RGB (se invierte): ambos dan rojo.
        let bgr = bitfields(0x00FF_0000, 0x0000_FF00, 0x0000_00FF, [0, 0, 255, 255]);
        assert_eq!(from_dib(&bgr).unwrap().pixels, [255, 0, 0, 255]);
        let rgb = bitfields(0x0000_00FF, 0x0000_FF00, 0x00FF_0000, [255, 0, 0, 255]);
        assert_eq!(from_dib(&rgb).unwrap().pixels, [255, 0, 0, 255]);
        // Máscaras raras (16 bits empaquetados, etc.): no se adivina.
        assert!(from_dib(&bitfields(0xF800, 0x07E0, 0x001F, [0; 4])).is_none());
    }

    #[test]
    fn dib_len_validates_before_copying() {
        let dib = to_dib(&sample());
        assert_eq!(dib_len(&dib[..DIB_HEADER_PEEK]), Some(dib.len()));
        // 60000 x 60000: se rechaza solo con la cabecera, sin copiar 14 GB.
        let mut huge = dib[..40].to_vec();
        huge[4..8].copy_from_slice(&60_000i32.to_le_bytes());
        huge[8..12].copy_from_slice(&60_000i32.to_le_bytes());
        assert_eq!(dib_len(&huge), None);
        assert_eq!(dib_len(&dib[..20]), None, "cabecera incompleta");
    }

    #[test]
    fn very_wide_images_scale_without_overflow() {
        // 2 000 000 x 2: `x * ancho` no cabe en u32.
        let img = Rgba {
            width: 2_000_000,
            height: 2,
            pixels: vec![200; 2_000_000 * 2 * 4],
        };
        let t = fit(&img, 2500, 2500);
        assert_eq!(t.width, 2500);
        assert!(t.pixels.iter().all(|&v| v == 200), "sin basura");
    }

    #[test]
    fn png_roundtrip_and_hash() {
        let img = sample();
        let back = decode_png(&encode_png(&img, png::Compression::Fast).unwrap()).unwrap();
        assert_eq!(back.pixels, img.pixels);
        assert_eq!(hash(&img), hash(&back), "mismos píxeles, mismo hash");
        let mut other = sample();
        other.pixels[0] = 254;
        assert_ne!(hash(&img), hash(&other));
    }

    #[test]
    fn fit_keeps_aspect_and_never_enlarges() {
        let img = Rgba {
            width: 400,
            height: 100,
            pixels: vec![255; 400 * 100 * 4],
        };
        let t = thumbnail(&img);
        assert_eq!((t.width, t.height), (THUMB_W, 30));
        let small = fit(&sample(), 100, 100);
        assert_eq!((small.width, small.height), (3, 2));
    }
}
