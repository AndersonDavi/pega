//! Llena una base nueva con datos de ejemplo, para capturas de pantalla sin datos reales.
//!
//!     cargo run --example demo -- C:\Temp\pega-demo\datos\pega.db
//!
//! Junto a esa carpeta `datos\` va una copia de `pega.exe` con un archivo `portable` al lado.

use pega::image::{self, Rgba};
use pega::settings::Theme;
use pega::state::{self, State};
use pega::store::Store;

const MIN: i64 = 60_000;

fn main() {
    let path = std::env::args().nth(1).expect("ruta de la base");
    let path = std::path::Path::new(&path);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).unwrap();
    }
    let mut s = State::load(Store::open(path).unwrap()).unwrap();
    let mut settings = s.settings.clone();
    settings.theme = Theme::Dark;
    settings.mask_cards = true;
    s.save_settings(settings);

    let now = state::now();
    let texts: &[(&str, &str, i64, bool)] = &[
        ("SELECT id, nombre, correo\nFROM clientes\nWHERE creado >= now() - interval '7 days'\nORDER BY creado DESC;", "code.exe", 2, false),
        ("https://github.com/AndersonDavi/pega", "chrome.exe", 4, false),
        ("{\n  \"nombre\": \"Pega\",\n  \"ram_mb\": 5,\n  \"cifrado\": true\n}", "code.exe", 7, false),
        ("Calle 93 # 11-26, Bogotá", "outlook.exe", 12, true),
        ("4111 1111 1111 1111", "chrome.exe", 15, false),
        ("cargo build --release && .\\target\\release\\pega.exe", "windowsterminal.exe", 21, false),
        ("fn main() {\n    println!(\"Hola desde Pega\");\n}", "code.exe", 33, false),
        ("¡Gracias por escribirnos! Te respondemos en menos de 24 horas.", "slack.exe", 48, false),
        ("Get-Process pega | Select-Object Name, PM", "windowsterminal.exe", 65, false),
        ("Reunión de equipo: jueves 10:00 a. m.", "outlook.exe", 90, false),
    ];
    for &(text, app, ago, pinned) in texts.iter().rev() {
        s.import_text(text.into(), now - ago * MIN, pinned, Some(app.into()));
    }

    // Una "captura": degradado azul con una franja clara.
    let (w, h) = (640u32, 360u32);
    let mut pixels = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            let band = (140..220).contains(&y) && (80..560).contains(&x);
            let (r, g, b) = if band {
                (235, 240, 250)
            } else {
                (20 + (x * 30 / w) as u8, 60 + (y * 60 / h) as u8, 160 + (x * 80 / w) as u8)
            };
            pixels.extend_from_slice(&[r, g, b, 255]);
        }
    }
    let img = Rgba { width: w, height: h, pixels };
    let png = image::encode_png(&img, png::Compression::Fast).unwrap();
    let thumb = image::encode_png(&image::thumbnail(&img), png::Compression::Fast).unwrap();
    s.import_image(&png, thumb, image::hash(&img), w, h, now - 9 * MIN, false, Some("sharex.exe".into()));

    let boards: &[(&str, u32, &[(&str, &str)])] = &[
        ("Datos personales", 0x3B82F6, &[
            ("Correo", "hola@ejemplo.com"),
            ("Dirección", "Calle 93 # 11-26, Bogotá"),
            ("NIT", "900.123.456-7"),
        ]),
        ("Respuestas", 0x22C55E, &[
            ("Saludo", "Hola {campo:Nombre}, gracias por escribirnos."),
            ("Firma", "Saludos,\nEquipo Pega\n{fecha}"),
        ]),
        ("SQL", 0xF97316, &[
            ("Clientes de la semana", "SELECT * FROM clientes WHERE creado >= now() - interval '7 days';"),
        ]),
    ];
    for &(name, color, clips) in boards {
        let id = s.add_board(name).unwrap();
        s.set_board_color(id, Some(color));
        for &(clip, text) in clips {
            s.save_clip(None, id, clip, text);
        }
    }
    s.flush();
    println!("Listo: {}", path.display());
}
