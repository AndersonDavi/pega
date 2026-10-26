//! Estado en memoria (descifrado) + escritura cifrada a SQLite.
//! Lo comparten la interfaz y el hilo del portapapeles detrás de un Mutex.

use crate::code::{self, Lang};
use crate::crypto;
use crate::image::{self, Rgba};
use crate::settings::Settings;
use crate::store::{KIND_IMAGE, Name, Store};
use std::time::{SystemTime, UNIX_EPOCH};

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
    /// App de la que vino la copia (id en State::apps).
    pub app: Option<i64>,
    /// Lenguaje de programación detectado, si parece código.
    pub lang: Option<Lang>,
}

/// Clave de búsqueda de un texto: sin tildes ni mayúsculas, más el lenguaje ("json").
fn text_key(text: &str, lang: Option<Lang>) -> String {
    let mut key = fold(text);
    if let Some(l) = lang {
        key.push(' ');
        key.push_str(l.name());
    }
    key
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
    app: Option<String>,
}

pub fn prepare_image(img: &Rgba, hash: i64, app: Option<String>) -> Option<PreparedImage> {
    let png = image::encode_png(img, png::Compression::Balanced)?;
    let thumb_png = image::encode_png(&image::thumbnail(img), png::Compression::Balanced)?;
    Some(PreparedImage {
        hash,
        width: img.width,
        height: img.height,
        data: crypto::protect(&png)?,
        thumb: crypto::protect(&thumb_png)?,
        thumb_png,
        app,
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
    /// Ejecutables de origen ("chrome.exe"), con sus nombres cifrados en disco.
    pub apps: Vec<(i64, String)>,
    pub settings: Settings,
    pub hotkey_ok: bool,
    pub error: Option<String>,
}

const SETTINGS_KEY: &str = "settings";

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
                        app: r.app_id,
                        lang: None,
                        image: Some(ImageInfo {
                            width: r.width,
                            height: r.height,
                            hash: r.hash?,
                            thumb_png,
                        }),
                    });
                }
                let text = crypto::unprotect_str(r.data.as_deref()?)?;
                let lang = code::detect(&text);
                Some(Entry {
                    id: r.id,
                    key: text_key(&text, lang),
                    lang,
                    text,
                    used_at: r.used_at,
                    pinned: r.pinned,
                    image: None,
                    app: r.app_id,
                })
            })
            .collect();
        let apps = store
            .load_apps()?
            .into_iter()
            .filter_map(|(id, name)| Some((id, crypto::unprotect_str(&name)?)))
            .collect();
        // Nombres en claro de versiones anteriores: se cifran al cargarlos.
        let mut plain_boards = Vec::new();
        let boards = store
            .load_boards()?
            .into_iter()
            .filter_map(|(id, name)| {
                let name = match name {
                    Name::Encrypted(b) => crypto::unprotect_str(&b)?,
                    Name::Plain(s) => {
                        plain_boards.push(id);
                        s
                    }
                };
                Some(Board { id, name })
            })
            .collect();
        let mut plain_clips = Vec::new();
        let clips = store
            .load_clips()?
            .into_iter()
            .filter_map(|r| {
                let name = match r.name {
                    Name::Encrypted(b) => crypto::unprotect_str(&b)?,
                    Name::Plain(s) => {
                        plain_clips.push(r.id);
                        s
                    }
                };
                let text = crypto::unprotect_str(&r.data)?;
                let key = fold(&format!("{name} {text}"));
                Some(Clip {
                    id: r.id,
                    board_id: r.board_id,
                    name,
                    text,
                    key,
                })
            })
            .collect();
        let settings = store
            .get_meta(SETTINGS_KEY)?
            .and_then(|b| crypto::unprotect_str(&b))
            .map(|t| Settings::decode(&t))
            .unwrap_or_default();
        let state = Self {
            store,
            history,
            boards,
            clips,
            apps,
            settings,
            hotkey_ok: false,
            error: None,
        };
        state.encrypt_legacy_names(&plain_boards, &plain_clips)?;
        let mut state = state;
        state.purge_old();
        Ok(state)
    }

    fn encrypt_legacy_names(&self, boards: &[i64], clips: &[i64]) -> rusqlite::Result<()> {
        for b in self.boards.iter().filter(|b| boards.contains(&b.id)) {
            if let Some(enc) = crypto::protect_str(&b.name) {
                self.store.rename_board(b.id, &enc)?;
            }
        }
        for c in self.clips.iter().filter(|c| clips.contains(&c.id)) {
            if let Some(enc) = crypto::protect_str(&c.name) {
                self.store.set_clip_name(c.id, &enc)?;
            }
        }
        if !boards.is_empty() || !clips.is_empty() {
            self.store.checkpoint()?;
        }
        Ok(())
    }

    /// Guarda ajustes nuevos y aplica los límites. Devuelve si cambió el atajo.
    pub fn save_settings(&mut self, new: Settings) -> bool {
        let hotkey_changed = new.hotkey != self.settings.hotkey;
        match crypto::protect_str(&new.encode()) {
            Some(enc) => {
                let r = self.store.set_meta(SETTINGS_KEY, &enc);
                self.report(r);
            }
            None => self.error = Some("DPAPI no pudo cifrar los ajustes".into()),
        }
        self.settings = new;
        self.trim();
        self.purge_old();
        hotkey_changed
    }

    fn report<T>(&mut self, r: rusqlite::Result<T>) -> Option<T> {
        r.map_err(|e| self.error = Some(e.to_string())).ok()
    }

    /// Registra una copia nueva. Si ya existía, solo la sube al principio.
    pub fn capture(&mut self, text: String, app: Option<String>) -> bool {
        if text.trim().is_empty() || text.len() > MAX_TEXT_BYTES || self.excluded(&app) {
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
        let app = self.app_id(app);
        let r = self.store.insert_history(&data, used_at, app);
        let Some(id) = self.report(r) else {
            return false;
        };
        let lang = code::detect(&text);
        self.history.insert(
            0,
            Entry {
                id,
                key: text_key(&text, lang),
                lang,
                text,
                used_at,
                pinned: false,
                image: None,
                app,
            },
        );
        self.trim();
        self.purge_old();
        true
    }

    pub fn excluded(&self, app: &Option<String>) -> bool {
        app.as_deref().is_some_and(|a| self.settings.is_excluded(a))
    }

    pub fn app_name(&self, id: Option<i64>) -> Option<&str> {
        let id = id?;
        self.apps
            .iter()
            .find(|(i, _)| *i == id)
            .map(|(_, n)| n.as_str())
    }

    /// Id de una app de origen; la registra (cifrada) la primera vez que aparece.
    fn app_id(&mut self, name: Option<String>) -> Option<i64> {
        let name = name?;
        if let Some((id, _)) = self
            .apps
            .iter()
            .find(|(_, n)| n.eq_ignore_ascii_case(&name))
        {
            return Some(*id);
        }
        let enc = crypto::protect_str(&name)?;
        let r = self.store.add_app(&enc);
        let id = self.report(r)?;
        self.apps.push((id, name));
        Some(id)
    }

    /// Deja de guardar copias de una app (desde el menú de una entrada).
    pub fn exclude_app(&mut self, app: &str) {
        if !self.settings.is_excluded(app) {
            let mut s = self.settings.clone();
            s.excluded_apps.push(app.to_lowercase());
            self.save_settings(s);
        }
    }

    /// Borra lo no fijado más viejo que keep_days (0 = nunca).
    fn purge_old(&mut self) {
        if self.settings.keep_days == 0 {
            return;
        }
        let cutoff = now() - self.settings.keep_days as i64 * 86_400_000;
        let drop: Vec<i64> = self
            .history
            .iter()
            .filter(|e| !e.pinned && e.used_at < cutoff)
            .map(|e| e.id)
            .collect();
        if drop.is_empty() {
            return;
        }
        self.history.retain(|e| !drop.contains(&e.id));
        let r = self.store.delete_history(&drop);
        self.report(r);
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
        if self.excluded(&p.app) {
            return false;
        }
        let used_at = now();
        let app = self.app_id(p.app);
        let r = self
            .store
            .insert_image(&p.data, &p.thumb, p.hash, p.width, p.height, used_at, app);
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
                app,
                lang: None,
                image: Some(ImageInfo {
                    width: p.width,
                    height: p.height,
                    hash: p.hash,
                    thumb_png: p.thumb_png,
                }),
            },
        );
        self.trim();
        self.purge_old();
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
        // 0 = sin límite.
        let limit = |n: usize| if n == 0 { usize::MAX } else { n };
        let (max_texts, max_images) = (
            limit(self.settings.max_texts),
            limit(self.settings.max_images),
        );
        let mut texts = 0;
        let mut images = 0;
        let mut drop = Vec::new();
        // Del más nuevo al más viejo: lo que pase del límite se va. Los fijados no cuentan.
        for e in self.history.iter().filter(|e| !e.pinned) {
            let over = if e.image.is_some() {
                images += 1;
                images > max_images
            } else {
                texts += 1;
                texts > max_texts
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
        let enc = crypto::protect_str(name)?;
        let r = self.store.add_board(&enc, self.boards.len() as i64);
        let id = self.report(r)?;
        self.boards.push(Board {
            id,
            name: name.to_owned(),
        });
        Some(id)
    }

    pub fn rename_board(&mut self, id: i64, name: &str) {
        let Some(enc) = crypto::protect_str(name) else {
            return;
        };
        let r = self.store.rename_board(id, &enc);
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
        let (Some(data), Some(enc_name)) = (crypto::protect_str(text), crypto::protect_str(name))
        else {
            self.error = Some("DPAPI no pudo cifrar el clip".into());
            return;
        };
        let key = fold(&format!("{name} {text}"));
        match id {
            Some(id) => {
                let r = self.store.update_clip(id, &enc_name, &data);
                self.report(r);
                if let Some(c) = self.clips.iter_mut().find(|c| c.id == id) {
                    c.name = name.to_owned();
                    c.text = text.to_owned();
                    c.key = key;
                }
            }
            None => {
                let pos = self.clips.iter().filter(|c| c.board_id == board_id).count() as i64;
                let r = self.store.add_clip(board_id, &enc_name, &data, pos);
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn state() -> State {
        State::load(Store::open(Path::new(":memory:")).unwrap()).unwrap()
    }

    #[test]
    fn repeated_copy_moves_to_top_instead_of_duplicating() {
        let mut s = state();
        assert!(s.capture("uno".into(), None));
        assert!(s.capture("dos".into(), None));
        assert!(s.capture("uno".into(), None));
        assert_eq!(s.history.len(), 2);
        assert_eq!(s.history[0].text, "uno");
    }

    #[test]
    fn excluded_app_is_not_saved() {
        let mut s = state();
        s.exclude_app("KeePass.exe");
        assert!(!s.capture("secreto".into(), Some("keepass.exe".into())));
        assert!(s.capture("normal".into(), Some("chrome.exe".into())));
        assert_eq!(s.app_name(s.history[0].app), Some("chrome.exe"));
    }

    #[test]
    fn limits_keep_newest_and_pinned() {
        let mut s = state();
        let mut set = s.settings.clone();
        set.max_texts = 2;
        s.save_settings(set);
        for t in ["a", "b", "c"] {
            s.capture(t.into(), None);
        }
        let texts: Vec<&str> = s.history.iter().map(|e| e.text.as_str()).collect();
        assert_eq!(texts, ["c", "b"]);
        let id = s.history[1].id;
        s.toggle_pin(id);
        s.capture("d".into(), None);
        assert!(
            s.history.iter().any(|e| e.text == "b"),
            "lo fijado no se borra"
        );
    }

    #[test]
    fn old_entries_are_purged() {
        let mut s = state();
        s.capture("viejo".into(), None);
        s.history[0].used_at = now() - 10 * 86_400_000;
        let mut set = s.settings.clone();
        set.keep_days = 7;
        s.save_settings(set);
        assert!(s.history.is_empty());
    }

    #[test]
    fn names_and_text_round_trip_encrypted() {
        let mut s = state();
        let board = s.add_board("Info personal").unwrap();
        s.save_clip(None, board, "Cédula", "123");
        assert_eq!(s.clips[0].name, "Cédula");
        assert!(matches("cedula 123", &["cedula".into()]));
        assert_eq!(fold(&s.clips[0].key), "cedula 123");
    }
}
