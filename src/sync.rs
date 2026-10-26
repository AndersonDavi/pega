//! Sincronización entre PCs a través de una carpeta que ya se sincroniza sola (OneDrive,
//! Dropbox, Google Drive…), sin servidor propio y cifrada de punta a punta.
//!
//! Cada PC escribe solo su archivo (`<id>.pegasync`) y lee los de los demás: como nadie
//! escribe el archivo de otro, la nube nunca tiene que resolver conflictos. Cada archivo es
//! una foto de tableros, clips, textos del historial y borrados recientes, en una base SQLite
//! en memoria cifrada con XChaCha20-Poly1305. La clave sale de una contraseña común
//! (Argon2id) con la sal guardada en la carpeta (`pega-sync.salt`); en cada PC la clave se
//! guarda cifrada con DPAPI, así que la contraseña se escribe una sola vez.
//!
//! No viajan las imágenes (pesan) ni lo temporal (códigos de un solo uso, apps sensibles).

use crate::export::{derive_key, random};
use crate::state::{self, State, board_sync_key, clip_sync_key, text_sync_key};
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use rusqlite::{Connection, MAIN_DB, params};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

const MAGIC: &[u8; 8] = b"PEGASYN1";
const NONCE_LEN: usize = 24;
const SALT_FILE: &str = "pega-sync.salt";
const EXT: &str = "pegasync";

#[derive(Clone, PartialEq, Eq)]
pub struct Config {
    pub dir: PathBuf,
    pub key: [u8; 32],
    /// Id de este PC: nombre de su archivo en la carpeta.
    pub device: String,
}

impl std::fmt::Debug for Config {
    // Sin la clave, por si algún día termina en un log.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Config")
            .field("dir", &self.dir)
            .field("device", &self.device)
            .finish()
    }
}

impl Config {
    pub fn encode(&self) -> String {
        let key: String = self.key.iter().map(|b| format!("{b:02x}")).collect();
        format!("{}\n{key}\n{}", self.dir.display(), self.device)
    }

    pub fn decode(text: &str) -> Option<Self> {
        let mut lines = text.lines();
        let dir = PathBuf::from(lines.next()?);
        let hex = lines.next()?;
        let device = lines.next()?.to_owned();
        if hex.len() != 64 || device.is_empty() {
            return None;
        }
        let mut key = [0u8; 32];
        for (i, b) in key.iter_mut().enumerate() {
            *b = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).ok()?;
        }
        Some(Self { dir, key, device })
    }

    fn own_file(&self) -> PathBuf {
        self.dir.join(format!("{}.{EXT}", self.device))
    }
}

/// Nombre del PC, para mostrar con quién se sincroniza.
fn computer_name() -> String {
    std::env::var("COMPUTERNAME").unwrap_or_else(|_| "PC".into())
}

/// Carpeta sugerida: `OneDrive\Pega` si OneDrive está instalado.
pub fn suggested_dir() -> Option<PathBuf> {
    std::env::var_os("OneDrive").map(|d| PathBuf::from(d).join("Pega"))
}

/// Activa la sincronización en `dir` con la contraseña común. Si la carpeta ya tiene
/// archivos de otros PCs, la contraseña tiene que abrirlos.
pub fn enable(dir: &Path, password: &str) -> Result<Config, String> {
    if password.chars().count() < 8 {
        return Err(crate::i18n::t(
            "La contraseña debe tener al menos 8 caracteres.",
            "The password must be at least 8 characters.",
        )
        .into());
    }
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let salt_path = dir.join(SALT_FILE);
    let salt = match std::fs::read(&salt_path) {
        Ok(s) if s.len() == 16 => s,
        _ => {
            let mut s = [0u8; 16];
            random(&mut s)?;
            std::fs::write(&salt_path, s).map_err(|e| e.to_string())?;
            s.to_vec()
        }
    };
    let key = derive_key(password, &salt)?;
    for path in peer_files(dir, None) {
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        if open(&key, &bytes).is_none() {
            return Err(crate::i18n::t(
                "La contraseña no coincide con la que usaste en tus otros PCs.",
                "The password doesn't match the one used on your other PCs.",
            )
            .into());
        }
    }
    let mut id = [0u8; 6];
    random(&mut id)?;
    Ok(Config {
        dir: dir.to_owned(),
        key,
        device: id.iter().map(|b| format!("{b:02x}")).collect(),
    })
}

/// Archivos de sincronización de la carpeta, menos el de `own` (este PC).
fn peer_files(dir: &Path, own: Option<&str>) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == EXT))
        .filter(|p| own.is_none_or(|own| p.file_stem().is_none_or(|s| s != own)))
        .collect()
}

pub fn seal(key: &[u8; 32], plain: &[u8]) -> Result<Vec<u8>, String> {
    let mut nonce = [0u8; NONCE_LEN];
    random(&mut nonce)?;
    let cipher = XChaCha20Poly1305::new_from_slice(key).map_err(|e| e.to_string())?;
    let mut out = Vec::with_capacity(MAGIC.len() + NONCE_LEN + plain.len() + 16);
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&nonce);
    let sealed = cipher
        .encrypt(
            &XNonce::from(nonce),
            Payload {
                msg: plain,
                aad: &out,
            },
        )
        .map_err(|_| "No se pudo cifrar".to_owned())?;
    out.extend_from_slice(&sealed);
    Ok(out)
}

pub fn open(key: &[u8; 32], bytes: &[u8]) -> Option<Vec<u8>> {
    let head = MAGIC.len() + NONCE_LEN;
    if bytes.len() <= head || &bytes[..MAGIC.len()] != MAGIC {
        return None;
    }
    let nonce: [u8; NONCE_LEN] = bytes[MAGIC.len()..head].try_into().ok()?;
    let cipher = XChaCha20Poly1305::new_from_slice(key).ok()?;
    cipher
        .decrypt(
            &XNonce::from(nonce),
            Payload {
                msg: &bytes[head..],
                aad: &bytes[..head],
            },
        )
        .ok()
}

const SCHEMA: &str = "
    CREATE TABLE boards  (name TEXT NOT NULL, color INTEGER);
    CREATE TABLE clips   (board TEXT NOT NULL, name TEXT NOT NULL, text TEXT NOT NULL, updated_at INTEGER NOT NULL);
    CREATE TABLE history (text TEXT NOT NULL, used_at INTEGER NOT NULL, pinned INTEGER NOT NULL, app TEXT);
    CREATE TABLE tombs   (key INTEGER NOT NULL, at INTEGER NOT NULL);
    CREATE TABLE meta    (key TEXT PRIMARY KEY, value TEXT NOT NULL);";

/// Foto de lo que este PC comparte (sin cifrar; `seal` la cifra). Siempre igual para el
/// mismo contenido, así se puede saber si cambió comparando bytes.
pub fn snapshot(state: &State, device_name: &str) -> Result<Vec<u8>, String> {
    let err = |e: rusqlite::Error| e.to_string();
    let mut conn = Connection::open_in_memory().map_err(err)?;
    conn.execute_batch(SCHEMA).map_err(err)?;
    let tx = conn.transaction().map_err(err)?;
    for b in &state.boards {
        tx.execute(
            "INSERT INTO boards VALUES (?1, ?2)",
            params![b.name, b.color],
        )
        .map_err(err)?;
        for c in state.clips.iter().filter(|c| c.board_id == b.id) {
            tx.execute(
                "INSERT INTO clips VALUES (?1, ?2, ?3, ?4)",
                params![b.name, c.name, c.text, c.updated_at],
            )
            .map_err(err)?;
        }
    }
    for e in &state.history {
        // Ni imágenes ni lo que se borra solo (códigos, apps sensibles).
        if e.image.is_some() || state.expiry_of(e).is_some() {
            continue;
        }
        tx.execute(
            "INSERT INTO history VALUES (?1, ?2, ?3, ?4)",
            params![e.text, e.used_at, e.pinned, state.app_name(e.app)],
        )
        .map_err(err)?;
    }
    for (key, at) in &state.tombs {
        tx.execute("INSERT INTO tombs VALUES (?1, ?2)", params![key, at])
            .map_err(err)?;
    }
    tx.execute(
        "INSERT INTO meta VALUES ('device', ?1)",
        params![device_name],
    )
    .map_err(err)?;
    tx.commit().map_err(err)?;
    Ok(conn.serialize(MAIN_DB).map_err(err)?.to_vec())
}

/// Lo que trae el archivo de otro PC.
struct Remote {
    device: String,
    boards: Vec<(String, Option<u32>)>,
    clips: Vec<(String, String, String, i64)>,
    history: Vec<(String, i64, bool, Option<String>)>,
    tombs: HashMap<i64, i64>,
}

fn parse(plain: &[u8]) -> Result<Remote, String> {
    let err = |e: rusqlite::Error| e.to_string();
    let mut conn = Connection::open_in_memory().map_err(err)?;
    conn.deserialize_read_exact(MAIN_DB, plain, plain.len(), true)
        .map_err(err)?;
    fn all<T>(
        conn: &Connection,
        sql: &str,
        f: impl FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<T>,
    ) -> Result<Vec<T>, String> {
        let mut st = conn.prepare(sql).map_err(|e| e.to_string())?;
        let rows = st.query_map([], f).map_err(|e| e.to_string())?;
        rows.collect::<rusqlite::Result<_>>()
            .map_err(|e| e.to_string())
    }
    Ok(Remote {
        device: conn
            .query_row("SELECT value FROM meta WHERE key = 'device'", [], |r| {
                r.get(0)
            })
            .unwrap_or_else(|_| "?".into()),
        boards: all(&conn, "SELECT name, color FROM boards", |r| {
            Ok((r.get(0)?, r.get(1)?))
        })?,
        clips: all(
            &conn,
            "SELECT board, name, text, updated_at FROM clips",
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )?,
        history: all(
            &conn,
            "SELECT text, used_at, pinned, app FROM history ORDER BY used_at",
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )?,
        tombs: all(&conn, "SELECT key, at FROM tombs", |r| {
            Ok((r.get(0)?, r.get(1)?))
        })?
        .into_iter()
        .collect(),
    })
}

/// Mezcla la foto de otro PC con lo local. Devuelve el nombre de ese PC y si cambió algo.
pub fn merge(state: &mut State, plain: &[u8]) -> Result<(String, bool), String> {
    let remote = parse(plain)?;
    let mut changed = false;
    let local_tombs: HashMap<i64, i64> = state.tombs.iter().copied().collect();
    let deleted_here = |key: i64, at: i64| local_tombs.get(&key).is_some_and(|&t| t >= at);

    // 1. Lo que el otro PC borró. Lo fijado aquí no se toca.
    let gone: Vec<i64> = state
        .history
        .iter()
        .filter(|e| e.image.is_none() && !e.pinned)
        .filter(|e| {
            remote
                .tombs
                .get(&text_sync_key(&e.text))
                .is_some_and(|&t| t >= e.used_at)
        })
        .map(|e| e.id)
        .collect();
    changed |= !gone.is_empty();
    state.remove_entries(&gone);
    let board_names: HashMap<i64, String> = state
        .boards
        .iter()
        .map(|b| (b.id, b.name.clone()))
        .collect();
    let gone: Vec<i64> = state
        .clips
        .iter()
        .filter(|c| {
            let board = board_names.get(&c.board_id).map_or("", String::as_str);
            remote
                .tombs
                .get(&clip_sync_key(board, &c.name))
                .is_some_and(|&t| t >= c.updated_at)
        })
        .map(|c| c.id)
        .collect();
    for id in &gone {
        state.remove_clip(*id);
        changed = true;
    }
    let gone: Vec<i64> = state
        .boards
        .iter()
        .filter(|b| remote.tombs.contains_key(&board_sync_key(&b.name)))
        // Solo si quedó vacío: un clip nuevo aquí lo mantiene.
        .filter(|b| !state.clips.iter().any(|c| c.board_id == b.id))
        .map(|b| b.id)
        .collect();
    for id in &gone {
        state.remove_board(*id);
        changed = true;
    }

    // 2. Tableros y clips: los que faltan se crean; si ambos lados lo tienen, gana la
    //    edición más nueva.
    for (name, color) in &remote.boards {
        let newest = remote
            .clips
            .iter()
            .filter(|c| &c.0 == name)
            .map(|c| c.3)
            .max()
            .unwrap_or(0);
        let exists = state
            .boards
            .iter()
            .any(|b| state::fold(&b.name) == state::fold(name));
        if !exists && !deleted_here(board_sync_key(name), newest) {
            if let Some(id) = state.add_board(name) {
                if color.is_some() {
                    state.set_board_color(id, *color);
                }
                changed = true;
            }
        }
    }
    for (board, name, text, updated_at) in &remote.clips {
        if deleted_here(clip_sync_key(board, name), *updated_at) {
            continue;
        }
        let Some(board_id) = state
            .boards
            .iter()
            .find(|b| state::fold(&b.name) == state::fold(board))
            .map(|b| b.id)
        else {
            continue;
        };
        let local = state
            .clips
            .iter()
            .find(|c| c.board_id == board_id && state::fold(&c.name) == state::fold(name))
            .map(|c| (c.id, c.updated_at, c.text == *text && c.name == *name));
        match local {
            None => {
                state.save_clip_at(None, board_id, name, text, *updated_at);
                changed = true;
            }
            Some((id, local_at, same)) if !same && *updated_at > local_at => {
                state.save_clip_at(Some(id), board_id, name, text, *updated_at);
                changed = true;
            }
            _ => {}
        }
    }

    // 3. Textos del historial. Lo que de todos modos se recortaría por los límites de este
    //    PC no se inserta (evita meter y borrar lo mismo en cada mezcla).
    let unpinned: Vec<i64> = state
        .history
        .iter()
        .filter(|e| !e.pinned)
        .map(|e| e.used_at)
        .collect();
    let full = state.settings.max_texts != 0 && unpinned.len() >= state.settings.max_texts;
    let floor = if full {
        unpinned.iter().copied().min().unwrap_or(0)
    } else {
        0
    };
    let cutoff = match state.settings.keep_days {
        0 => 0,
        d => state::now() - d as i64 * 86_400_000,
    };
    for (text, used_at, pinned, app) in remote.history {
        if deleted_here(text_sync_key(&text), used_at)
            || (!pinned && (used_at < floor || used_at < cutoff))
        {
            continue;
        }
        changed |= state.import_text(text, used_at, pinned, app);
    }
    Ok((remote.device, changed))
}

/// Estado para mostrar en la ventana de sincronización.
#[derive(Clone, Default)]
pub struct Status {
    /// Última vez que se leyó o escribió bien (ms).
    pub last_ok: Option<i64>,
    /// Otros PCs encontrados en la carpeta.
    pub peers: Vec<String>,
    pub error: Option<String>,
}

static STATUS: Mutex<Option<Status>> = Mutex::new(None);

pub fn status() -> Status {
    STATUS.lock().unwrap().clone().unwrap_or_default()
}

fn set_status(f: impl FnOnce(&mut Status)) {
    let mut s = STATUS.lock().unwrap();
    f(s.get_or_insert_with(Status::default));
}

/// Recuerda qué archivos ya se mezclaron y qué se escribió, para no repetir trabajo.
#[derive(Default)]
pub struct Tracker {
    seen: HashMap<PathBuf, SystemTime>,
    last_written: Option<Vec<u8>>,
}

/// Lee los archivos de los otros PCs que cambiaron y los mezcla. Devuelve si cambió algo.
pub fn read_peers(state: &Mutex<State>, cfg: &Config, tracker: &mut Tracker) -> bool {
    let mut changed = false;
    let mut peers = Vec::new();
    let mut error = None;
    for path in peer_files(&cfg.dir, Some(&cfg.device)) {
        let modified = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
        let unchanged = modified.is_some() && tracker.seen.get(&path) == modified.as_ref();
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        let Some(plain) = open(&cfg.key, &bytes) else {
            error = Some(crate::tr!(
                "No se pudo abrir {}: otra contraseña o archivo a medio sincronizar.",
                "Couldn't open {}: another password or a file still syncing.",
                path.file_name().unwrap_or_default().to_string_lossy()
            ));
            continue;
        };
        if unchanged {
            // Solo el nombre del PC, para la lista.
            if let Ok(r) = parse(&plain) {
                peers.push(r.device);
            }
            continue;
        }
        match merge(&mut state.lock().unwrap(), &plain) {
            Ok((device, c)) => {
                peers.push(device);
                changed |= c;
                if let Some(m) = modified {
                    tracker.seen.insert(path, m);
                }
            }
            Err(e) => error = Some(e),
        }
    }
    peers.sort();
    peers.dedup();
    set_status(|s| {
        s.peers = peers;
        s.error = error.clone();
        if error.is_none() {
            s.last_ok = Some(state::now());
        }
    });
    changed
}

/// Escribe el archivo de este PC si lo compartido cambió desde la última vez.
pub fn write_own(state: &Mutex<State>, cfg: &Config, tracker: &mut Tracker) {
    let plain = match snapshot(&state.lock().unwrap(), &computer_name()) {
        Ok(p) => p,
        Err(e) => return set_status(|s| s.error = Some(e)),
    };
    if tracker.last_written.as_ref() == Some(&plain) && cfg.own_file().exists() {
        return;
    }
    let result = seal(&cfg.key, &plain).and_then(|bytes| {
        // Escribir aparte y renombrar: los otros PCs nunca ven un archivo a medias.
        let tmp = cfg.dir.join(format!("{}.tmp", cfg.device));
        std::fs::write(&tmp, bytes).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, cfg.own_file()).map_err(|e| e.to_string())
    });
    match result {
        Ok(()) => {
            tracker.last_written = Some(plain);
            set_status(|s| {
                s.last_ok = Some(state::now());
                s.error = None;
            });
        }
        Err(e) => set_status(|s| s.error = Some(e)),
    }
}

// --- Hilo de sincronización: duerme hasta que la carpeta cambia o la app avisa. ---

use windows_sys::Win32::Foundation::{HANDLE, INVALID_HANDLE_VALUE, WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows_sys::Win32::Storage::FileSystem::{
    FILE_NOTIFY_CHANGE_FILE_NAME, FILE_NOTIFY_CHANGE_LAST_WRITE, FindCloseChangeNotification,
    FindFirstChangeNotificationW, FindNextChangeNotification,
};
use windows_sys::Win32::System::Threading::{
    CreateEventW, INFINITE, SetEvent, WaitForMultipleObjects, WaitForSingleObject,
};

static WAKE: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);

/// Pide escribir el archivo de este PC (tras un cambio local) o reaplicar la configuración.
pub fn poke() {
    let h = WAKE.load(std::sync::atomic::Ordering::Relaxed);
    if h != 0 {
        unsafe { SetEvent(h as HANDLE) };
    }
}

fn wide(s: &Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    s.as_os_str().encode_wide().chain(Some(0)).collect()
}

/// Arranca el hilo (una vez). `on_change` se llama cuando la mezcla cambió algo.
pub fn start(state: Arc<Mutex<State>>, on_change: impl Fn() + Send + 'static) {
    let wake = unsafe { CreateEventW(std::ptr::null(), 0, 0, std::ptr::null()) };
    if wake.is_null() {
        return;
    }
    WAKE.store(wake as isize, std::sync::atomic::Ordering::Relaxed);
    let wake = wake as isize;
    std::thread::Builder::new()
        .name("pega-sync".into())
        .spawn(move || run(state, wake as HANDLE, on_change))
        .ok();
}

fn run(state: Arc<Mutex<State>>, wake: HANDLE, on_change: impl Fn()) {
    let mut tracker = Tracker::default();
    loop {
        let Some(cfg) = state.lock().unwrap().sync.clone() else {
            *STATUS.lock().unwrap() = None;
            unsafe { WaitForSingleObject(wake, INFINITE) };
            continue;
        };
        let watch = unsafe {
            FindFirstChangeNotificationW(
                wide(&cfg.dir).as_ptr(),
                0,
                FILE_NOTIFY_CHANGE_LAST_WRITE | FILE_NOTIFY_CHANGE_FILE_NAME,
            )
        };
        if watch == INVALID_HANDLE_VALUE {
            set_status(|s| {
                s.error = Some(crate::tr!(
                    "No se puede abrir la carpeta {}.",
                    "Can't open the folder {}.",
                    cfg.dir.display()
                ))
            });
            // Reintenta cada minuto (la carpeta puede estar en un disco que se desconectó).
            unsafe { WaitForSingleObject(wake, 60_000) };
            continue;
        }
        if read_peers(&state, &cfg, &mut tracker) {
            on_change();
        }
        write_own(&state, &cfg, &mut tracker);
        loop {
            let handles = [wake, watch];
            let r = unsafe { WaitForMultipleObjects(2, handles.as_ptr(), 0, INFINITE) };
            if state.lock().unwrap().sync.as_ref() != Some(&cfg) {
                break;
            }
            if r == WAIT_OBJECT_0 {
                // Cambio local: se esperan 2 s sin más cambios y se escribe una sola vez.
                while unsafe { WaitForSingleObject(wake, 2_000) } != WAIT_TIMEOUT {}
                if state.lock().unwrap().sync.as_ref() != Some(&cfg) {
                    break;
                }
                write_own(&state, &cfg, &mut tracker);
            } else if r == WAIT_OBJECT_0 + 1 {
                unsafe { FindNextChangeNotification(watch) };
                // Deja que la nube termine de bajar el archivo.
                std::thread::sleep(std::time::Duration::from_millis(1_500));
                if read_peers(&state, &cfg, &mut tracker) {
                    on_change();
                    write_own(&state, &cfg, &mut tracker);
                }
            } else {
                break;
            }
        }
        unsafe { FindCloseChangeNotification(watch) };
        tracker = Tracker::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;

    fn state() -> State {
        State::load(Store::open(Path::new(":memory:")).unwrap()).unwrap()
    }

    /// Lo que A le pasa a B: foto de A mezclada en B.
    fn send(from: &State, to: &mut State) -> bool {
        let plain = snapshot(from, "A").unwrap();
        merge(to, &plain).unwrap().1
    }

    #[test]
    fn config_roundtrip_hides_key_in_debug() {
        let c = Config {
            dir: PathBuf::from(r"C:\Users\x\OneDrive\Pega"),
            key: [7; 32],
            device: "a1b2c3".into(),
        };
        assert_eq!(Config::decode(&c.encode()), Some(c.clone()));
        assert!(!format!("{c:?}").contains("07"));
        assert!(Config::decode("solo una línea").is_none());
    }

    #[test]
    fn seal_and_open() {
        let key = [3u8; 32];
        let sealed = seal(&key, b"hola").unwrap();
        assert_eq!(open(&key, &sealed).unwrap(), b"hola");
        assert!(open(&[4u8; 32], &sealed).is_none(), "otra clave");
        let mut bad = sealed.clone();
        *bad.last_mut().unwrap() ^= 1;
        assert!(open(&key, &bad).is_none(), "alterado");
    }

    #[test]
    fn texts_boards_and_clips_travel_once() {
        let mut a = state();
        let mut b = state();
        a.capture("desde A".into(), Some("code.exe".into()));
        let board = a.add_board("Datos").unwrap();
        a.set_board_color(board, Some(0x22C55E));
        a.save_clip(None, board, "Correo", "a@b.co");
        assert!(send(&a, &mut b));
        assert_eq!(b.history[0].text, "desde A");
        assert_eq!(b.app_name(b.history[0].app), Some("code.exe"));
        assert_eq!(b.boards[0].color, Some(0x22C55E));
        assert_eq!(b.clips[0].text, "a@b.co");
        assert!(!send(&a, &mut b), "la segunda vez no cambia nada");
        assert_eq!(snapshot(&a, "A").unwrap(), snapshot(&a, "A").unwrap());
    }

    #[test]
    fn newest_clip_edit_wins() {
        let mut a = state();
        let mut b = state();
        let ba = a.add_board("Datos").unwrap();
        a.save_clip_at(None, ba, "Tel", "300", 1_000);
        send(&a, &mut b);
        let id_b = b.clips[0].id;
        let bb = b.clips[0].board_id;
        b.save_clip_at(Some(id_b), bb, "Tel", "301", 2_000);
        assert!(send(&b, &mut a));
        assert_eq!(a.clips[0].text, "301");
        // Una versión más vieja no pisa la nueva.
        let id_a = a.clips[0].id;
        let mut old = state();
        let bo = old.add_board("Datos").unwrap();
        old.save_clip_at(None, bo, "Tel", "299", 500);
        assert!(!send(&old, &mut a));
        assert_eq!(a.clips.iter().find(|c| c.id == id_a).unwrap().text, "301");
    }

    #[test]
    fn deletions_propagate_and_do_not_come_back() {
        let mut a = state();
        let mut b = state();
        a.capture("borrame".into(), None);
        let board = a.add_board("Viejo").unwrap();
        a.save_clip(None, board, "X", "x");
        send(&a, &mut b);
        assert_eq!((b.history.len(), b.clips.len()), (1, 1));

        // A borra; B se entera y lo borra.
        let id = a.history[0].id;
        a.delete_entry(id);
        a.delete_board(board);
        assert!(send(&a, &mut b));
        assert!(b.history.is_empty() && b.clips.is_empty() && b.boards.is_empty());

        // Una foto vieja de un tercer PC no lo revive en A.
        let mut c = state();
        c.import_text("borrame".into(), 1, false, None);
        assert!(!send(&c, &mut a));
        assert!(a.history.is_empty());
    }

    #[test]
    fn pinned_entries_survive_remote_deletes_and_temporary_ones_stay_home() {
        let mut a = state();
        let mut b = state();
        a.capture("fijado".into(), None);
        send(&a, &mut b);
        let id_b = b.history[0].id;
        b.toggle_pin(id_b);
        let id_a = a.history[0].id;
        a.delete_entry(id_a);
        send(&a, &mut b);
        assert_eq!(b.history.len(), 1, "fijado en B: no se borra");

        // Un código de un solo uso no viaja.
        let mut otp = state();
        otp.capture("482193".into(), None);
        let mut d = state();
        assert!(!send(&otp, &mut d));
        assert!(d.history.is_empty());
    }

    #[test]
    fn renamed_board_moves_its_clips() {
        let mut a = state();
        let mut b = state();
        let board = a.add_board("Trabajo").unwrap();
        a.save_clip(None, board, "Firma", "Saludos");
        send(&a, &mut b);
        a.rename_board(board, "Oficina");
        send(&a, &mut b);
        let names: Vec<&str> = b.boards.iter().map(|b| b.name.as_str()).collect();
        assert_eq!(names, ["Oficina"]);
        assert_eq!(b.clips.len(), 1);
        assert_eq!(b.clips[0].board_id, b.boards[0].id);
    }

    #[test]
    fn enable_checks_password_against_other_pcs() {
        let dir = std::env::temp_dir().join(format!("pega-sync-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let first = enable(&dir, "contraseña común").unwrap();
        let s = Mutex::new(state());
        s.lock().unwrap().capture("hola".into(), None);
        write_own(&s, &first, &mut Tracker::default());
        assert!(first.own_file().exists());
        assert!(enable(&dir, "otra contraseña").is_err());
        let second = enable(&dir, "contraseña común").unwrap();
        assert_eq!(second.key, first.key, "misma sal, misma clave");
        assert_ne!(second.device, first.device);
        let other = Mutex::new(state());
        assert!(read_peers(&other, &second, &mut Tracker::default()));
        assert_eq!(other.lock().unwrap().history[0].text, "hola");
        assert!(enable(&dir, "corta").is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
