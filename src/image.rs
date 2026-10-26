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
    for ty in 0..th {
        let y0 = ty * img.height / th;
        let y1 = ((ty + 1) * img.height / th).max(y0 + 1);
        for tx in 0..tw {
            let x0 = tx * img.width / tw;
            let x1 = ((tx + 1) * img.width / tw).max(x0 + 1);
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

fn u16_at(b: &[u8], i: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(i..i + 2)?.try_into().ok()?))
}

fn u32_at(b: &[u8], i: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(i..i + 4)?.try_into().ok()?))
}

/// CF_DIB (BITMAPINFOHEADER + píxeles) de 24 o 32 bits a RGBA.
pub fn from_dib(b: &[u8]) -> Option<Rgba> {
    let header_size = u32_at(b, 0)? as usize;
    let width = u32_at(b, 4)? as i32;
    let raw_height = u32_at(b, 8)? as i32;
    let bit_count = u16_at(b, 14)?;
    let compression = u32_at(b, 16)?;
    let colors_used = u32_at(b, 32)? as usize;
    const BI_RGB: u32 = 0;
    const BI_BITFIELDS: u32 = 3;
    if width <= 0 || raw_height == 0 || !(bit_count == 24 || bit_count == 32) {
        return None;
    }
    if compression != BI_RGB && !(compression == BI_BITFIELDS && bit_count == 32) {
        return None;
    }
    let (width, height) = (width as u32, raw_height.unsigned_abs());
    if width as u64 * height as u64 > MAX_PIXELS {
        return None;
    }
    // Con BI_BITFIELDS y cabecera clásica de 40 bytes, las 3 máscaras van justo después.
    let masks = if compression == BI_BITFIELDS && header_size == 40 {
        12
    } else {
        0
    };
    let offset = header_size + masks + colors_used * 4;
    let bytes_pp = bit_count as usize / 8;
    let stride = (width as usize * bit_count as usize).div_ceil(32) * 4;
    let data = b.get(offset..offset + stride * height as usize)?;
    // Altura positiva = filas de abajo hacia arriba.
    let bottom_up = raw_height > 0;
    let mut pixels = Vec::with_capacity(width as usize * height as usize * 4);
    let mut any_alpha = false;
    for y in 0..height as usize {
        let src_y = if bottom_up {
            height as usize - 1 - y
        } else {
            y
        };
        let row = &data[src_y * stride..];
        for x in 0..width as usize {
            let p = &row[x * bytes_pp..];
            let a = if bytes_pp == 4 { p[3] } else { 255 };
            any_alpha |= a != 0;
            pixels.extend_from_slice(&[p[2], p[1], p[0], a]);
        }
    }
    // Muchas apps dejan el alfa en 0 aunque la imagen sea opaca.
    if !any_alpha {
        pixels.chunks_exact_mut(4).for_each(|p| p[3] = 255);
    }
    Some(Rgba {
        width,
        height,
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
