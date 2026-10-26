//! Entradas raras o malformadas contra todo lo que recibe datos de afuera (portapapeles,
//! archivos, la terminal). En release un pánico cierra la app, así que nada de esto puede
//! entrar en pánico; los resultados en sí no importan aquí.

use pega::code::{self, Lang};
use pega::query::Query;
use pega::settings::Settings;
use pega::transform::ALL;
use pega::{cli, image, mask, sync, template};
use std::collections::HashMap;

/// Generador pseudoaleatorio simple y repetible (xorshift).
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }

    fn bytes(&mut self, len: usize) -> Vec<u8> {
        (0..len).map(|_| self.next() as u8).collect()
    }

    /// Texto con piezas que suelen romper parsers: tildes, emoji, combinantes, llaves,
    /// comillas, porcentajes, saltos de línea, nulos y números largos.
    fn text(&mut self, max: usize) -> String {
        const PIECES: &[&str] = &[
            "a",
            "Ñ",
            "é",
            "e\u{301}",
            "😀",
            "👩‍👩‍👧",
            "{",
            "}",
            "{campo:",
            "{fecha}",
            "{campo:}",
            "\"",
            "'",
            "%",
            "%2",
            "%zz",
            "%E2%82",
            "\n",
            "\r\n",
            "\t",
            "\0",
            " ",
            "4111 1111 1111 1111",
            "ghp_",
            "sk-",
            "AKIA",
            "<",
            ">",
            "</",
            "{\"a\":",
            "[",
            "]",
            ":",
            ",",
            "SELECT ",
            "fn ",
            "#",
            "```",
            "---",
            "app:",
            "lang:",
            "tipo:",
            "hace:",
            "fijado",
            "-",
            "9999999999999999999999",
            "\u{FEFF}",
            "\u{200B}",
            "ᄀ",
            "𝔸",
        ];
        let n = self.below(max + 1);
        (0..n).map(|_| PIECES[self.below(PIECES.len())]).collect()
    }
}

const LANGS: [Lang; 13] = [
    Lang::Json,
    Lang::Html,
    Lang::Xml,
    Lang::Css,
    Lang::JavaScript,
    Lang::Rust,
    Lang::Python,
    Lang::Sql,
    Lang::Shell,
    Lang::PowerShell,
    Lang::CSharp,
    Lang::Markdown,
    Lang::Yaml,
];

#[test]
fn text_features_survive_random_text() {
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let vars = template::Vars::now("portapapeles 😀".into());
    let words = vec!["é".to_owned(), "😀".to_owned(), "".to_owned()];
    for _ in 0..3000 {
        let text = rng.text(40);
        let _ = code::detect(&text);
        let lang = LANGS[rng.below(LANGS.len())];
        let _ = code::highlight(&text, lang, rng.below(2) == 0);
        for tr in ALL {
            let _ = tr.apply(&text);
        }
        let _ = template::is_template(&text);
        let values: HashMap<String, String> = template::fields(&text)
            .into_iter()
            .map(|f| (f, rng.text(5)))
            .collect();
        let _ = template::render(&text, &vars, &values);
        let _ = mask::mask(&text, &words, true);
        let _ = mask::find_sensitive(&text, &words);
        let _ = Query::parse(&text);
        let _ = Settings::decode(&text);
        let _ = sync::Config::decode(&text);
        let args: Vec<String> = text.split(' ').map(str::to_owned).collect();
        let _ = cli::parse(&args);
    }
}

#[test]
fn huge_text_is_handled_quickly() {
    // 256 KB, lo máximo que se guarda: una sola línea y muchas líneas.
    let one_line = "x".repeat(256 * 1024);
    let many = "{\"clave\": \"valor\"},\n".repeat(256 * 1024 / 20);
    let start = std::time::Instant::now();
    for text in [&one_line, &many] {
        let _ = code::detect(text);
        for lang in LANGS {
            let _ = code::highlight(text, lang, true);
        }
        let _ = mask::mask(text, &["valor".to_owned()], true);
        let _ = mask::find_sensitive(text, &["valor".to_owned()]);
        for tr in ALL {
            let _ = tr.apply(text);
        }
    }
    assert!(start.elapsed().as_secs() < 5, "tardó {:?}", start.elapsed());
}

#[test]
fn dib_parser_survives_lying_headers() {
    let mut rng = Rng(42);
    // Cabeceras con valores extremos en cada campo.
    let extremes: [u32; 9] = [
        0,
        1,
        40,
        124,
        0x7FFF_FFFF,
        0x8000_0000,
        0xFFFF_FFFF,
        65_536,
        3,
    ];
    for &size in &extremes {
        for &w in &extremes {
            for &h in &extremes {
                for bits in [0u16, 1, 8, 16, 24, 32, 64] {
                    for comp in [0u32, 1, 3, 0xFFFF_FFFF] {
                        let mut dib = vec![0u8; 40];
                        dib[0..4].copy_from_slice(&size.to_le_bytes());
                        dib[4..8].copy_from_slice(&w.to_le_bytes());
                        dib[8..12].copy_from_slice(&h.to_le_bytes());
                        dib[14..16].copy_from_slice(&bits.to_le_bytes());
                        dib[16..20].copy_from_slice(&comp.to_le_bytes());
                        dib[32..36].copy_from_slice(&extremes[rng.below(9)].to_le_bytes());
                        let extra = rng.below(256);
                        dib.extend(rng.bytes(extra));
                        let _ = image::from_dib(&dib);
                    }
                }
            }
        }
    }
    for len in 0..300 {
        let _ = image::from_dib(&rng.bytes(len));
        let _ = image::decode_png(&rng.bytes(len));
    }
}

#[test]
fn png_decoder_rejects_bombs_and_garbage() {
    // PNG válido de 1x1 con el ancho cambiado a 100000 x 100000 en la cabecera.
    let img = image::Rgba {
        width: 1,
        height: 1,
        pixels: vec![1, 2, 3, 255],
    };
    let mut png = image::encode_png(&img, png::Compression::Fast).unwrap();
    png[16..20].copy_from_slice(&100_000u32.to_be_bytes());
    png[20..24].copy_from_slice(&100_000u32.to_be_bytes());
    assert!(image::decode_png(&png).is_none());
    // Truncado en cada posición.
    let good = image::encode_png(&img, png::Compression::Fast).unwrap();
    for cut in 0..good.len() {
        let _ = image::decode_png(&good[..cut]);
    }
}

#[test]
fn sync_files_reject_garbage() {
    let mut rng = Rng(7);
    let key = [9u8; 32];
    for len in 0..200 {
        assert!(sync::open(&key, &rng.bytes(len)).is_none());
    }
    // Un archivo válido truncado o con un byte cambiado no se acepta.
    let sealed = sync::seal(&key, b"datos").unwrap();
    for cut in 0..sealed.len() {
        assert!(sync::open(&key, &sealed[..cut]).is_none());
    }
}
