//! Estado en memoria (descifrado) + escritura cifrada a SQLite.
//! Lo comparten la interfaz y el hilo del portapapeles detrás de un Mutex.

use crate::crypto;
use crate::image::{self, Rgba};
use crate::store::{KIND_IMAGE, Store};
use std::time::{SystemTime, UNIX_EPOCH};

/// Copias sin fijar que se conservan; las más viejas se borran.
pub const MAX_HISTORY: usize = 1000;
/// De esas, cuántas pueden ser imágenes (pesan mucho más que el texto).
pub const MAX_IMAGES: usize = 100;
/// Textos más grandes se ignoran (logs enormes, archivos pegados por error).
pub const MAX_TEXT_BYTES: usize = 256 * 1024;

pub struct Entry {
    pub id: i64,
    /// Vacío en las imágenes.
    pub text: String,
    /// `text` en minúsculas y sin tildes, para buscar.
    pub key: String,
    pub used_at: i64,
    pub pinned: bool,
    pub image: Option<ImageInfo>,
}

pub struct ImageInfo {
    pub width: u32,
    pub height: u32,
    pub hash: i64,
    /// Miniatura en PNG (unos KB); la imagen completa se lee de disco al pegarla.
    pub thumb_png: Vec<u8>,
}

/// Imagen ya codificada y cifrada, lista para guardarse. Se arma sin el candado
/// porque comprimir una captura 4K tarda.
pub struct PreparedImage {
    hash: i64,
    width: u32,
    height: u32,
    data: Vec<u8>,
    thumb: Vec<u8>,
    thumb_png: Vec<u8>,
}

pub fn prepare_image(img: &Rgba, hash: i64) -> Option<PreparedImage> {
    let png = image::encode_png(img, png::Compression::Balanced)?;
    let thumb_png = image::encode_png(&image::thumbnail(img), png::Compression::Balanced)?;
    Some(PreparedImage {
        hash,
        width: img.width,
        height: img.height,
        data: crypto::protect(&png)?,
        thumb: crypto::protect(&thumb_png)?,
        thumb_png,
    })
}

fn image_key(w: u32, h: u32) -> String {
    format!("imagen {w}x{h}")
}

pub struct Board {
    pub id: i64,
    pub name: String,
}

pub struct Clip {
    pub id: i64,
    pub board_id: i64,
    pub name: String,
    pub text: String,
    pub key: String,
}

pub struct State {
    store: Store,
    /// Ordenado del más reciente al más viejo.
    pub history: Vec<Entry>,
    pub boards: Vec<Board>,
    pub clips: Vec<Clip>,
    pub hotkey_ok: bool,
    pub error: Option<String>,
}

pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Minúsculas y sin tildes: "Dirección" encuentra "direccion".
pub fn fold(s: &str) -> String {
    s.chars()
        .flat_map(char::to_lowercase)
        .map(|c| match c {
            'á' | 'à' | 'ä' | 'â' | 'ã' => 'a',
            'é' | 'è' | 'ë' | 'ê' => 'e',
            'í' | 'ì' | 'ï' | 'î' => 'i',
            'ó' | 'ò' | 'ö' | 'ô' | 'õ' => 'o',
            'ú' | 'ù' | 'ü' | 'û' => 'u',
            'ñ' => 'n',
            'ç' => 'c',
            c => c,
        })
        .collect()
}

/// Todas las palabras de la búsqueda deben aparecer, en cualquier orden.
pub fn matches(key: &str, terms: &[String]) -> bool {
    terms.iter().all(|t| key.contains(t.as_str()))
}

impl State {
    pub fn load(store: Store) -> rusqlite::Result<Self> {
        let history = store
            .load_history()?
            .into_iter()
            .filter_map(|r| {
                if r.kind == KIND_IMAGE {
                    let thumb_png = crypto::unprotect(r.thumb.as_deref()?)?;
                    return Some(Entry {
                        id: r.id,
                        text: String::new(),
                        key: image_key(r.width, r.height),
                        used_at: r.used_at,
                        pinned: r.pinned,
                        image: Some(ImageInfo {
                            width: r.width,
                            height: r.height,
                            hash: r.hash?,
                            thumb_png,
                        }),
                    });
                }
                let text = crypto::unprotect_str(r.data.as_deref()?)?;
                Some(Entry {
                    id: r.id,
                    key: fold(&text),
                    text,
                    used_at: r.used_at,
                    pinned: r.pinned,
                    image: None,
                })
            })
            .collect();
        let boards = store
            .load_boards()?
            .into_iter()
            .map(|(id, name)| Board { id, name })
            .collect();
        let clips = store
            .load_clips()?
            .into_iter()
            .filter_map(|r| {
                let text = crypto::unprotect_str(&r.data)?;
                let key = fold(&format!("{} {}", r.name, text));
                Some(Clip {
                    id: r.id,
                    board_id: r.board_id,
                    name: r.name,
                    text,
                    key,
                })
            })
            .collect();
        Ok(Self {
            store,
            history,
            boards,
            clips,
            hotkey_ok: false,
            error: None,
        })
    }

    fn report<T>(&mut self, r: rusqlite::Result<T>) -> Option<T> {
        r.map_err(|e| self.error = Some(e.to_string())).ok()
    }

    /// Registra una copia nueva. Si ya existía, solo la sube al principio.
    pub fn capture(&mut self, text: String) -> bool {
        if text.trim().is_empty() || text.len() > MAX_TEXT_BYTES {
            return false;
        }
        if let Some(pos) = self
            .history
            .iter()
            .position(|e| e.image.is_none() && e.text == text)
        {
            return self.bump(pos);
        }
        let used_at = now();
        let Some(data) = crypto::protect_str(&text) else {
            self.error = Some("DPAPI no pudo cifrar la copia".into());
            return false;
        };
        let r = self.store.insert_history(&data, used_at);
        let Some(id) = self.report(r) else {
            return false;
        };
        self.history.insert(
            0,
            Entry {
                id,
                key: fold(&text),
                text,
                used_at,
                pinned: false,
                image: None,
            },
        );
        self.trim();
        true
    }

    /// Sube al principio una entrada que se volvió a copiar.
    fn bump(&mut self, pos: usize) -> bool {
        if pos == 0 {
            return false;
        }
        let used_at = now();
        let mut e = self.history.remove(pos);
        e.used_at = used_at;
        let r = self.store.touch_history(e.id, used_at);
        self.report(r);
        self.history.insert(0, e);
        true
    }

    /// Si la imagen ya está en el historial la sube y devuelve `Some`; si no, `None`.
    pub fn bump_image(&mut self, hash: i64) -> Option<bool> {
        let pos = self
            .history
            .iter()
            .position(|e| e.image.as_ref().is_some_and(|i| i.hash == hash))?;
        Some(self.bump(pos))
    }

    pub fn insert_image(&mut self, p: PreparedImage) -> bool {
        // Otra copia igual pudo entrar mientras se comprimía.
        if self.bump_image(p.hash).is_some() {
            return true;
        }
        let used_at = now();
        let r = self
            .store
            .insert_image(&p.data, &p.thumb, p.hash, p.width, p.height, used_at);
        let Some(id) = self.report(r) else {
            return false;
        };
        self.history.insert(
            0,
            Entry {
                id,
                text: String::new(),
                key: image_key(p.width, p.height),
                used_at,
                pinned: false,
                image: Some(ImageInfo {
                    width: p.width,
                    height: p.height,
                    hash: p.hash,
                    thumb_png: p.thumb_png,
                }),
            },
        );
        self.trim();
        true
    }

    /// Lee de disco, descifra y decodifica una imagen para ponerla en el portapapeles.
    /// Devuelve también el PNG: muchas apps lo prefieren al bitmap.
    pub fn image_for_paste(&mut self, id: i64) -> Option<(Rgba, Vec<u8>)> {
        let r = self.store.history_data(id);
        let png = crypto::unprotect(&self.report(r)?)?;
        let img = image::decode_png(&png)?;
        Some((img, png))
    }

    fn trim(&mut self) {
        let mut texts = 0;
        let mut images = 0;
        let mut drop = Vec::new();
        // Del más nuevo al más viejo: lo que pase del límite se va. Los fijados no cuentan.
        for e in self.history.iter().filter(|e| !e.pinned) {
            let over = if e.image.is_some() {
                images += 1;
                images > MAX_IMAGES
            } else {
                texts += 1;
                texts > MAX_HISTORY
            };
            if over {
                drop.push(e.id);
            }
        }
        if drop.is_empty() {
            return;
        }
        self.history.retain(|e| !drop.contains(&e.id));
        let r = self.store.delete_history(&drop);
        self.report(r);
    }

    pub fn toggle_pin(&mut self, id: i64) {
        if let Some(e) = self.history.iter_mut().find(|e| e.id == id) {
            e.pinned = !e.pinned;
            let pinned = e.pinned;
            let r = self.store.set_pinned(id, pinned);
            self.report(r);
        }
    }

    pub fn delete_entry(&mut self, id: i64) {
        self.history.retain(|e| e.id != id);
        let r = self.store.delete_history(&[id]);
        self.report(r);
    }

    pub fn clear_history(&mut self) {
        let ids: Vec<i64> = self
            .history
            .iter()
            .filter(|e| !e.pinned)
            .map(|e| e.id)
            .collect();
        self.history.retain(|e| e.pinned);
        let r = self.store.delete_history(&ids);
        self.report(r);
    }

    pub fn add_board(&mut self, name: &str) -> Option<i64> {
        let r = self.store.add_board(name, self.boards.len() as i64);
        let id = self.report(r)?;
        self.boards.push(Board {
            id,
            name: name.to_owned(),
        });
        Some(id)
    }

    pub fn rename_board(&mut self, id: i64, name: &str) {
        let r = self.store.rename_board(id, name);
        self.report(r);
        if let Some(b) = self.boards.iter_mut().find(|b| b.id == id) {
            b.name = name.to_owned();
        }
    }

    pub fn delete_board(&mut self, id: i64) {
        let r = self.store.delete_board(id);
        self.report(r);
        self.boards.retain(|b| b.id != id);
        self.clips.retain(|c| c.board_id != id);
    }

    /// Crea (id = None) o edita un clip de tablero.
    pub fn save_clip(&mut self, id: Option<i64>, board_id: i64, name: &str, text: &str) {
        let Some(data) = crypto::protect_str(text) else {
            self.error = Some("DPAPI no pudo cifrar el clip".into());
            return;
        };
        let key = fold(&format!("{name} {text}"));
        match id {
            Some(id) => {
                let r = self.store.update_clip(id, name, &data);
                self.report(r);
                if let Some(c) = self.clips.iter_mut().find(|c| c.id == id) {
                    c.name = name.to_owned();
                    c.text = text.to_owned();
                    c.key = key;
                }
            }
            None => {
                let pos = self.clips.iter().filter(|c| c.board_id == board_id).count() as i64;
                let r = self.store.add_clip(board_id, name, &data, pos);
                if let Some(id) = self.report(r) {
                    self.clips.push(Clip {
                        id,
                        board_id,
                        name: name.to_owned(),
                        text: text.to_owned(),
                        key,
                    });
                }
            }
        }
    }

    pub fn delete_clip(&mut self, id: i64) {
        let r = self.store.delete_clip(id);
        self.report(r);
        self.clips.retain(|c| c.id != id);
    }
}
