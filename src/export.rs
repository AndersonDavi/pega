//! Exportación portátil con contraseña: sirve en otro PC u otro usuario, a diferencia de
//! los respaldos (que siguen atados a DPAPI). Los datos se descifran, se arma una base
//! SQLite en memoria y se cifra con XChaCha20-Poly1305; la clave sale de la contraseña con
//! Argon2id (parámetros recomendados por OWASP). Nunca se escribe nada en claro a disco.
//!
//! Archivo: "PEGAEXP1" | sal (16) | nonce (24) | datos cifrados y autenticados.

use crate::settings::Settings;
use crate::state::State;
use argon2::{Algorithm, Argon2, Params, Version};
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use rusqlite::{Connection, MAIN_DB, params};
use std::collections::HashMap;
use windows_sys::Win32::Security::Cryptography::{
    BCRYPT_USE_SYSTEM_PREFERRED_RNG, BCryptGenRandom,
};

const MAGIC: &[u8; 8] = b"PEGAEXP1";
const SALT_LEN: usize = 16;
const NONCE_LEN: usize = 24;
pub const EXTENSION: &str = "pega";

#[derive(Debug, Default, PartialEq)]
pub struct ImportSummary {
    pub history: usize,
    pub boards: usize,
    pub clips: usize,
    /// Se importaron ajustes que cambian los atajos: hay que volver a registrarlos.
    pub hotkeys_changed: bool,
}

pub(crate) fn random(buf: &mut [u8]) -> Result<(), String> {
    let status = unsafe {
        BCryptGenRandom(
            std::ptr::null_mut(),
            buf.as_mut_ptr(),
            buf.len() as u32,
            BCRYPT_USE_SYSTEM_PREFERRED_RNG,
        )
    };
    if status == 0 {
        Ok(())
    } else {
        Err("No se pudo generar aleatoriedad".into())
    }
}

pub(crate) fn derive_key(password: &str, salt: &[u8]) -> Result<[u8; 32], String> {
    let params = Params::new(Params::DEFAULT_M_COST, Params::DEFAULT_T_COST, 1, Some(32))
        .map_err(|e| e.to_string())?;
    let mut key = [0u8; 32];
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .hash_password_into(password.as_bytes(), salt, &mut key)
        .map_err(|e| e.to_string())?;
    Ok(key)
}

const SCHEMA: &str = "
    CREATE TABLE boards  (id INTEGER PRIMARY KEY, name TEXT NOT NULL, color INTEGER, pos INTEGER NOT NULL);
    CREATE TABLE clips   (board_id INTEGER NOT NULL, name TEXT NOT NULL, data TEXT NOT NULL, pos INTEGER NOT NULL);
    CREATE TABLE history (kind INTEGER NOT NULL, text TEXT, png BLOB, thumb BLOB, hash INTEGER,
                          width INTEGER, height INTEGER, used_at INTEGER NOT NULL, pinned INTEGER NOT NULL, app TEXT);
    CREATE TABLE meta    (key TEXT PRIMARY KEY, value TEXT NOT NULL);";

/// Arma el archivo de exportación (en memoria) a partir del estado actual.
pub fn export(state: &mut State, password: &str) -> Result<Vec<u8>, String> {
    if password.chars().count() < 8 {
        return Err("La contraseña debe tener al menos 8 caracteres.".into());
    }
    let mut conn = Connection::open_in_memory().map_err(|e| e.to_string())?;
    conn.execute_batch(SCHEMA).map_err(|e| e.to_string())?;
    {
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        for (pos, b) in state.boards.iter().enumerate() {
            tx.execute(
                "INSERT INTO boards VALUES (?1, ?2, ?3, ?4)",
                params![b.id, b.name, b.color, pos as i64],
            )
            .map_err(|e| e.to_string())?;
        }
        for (pos, c) in state.clips.iter().enumerate() {
            tx.execute(
                "INSERT INTO clips VALUES (?1, ?2, ?3, ?4)",
                params![c.board_id, c.name, c.text, pos as i64],
            )
            .map_err(|e| e.to_string())?;
        }
        let ids: Vec<i64> = state.history.iter().map(|e| e.id).collect();
        for id in ids {
            let png = state.image_png(id);
            let Some(e) = state.history.iter().find(|e| e.id == id) else {
                continue;
            };
            let app = state.app_name(e.app).map(str::to_owned);
            match &e.image {
                Some(img) => {
                    let Some(png) = png else { continue };
                    tx.execute(
                        "INSERT INTO history VALUES (1, NULL, ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                        params![
                            png,
                            img.thumb_png,
                            img.hash,
                            img.width,
                            img.height,
                            e.used_at,
                            e.pinned,
                            app
                        ],
                    )
                }
                None => tx.execute(
                    "INSERT INTO history VALUES (0, ?1, NULL, NULL, NULL, 0, 0, ?2, ?3, ?4)",
                    params![e.text, e.used_at, e.pinned, app],
                ),
            }
            .map_err(|e| e.to_string())?;
        }
        tx.execute(
            "INSERT INTO meta VALUES ('settings', ?1)",
            [state.settings.encode()],
        )
        .map_err(|e| e.to_string())?;
        tx.commit().map_err(|e| e.to_string())?;
    }
    let plain = conn.serialize(MAIN_DB).map_err(|e| e.to_string())?.to_vec();

    let mut salt = [0u8; SALT_LEN];
    let mut nonce = [0u8; NONCE_LEN];
    random(&mut salt)?;
    random(&mut nonce)?;
    let key = derive_key(password, &salt)?;
    let cipher = XChaCha20Poly1305::new_from_slice(&key).map_err(|e| e.to_string())?;
    let mut header = Vec::with_capacity(MAGIC.len() + SALT_LEN + NONCE_LEN);
    header.extend_from_slice(MAGIC);
    header.extend_from_slice(&salt);
    header.extend_from_slice(&nonce);
    // La cabecera va como datos asociados: si alguien la cambia, el descifrado falla.
    let sealed = cipher
        .encrypt(
            &XNonce::from(nonce),
            Payload {
                msg: &plain,
                aad: &header,
            },
        )
        .map_err(|_| "No se pudo cifrar la exportación".to_owned())?;
    header.extend_from_slice(&sealed);
    Ok(header)
}

/// Descifra una exportación y la mezcla con lo actual, sin duplicar.
pub fn import(state: &mut State, bytes: &[u8], password: &str) -> Result<ImportSummary, String> {
    let head = MAGIC.len() + SALT_LEN + NONCE_LEN;
    if bytes.len() <= head || &bytes[..MAGIC.len()] != MAGIC {
        return Err("No es una exportación de Pega.".into());
    }
    let salt = &bytes[MAGIC.len()..MAGIC.len() + SALT_LEN];
    let nonce: [u8; NONCE_LEN] = bytes[MAGIC.len() + SALT_LEN..head]
        .try_into()
        .map_err(|_| "Archivo dañado")?;
    let key = derive_key(password, salt)?;
    let cipher = XChaCha20Poly1305::new_from_slice(&key).map_err(|e| e.to_string())?;
    let plain = cipher
        .decrypt(
            &XNonce::from(nonce),
            Payload {
                msg: &bytes[head..],
                aad: &bytes[..head],
            },
        )
        .map_err(|_| "Contraseña incorrecta o archivo dañado.".to_owned())?;

    let mut conn = Connection::open_in_memory().map_err(|e| e.to_string())?;
    conn.deserialize_read_exact(MAIN_DB, &plain[..], plain.len(), true)
        .map_err(|e| e.to_string())?;
    drop(plain);
    let mut summary = ImportSummary::default();

    if let Ok(text) = conn.query_row("SELECT value FROM meta WHERE key = 'settings'", [], |r| {
        r.get::<_, String>(0)
    }) {
        // Lo que protege o dimensiona los datos de este PC no se toma del archivo: con los
        // límites de otro se borraba historial al importar, y con su PIN (o sin PIN) se podía
        // quedar bloqueado o desprotegido.
        let local = &state.settings;
        let settings = Settings {
            pin: local.pin.clone(),
            lock_minutes: local.lock_minutes,
            max_texts: local.max_texts,
            max_images: local.max_images,
            keep_days: local.keep_days,
            ..Settings::decode(&text)
        };
        summary.hotkeys_changed = state.save_settings(settings);
    }

    // Tableros: por nombre; los nuevos se crean. Ids exportados -> ids locales.
    let mut board_map: HashMap<i64, i64> = HashMap::new();
    let boards: Vec<(i64, String, Option<u32>)> = query_all(
        &conn,
        "SELECT id, name, color FROM boards ORDER BY pos",
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    for (old_id, name, color) in boards {
        let local = match state.boards.iter().find(|b| b.name == name) {
            Some(b) => b.id,
            None => {
                summary.boards += 1;
                let id = state
                    .add_board(&name)
                    .ok_or("No se pudo crear un tablero")?;
                if color.is_some() {
                    state.set_board_color(id, color);
                }
                id
            }
        };
        board_map.insert(old_id, local);
    }
    let clips: Vec<(i64, String, String)> = query_all(
        &conn,
        "SELECT board_id, name, data FROM clips ORDER BY pos",
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    for (old_board, name, text) in clips {
        let Some(&board) = board_map.get(&old_board) else {
            continue;
        };
        if !state
            .clips
            .iter()
            .any(|c| c.board_id == board && c.name == name && c.text == text)
        {
            state.save_clip(None, board, &name, &text);
            summary.clips += 1;
        }
    }

    type Row = (
        i64,
        Option<String>,
        Option<Vec<u8>>,
        Option<Vec<u8>>,
        Option<i64>,
        u32,
        u32,
        i64,
        bool,
        Option<String>,
    );
    let rows: Vec<Row> = query_all(
        &conn,
        "SELECT kind, text, png, thumb, hash, width, height, used_at, pinned, app FROM history ORDER BY used_at",
        |r| {
            Ok((
                r.get(0)?,
                r.get(1)?,
                r.get(2)?,
                r.get(3)?,
                r.get(4)?,
                r.get(5)?,
                r.get(6)?,
                r.get(7)?,
                r.get(8)?,
                r.get(9)?,
            ))
        },
    )?;
    for (kind, text, png, thumb, hash, w, h, used_at, pinned, app) in rows {
        let added = if kind == 1 {
            match (png, thumb, hash) {
                (Some(png), Some(thumb), Some(hash)) => {
                    state.import_image(&png, thumb, hash, w, h, used_at, pinned, app)
                }
                _ => false,
            }
        } else {
            text.is_some_and(|t| state.import_text(t, used_at, pinned, app))
        };
        summary.history += added as usize;
    }
    Ok(summary)
}

fn query_all<T>(
    conn: &Connection,
    sql: &str,
    f: impl FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<T>,
) -> Result<Vec<T>, String> {
    let mut st = conn.prepare(sql).map_err(|e| e.to_string())?;
    let rows = st.query_map([], f).map_err(|e| e.to_string())?;
    rows.collect::<rusqlite::Result<Vec<T>>>()
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::image::Rgba;
    use crate::store::Store;
    use std::path::Path;

    fn state() -> State {
        State::load(Store::open(Path::new(":memory:")).unwrap()).unwrap()
    }

    fn sample() -> State {
        let mut s = state();
        let b = s.add_board("Info personal").unwrap();
        s.set_board_color(b, Some(0x3B82F6));
        s.save_clip(None, b, "Cédula", "1098765432");
        s.capture("hola mundo".into(), Some("chrome.exe".into()));
        let img = Rgba {
            width: 2,
            height: 1,
            pixels: vec![255, 0, 0, 255, 0, 255, 0, 255],
        };
        let hash = crate::image::hash(&img);
        let p = crate::state::prepare_image(&img, hash, None).unwrap();
        s.insert_image(p);
        let id = s.history.iter().find(|e| e.image.is_none()).unwrap().id;
        s.toggle_pin(id);
        s
    }

    #[test]
    fn export_import_roundtrip() {
        let mut src = sample();
        let file = export(&mut src, "contraseña larga").unwrap();
        assert_eq!(&file[..8], MAGIC);
        assert!(
            !file.windows(10).any(|w| w == b"1098765432"),
            "nada en claro en el archivo"
        );

        let mut dst = state();
        let sum = import(&mut dst, &file, "contraseña larga").unwrap();
        assert_eq!((sum.history, sum.boards, sum.clips), (2, 1, 1));
        assert_eq!(dst.clips[0].text, "1098765432");
        assert_eq!(dst.boards[0].color, Some(0x3B82F6));
        let text = dst.history.iter().find(|e| e.image.is_none()).unwrap();
        assert!(text.pinned);
        assert_eq!(dst.app_name(text.app), Some("chrome.exe"));
        assert!(dst.history.iter().any(|e| e.image.is_some()));

        // Importar de nuevo no duplica.
        assert_eq!(
            import(&mut dst, &file, "contraseña larga").unwrap(),
            ImportSummary::default()
        );
    }

    #[test]
    fn import_keeps_local_limits_and_pin() {
        let mut src = sample();
        let mut s = src.settings.clone();
        s.max_texts = 1;
        s.pin = String::new();
        s.theme = crate::settings::Theme::Light;
        src.save_settings(s);
        let file = export(&mut src, "contraseña larga").unwrap();

        let mut dst = state();
        let mut s = dst.settings.clone();
        s.max_texts = 0;
        s.pin = "2468".into();
        dst.save_settings(s);
        for i in 0..5 {
            dst.capture(format!("texto local {i}"), None);
        }
        import(&mut dst, &file, "contraseña larga").unwrap();
        assert_eq!(dst.settings.max_texts, 0, "límite local intacto");
        assert_eq!(dst.settings.pin, "2468", "PIN local intacto");
        assert!(
            dst.settings.theme == crate::settings::Theme::Light,
            "el resto sí llega"
        );
        assert!(
            dst.history
                .iter()
                .filter(|e| e.text.starts_with("texto local"))
                .count()
                == 5
        );
    }

    #[test]
    fn wrong_password_and_tampering_fail() {
        let mut src = sample();
        let mut file = export(&mut src, "contraseña larga").unwrap();
        assert!(import(&mut state(), &file, "otra contraseña").is_err());
        let last = file.len() - 1;
        file[last] ^= 1;
        assert!(import(&mut state(), &file, "contraseña larga").is_err());
        assert!(import(&mut state(), b"cualquier cosa", "x").is_err());
        assert!(export(&mut src, "corta").is_err());
    }
}
