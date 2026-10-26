//! Respaldos: una copia compacta de la base (`VACUUM INTO`), que sigue cifrada con
//! DPAPI. Por eso solo la puede restaurar el mismo usuario de Windows.
//!
//! Restaurar reemplaza la base entera: Pega se reinicia con `--restore <archivo>`,
//! guarda la base actual con otro nombre y copia el respaldo en su lugar.

use crate::crypto;
use crate::i18n::t;
use crate::tr;
use rusqlite::{Connection, OpenFlags};
use std::mem;
use std::path::{Path, PathBuf};
use windows_sys::Win32::Foundation::{HWND, SYSTEMTIME};
use windows_sys::Win32::System::SystemInformation::GetLocalTime;
use windows_sys::Win32::UI::Controls::Dialogs::{
    GetOpenFileNameW, GetSaveFileNameW, OFN_FILEMUSTEXIST, OFN_NOCHANGEDIR, OFN_OVERWRITEPROMPT,
    OFN_PATHMUSTEXIST, OPENFILENAMEW,
};

pub const RESTORE_ARG: &str = "--restore";

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn local_time() -> SYSTEMTIME {
    let mut t: SYSTEMTIME = unsafe { mem::zeroed() };
    unsafe { GetLocalTime(&mut t) };
    t
}

/// "pega-respaldo-2026-10-26.pegabak", con la fecha local.
pub fn default_name() -> String {
    let t = local_time();
    tr!(
        "pega-respaldo-{:04}-{:02}-{:02}.pegabak",
        "pega-backup-{:04}-{:02}-{:02}.pegabak",
        t.wYear,
        t.wMonth,
        t.wDay
    )
}

/// Diálogo nativo de Windows para guardar (`save`) o abrir un respaldo.
pub fn file_dialog(owner: isize, save: bool, default_name: &str) -> Option<PathBuf> {
    // Filtro: pares descripción\0patrón\0, terminado en doble \0.
    let filter: Vec<u16> = t(
        "Respaldo de Pega (*.pegabak)\0*.pegabak\0Todos los archivos\0*.*\0\0",
        "Pega backup (*.pegabak)\0*.pegabak\0All files\0*.*\0\0",
    )
    .encode_utf16()
    .collect();
    let def_ext = wide("pegabak");
    let mut buf = [0u16; 1024];
    for (i, u) in default_name.encode_utf16().take(buf.len() - 1).enumerate() {
        buf[i] = u;
    }
    let mut ofn: OPENFILENAMEW = unsafe { mem::zeroed() };
    ofn.lStructSize = mem::size_of::<OPENFILENAMEW>() as u32;
    ofn.hwndOwner = owner as HWND;
    ofn.lpstrFilter = filter.as_ptr();
    ofn.lpstrFile = buf.as_mut_ptr();
    ofn.nMaxFile = buf.len() as u32;
    ofn.lpstrDefExt = def_ext.as_ptr();
    ofn.Flags = OFN_NOCHANGEDIR
        | OFN_PATHMUSTEXIST
        | if save {
            OFN_OVERWRITEPROMPT
        } else {
            OFN_FILEMUSTEXIST
        };
    let ok = unsafe {
        if save {
            GetSaveFileNameW(&mut ofn)
        } else {
            GetOpenFileNameW(&mut ofn)
        }
    };
    if ok == 0 {
        return None;
    }
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    Some(PathBuf::from(String::from_utf16_lossy(&buf[..len])))
}

/// Comprueba que el archivo sea un respaldo de Pega que este usuario pueda descifrar.
pub fn validate(path: &Path) -> Result<(), String> {
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|_| t("No se pudo abrir el archivo.", "Couldn't open the file.").to_owned())?;
    let tables: i64 = conn
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name IN ('history', 'boards', 'clips')",
            [],
            |r| r.get(0),
        )
        .map_err(|_| t("No es un respaldo de Pega.", "This isn't a Pega backup.").to_owned())?;
    if tables != 3 {
        return Err(t("No es un respaldo de Pega.", "This isn't a Pega backup.").into());
    }
    // Una muestra de cada tabla: si hay datos y ninguno se descifra, es de otro usuario.
    let sample: Vec<Vec<u8>> = [
        "SELECT data FROM clips LIMIT 1",
        "SELECT data FROM history WHERE kind = 0 LIMIT 1",
        "SELECT value FROM meta WHERE key = 'settings'",
    ]
    .iter()
    .filter_map(|q| conn.query_row(q, [], |r| r.get(0)).ok())
    .collect();
    if !sample.is_empty() && !sample.iter().any(|b| crypto::unprotect(b).is_some()) {
        return Err(t(
            "Este respaldo es de otro usuario de Windows (o está dañado): no se puede descifrar.",
            "This backup belongs to another Windows user (or is damaged): it can't be decrypted.",
        )
        .into());
    }
    Ok(())
}

/// Se llama al arrancar con `--restore`: aparta la base actual y copia el respaldo.
pub fn apply_restore(dir: &Path, backup: &Path) -> Result<PathBuf, String> {
    validate(backup)?;
    let db = dir.join("pega.db");
    let t = local_time();
    let kept = dir.join(format!(
        "pega-antes-de-restaurar-{:04}{:02}{:02}-{:02}{:02}{:02}.db",
        t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond
    ));
    if db.exists() {
        std::fs::rename(&db, &kept).map_err(|e| {
            tr!(
                "No se pudo apartar la base actual: {e}",
                "Couldn't set the current database aside: {e}"
            )
        })?;
    }
    // El WAL de la base anterior ya se volcó antes de reiniciar; no debe aplicarse a la nueva.
    for ext in ["pega.db-wal", "pega.db-shm"] {
        let _ = std::fs::remove_file(dir.join(ext));
    }
    std::fs::copy(backup, &db).map_err(|e| {
        tr!(
            "No se pudo copiar el respaldo: {e}",
            "Couldn't copy the backup: {e}"
        )
    })?;
    Ok(kept)
}

/// Reinicia Pega para restaurar `backup` (el proceso actual debe terminar enseguida).
pub fn restart_with_restore(backup: &Path) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    std::process::Command::new(exe)
        .arg(RESTORE_ARG)
        .arg(backup)
        .spawn()
        .map(|_| ())
        .map_err(|e| {
            tr!(
                "No se pudo reiniciar Pega: {e}",
                "Couldn't restart Pega: {e}"
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::State;
    use crate::store::Store;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("pega-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn backup_validate_and_restore() {
        let src = temp_dir("src");
        let mut s = State::load(Store::open(&src.join("pega.db")).unwrap()).unwrap();
        let board = s.add_board("Info personal").unwrap();
        s.save_clip(None, board, "Cédula", "123");
        s.capture("hola".into(), None);
        let file = src.join("respaldo.pegabak");
        s.backup_to(&file).unwrap();
        assert!(validate(&file).is_ok());

        let dst = temp_dir("dst");
        std::fs::write(dst.join("pega.db"), b"base vieja").unwrap();
        let kept = apply_restore(&dst, &file).unwrap();
        assert_eq!(std::fs::read(&kept).unwrap(), b"base vieja");
        let restored = State::load(Store::open(&dst.join("pega.db")).unwrap()).unwrap();
        assert_eq!(restored.clips[0].name, "Cédula");
        assert_eq!(restored.history[0].text, "hola");
        let _ = std::fs::remove_dir_all(src);
        let _ = std::fs::remove_dir_all(dst);
    }

    #[test]
    fn rejects_other_files() {
        let dir = temp_dir("bad");
        let file = dir.join("x.pegabak");
        std::fs::write(&file, b"no soy una base").unwrap();
        assert!(validate(&file).is_err());
        let _ = std::fs::remove_dir_all(dir);
    }
}
