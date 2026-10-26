//! SQLite en disco. Solo ve bytes: el cifrado ocurre antes, en `state`.

use rusqlite::types::ValueRef;
use rusqlite::{Connection, OptionalExtension, Result, params};
use std::path::Path;

pub struct Store {
    conn: Connection,
}

pub const KIND_IMAGE: i64 = 1;

pub struct HistoryRow {
    pub id: i64,
    pub kind: i64,
    /// Solo para texto: de las imágenes no se carga el PNG completo hasta pegarlas.
    pub data: Option<Vec<u8>>,
    pub thumb: Option<Vec<u8>>,
    pub hash: Option<i64>,
    pub width: u32,
    pub height: u32,
    pub used_at: i64,
    pub pinned: bool,
    pub app_id: Option<i64>,
    /// Texto reconocido en una imagen (cifrado), si se activó el OCR.
    pub ocr: Option<Vec<u8>>,
}

pub struct ClipRow {
    pub id: i64,
    pub board_id: i64,
    pub name: Name,
    pub data: Vec<u8>,
}

/// Nombre de tablero o clip: cifrado, o en claro si viene de una versión anterior.
pub enum Name {
    Plain(String),
    Encrypted(Vec<u8>),
}

fn name_at(r: &rusqlite::Row, i: usize) -> Result<Name> {
    Ok(match r.get_ref(i)? {
        ValueRef::Blob(b) => Name::Encrypted(b.to_vec()),
        other => {
            Name::Plain(String::from_utf8_lossy(other.as_bytes().unwrap_or_default()).into_owned())
        }
    })
}

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)?;
        // pega.exe y pega-cli.exe pueden escribir a la vez: esperar en vez de fallar.
        conn.busy_timeout(std::time::Duration::from_secs(2))?;
        // auto_vacuum tiene que ir antes de crear tablas; con INCREMENTAL el archivo
        // devuelve el espacio libre en vez de quedarse inflado como el de PasteBar.
        conn.execute_batch(
            "PRAGMA auto_vacuum = INCREMENTAL;
             PRAGMA journal_mode = WAL;
             PRAGMA journal_size_limit = 1048576;
             PRAGMA wal_checkpoint(TRUNCATE);
             PRAGMA synchronous = NORMAL;
             PRAGMA foreign_keys = ON;
             PRAGMA secure_delete = ON;
             PRAGMA cache_size = -256;
             CREATE TABLE IF NOT EXISTS history (
                 id      INTEGER PRIMARY KEY,
                 data    BLOB    NOT NULL,
                 used_at INTEGER NOT NULL,
                 pinned  INTEGER NOT NULL DEFAULT 0
             );
             CREATE TABLE IF NOT EXISTS boards (
                 id   INTEGER PRIMARY KEY,
                 name TEXT    NOT NULL,
                 pos  INTEGER NOT NULL DEFAULT 0
             );
             CREATE TABLE IF NOT EXISTS clips (
                 id       INTEGER PRIMARY KEY,
                 board_id INTEGER NOT NULL REFERENCES boards(id) ON DELETE CASCADE,
                 name     TEXT    NOT NULL,
                 data     BLOB    NOT NULL,
                 pos      INTEGER NOT NULL DEFAULT 0
             );
             CREATE TABLE IF NOT EXISTS meta (
                 key   TEXT PRIMARY KEY,
                 value BLOB NOT NULL
             );
             CREATE TABLE IF NOT EXISTS apps (
                 id   INTEGER PRIMARY KEY,
                 name BLOB NOT NULL
             );",
        )?;
        // Columnas de imágenes, agregadas después de la primera versión.
        let existing: Vec<String> = conn
            .prepare("SELECT name FROM pragma_table_info('history')")?
            .query_map([], |r| r.get(0))?
            .collect::<Result<_>>()?;
        for (col, decl) in [
            ("kind", "INTEGER NOT NULL DEFAULT 0"),
            ("thumb", "BLOB"),
            ("hash", "INTEGER"),
            ("width", "INTEGER NOT NULL DEFAULT 0"),
            ("height", "INTEGER NOT NULL DEFAULT 0"),
            ("app_id", "INTEGER"),
            ("ocr", "BLOB"),
        ] {
            if !existing.iter().any(|c| c == col) {
                conn.execute_batch(&format!("ALTER TABLE history ADD COLUMN {col} {decl};"))?;
            }
        }
        conn.execute_batch("CREATE INDEX IF NOT EXISTS history_hash ON history(hash);")?;
        let board_cols: Vec<String> = conn
            .prepare("SELECT name FROM pragma_table_info('boards')")?
            .query_map([], |r| r.get(0))?
            .collect::<Result<_>>()?;
        if !board_cols.iter().any(|c| c == "color") {
            conn.execute_batch("ALTER TABLE boards ADD COLUMN color INTEGER;")?;
        }
        Ok(Self { conn })
    }

    pub fn load_history(&self) -> Result<Vec<HistoryRow>> {
        let mut st = self.conn.prepare(
            "SELECT id, kind, CASE WHEN kind = 0 THEN data END, thumb, hash, width, height, used_at, pinned, app_id, ocr
             FROM history ORDER BY used_at DESC",
        )?;
        let rows = st.query_map([], |r| {
            Ok(HistoryRow {
                id: r.get(0)?,
                kind: r.get(1)?,
                data: r.get(2)?,
                thumb: r.get(3)?,
                hash: r.get(4)?,
                width: r.get(5)?,
                height: r.get(6)?,
                used_at: r.get(7)?,
                pinned: r.get(8)?,
                app_id: r.get(9)?,
                ocr: r.get(10)?,
            })
        })?;
        rows.collect()
    }

    pub fn insert_image(
        &self,
        data: &[u8],
        thumb: &[u8],
        hash: i64,
        width: u32,
        height: u32,
        used_at: i64,
        app_id: Option<i64>,
    ) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO history (kind, data, thumb, hash, width, height, used_at, app_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                KIND_IMAGE, data, thumb, hash, width, height, used_at, app_id
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Contenido completo (cifrado) de una entrada; para imágenes, el PNG.
    pub fn history_data(&self, id: i64) -> Result<Vec<u8>> {
        self.conn
            .query_row("SELECT data FROM history WHERE id = ?1", [id], |r| r.get(0))
    }

    pub fn insert_history(&self, data: &[u8], used_at: i64, app_id: Option<i64>) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO history (data, used_at, app_id) VALUES (?1, ?2, ?3)",
            params![data, used_at, app_id],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn load_apps(&self) -> Result<Vec<(i64, Vec<u8>)>> {
        let mut st = self.conn.prepare("SELECT id, name FROM apps")?;
        let rows = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        rows.collect()
    }

    pub fn add_app(&self, name: &[u8]) -> Result<i64> {
        self.conn
            .execute("INSERT INTO apps (name) VALUES (?1)", [name])?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn touch_history(&self, id: i64, used_at: i64) -> Result<()> {
        self.conn.execute(
            "UPDATE history SET used_at = ?2 WHERE id = ?1",
            params![id, used_at],
        )?;
        Ok(())
    }

    pub fn set_ocr(&self, id: i64, text: &[u8]) -> Result<()> {
        self.conn.execute(
            "UPDATE history SET ocr = ?2 WHERE id = ?1",
            params![id, text],
        )?;
        Ok(())
    }

    pub fn set_pinned(&self, id: i64, pinned: bool) -> Result<()> {
        self.conn.execute(
            "UPDATE history SET pinned = ?2 WHERE id = ?1",
            params![id, pinned],
        )?;
        Ok(())
    }

    pub fn delete_history(&self, ids: &[i64]) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        for id in ids {
            tx.execute("DELETE FROM history WHERE id = ?1", [id])?;
        }
        tx.commit()?;
        self.reclaim()
    }

    pub fn get_meta(&self, key: &str) -> Result<Option<Vec<u8>>> {
        self.conn
            .query_row("SELECT value FROM meta WHERE key = ?1", [key], |r| r.get(0))
            .optional()
    }

    pub fn set_meta(&self, key: &str, value: &[u8]) -> Result<()> {
        self.conn.execute(
            "INSERT INTO meta (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    pub fn load_boards(&self) -> Result<Vec<(i64, Name, Option<u32>)>> {
        let mut st = self
            .conn
            .prepare("SELECT id, name, color FROM boards ORDER BY pos, id")?;
        let rows = st.query_map([], |r| Ok((r.get(0)?, name_at(r, 1)?, r.get(2)?)))?;
        rows.collect()
    }

    pub fn set_board_color(&self, id: i64, color: Option<u32>) -> Result<()> {
        self.conn.execute(
            "UPDATE boards SET color = ?2 WHERE id = ?1",
            params![id, color],
        )?;
        Ok(())
    }

    /// Guarda el orden de los tableros: (id, posición).
    pub fn set_board_positions(&self, order: &[(i64, i64)]) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        for (id, pos) in order {
            tx.execute("UPDATE boards SET pos = ?2 WHERE id = ?1", params![id, pos])?;
        }
        tx.commit()
    }

    /// Guarda tablero y orden de los clips: (id, tablero, posición).
    pub fn set_clip_positions(&self, order: &[(i64, i64, i64)]) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        for (id, board, pos) in order {
            tx.execute(
                "UPDATE clips SET board_id = ?2, pos = ?3 WHERE id = ?1",
                params![id, board, pos],
            )?;
        }
        tx.commit()
    }

    pub fn load_clips(&self) -> Result<Vec<ClipRow>> {
        let mut st = self
            .conn
            .prepare("SELECT id, board_id, name, data FROM clips ORDER BY pos, id")?;
        let rows = st.query_map([], |r| {
            Ok(ClipRow {
                id: r.get(0)?,
                board_id: r.get(1)?,
                name: name_at(r, 2)?,
                data: r.get(3)?,
            })
        })?;
        rows.collect()
    }

    pub fn add_board(&self, name: &[u8], pos: i64) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO boards (name, pos) VALUES (?1, ?2)",
            params![name, pos],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn rename_board(&self, id: i64, name: &[u8]) -> Result<()> {
        self.conn.execute(
            "UPDATE boards SET name = ?2 WHERE id = ?1",
            params![id, name],
        )?;
        Ok(())
    }

    pub fn delete_board(&self, id: i64) -> Result<()> {
        self.conn
            .execute("DELETE FROM boards WHERE id = ?1", [id])?;
        self.reclaim()
    }

    pub fn add_clip(&self, board_id: i64, name: &[u8], data: &[u8], pos: i64) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO clips (board_id, name, data, pos) VALUES (?1, ?2, ?3, ?4)",
            params![board_id, name, data, pos],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn update_clip(&self, id: i64, name: &[u8], data: &[u8]) -> Result<()> {
        self.conn.execute(
            "UPDATE clips SET name = ?2, data = ?3 WHERE id = ?1",
            params![id, name, data],
        )?;
        Ok(())
    }

    pub fn set_clip_name(&self, id: i64, name: &[u8]) -> Result<()> {
        self.conn.execute(
            "UPDATE clips SET name = ?2 WHERE id = ?1",
            params![id, name],
        )?;
        Ok(())
    }

    pub fn delete_clip(&self, id: i64) -> Result<()> {
        self.conn.execute("DELETE FROM clips WHERE id = ?1", [id])?;
        self.reclaim()
    }

    /// Vuelca el WAL al archivo principal y lo vacía: tras cifrar datos que estaban en claro,
    /// las copias viejas de esas páginas no quedan en el WAL.
    pub fn checkpoint(&self) -> Result<()> {
        self.conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
    }

    /// Suelta las páginas que SQLite tiene en caché (se vuelven a leer del disco si hacen falta).
    pub fn release_memory(&self) -> Result<()> {
        self.conn.release_memory()
    }

    /// Copia compacta y consistente de toda la base (los datos siguen cifrados).
    pub fn backup_to(&self, path: &Path) -> Result<()> {
        let _ = std::fs::remove_file(path);
        self.conn
            .execute("VACUUM INTO ?1", [path.to_string_lossy()])
            .map(|_| ())
    }

    /// Devuelve al sistema las páginas que quedaron libres tras borrar.
    fn reclaim(&self) -> Result<()> {
        self.conn.execute_batch("PRAGMA incremental_vacuum;")
    }
}
