//! Importa los tableros y clips de PasteBar. Su base se lee entera a memoria, sin
//! copia en texto plano en disco, y cada clip se guarda cifrado como cualquier otro.
//! Se puede repetir: lo que ya está (mismo tablero, nombre y contenido) no se duplica.

use crate::i18n::t;
use crate::state::State;
use crate::tr;
use rusqlite::{Connection, MAIN_DB};
use std::collections::HashMap;
use std::path::PathBuf;

pub struct Summary {
    pub boards: usize,
    pub clips: usize,
    /// Imágenes o clips vacíos, que Pega no guarda en tableros.
    pub skipped: usize,
}

struct Item {
    parent: Option<String>,
    name: String,
    value: String,
    is_board: bool,
    is_image: bool,
    order: i64,
    tab: String,
    tab_order: i64,
    collection: String,
}

fn pastebar_db() -> Option<PathBuf> {
    let path = PathBuf::from(std::env::var_os("APPDATA")?)
        .join("app.anothervision.pasteBar")
        .join("pastebar-db.data");
    path.exists().then_some(path)
}

/// Texto de una columna aunque no sea UTF-8 válido.
fn text(r: &rusqlite::Row, i: usize) -> rusqlite::Result<String> {
    Ok(r.get_ref(i)?
        .as_bytes_or_null()?
        .map(|b| String::from_utf8_lossy(b).into_owned())
        .unwrap_or_default())
}

fn read_items(conn: &Connection) -> rusqlite::Result<HashMap<String, Item>> {
    let mut st = conn.prepare(
        "SELECT cc.item_id, cc.parent_id, cc.order_number, i.name, i.value, i.is_board, i.is_image,
                t.tab_name, t.tab_order_number, co.title
         FROM collection_clips cc
         JOIN items i ON i.item_id = cc.item_id
         JOIN tabs t ON t.tab_id = cc.tab_id
         JOIN collections co ON co.collection_id = cc.collection_id
         WHERE i.is_deleted = 0",
    )?;
    let rows = st.query_map([], |r| {
        Ok((
            text(r, 0)?,
            Item {
                parent: r.get(1)?,
                order: r.get(2)?,
                name: text(r, 3)?,
                value: text(r, 4)?,
                is_board: r.get::<_, Option<bool>>(5)?.unwrap_or(false),
                is_image: r.get::<_, Option<bool>>(6)?.unwrap_or(false),
                tab: text(r, 7)?,
                tab_order: r.get(8)?,
                collection: text(r, 9)?,
            },
        ))
    })?;
    rows.collect()
}

pub fn import_pastebar(state: &mut State) -> Result<Summary, String> {
    let path = pastebar_db().ok_or(t(
        "No se encontró la base de PasteBar",
        "PasteBar's database wasn't found",
    ))?;
    // fs::read comparte lectura y escritura: funciona aunque PasteBar esté abierto.
    let bytes = std::fs::read(&path).map_err(|e| {
        tr!(
            "No se pudo leer PasteBar: {e}",
            "Couldn't read PasteBar: {e}"
        )
    })?;
    let mut conn = Connection::open_in_memory().map_err(|e| e.to_string())?;
    conn.deserialize_read_exact(MAIN_DB, &bytes[..], bytes.len(), true)
        .map_err(|e| {
            tr!(
                "La base de PasteBar no se pudo abrir: {e}",
                "PasteBar's database couldn't be opened: {e}"
            )
        })?;
    let items = read_items(&conn).map_err(|e| {
        tr!(
            "Formato de PasteBar inesperado: {e}",
            "Unexpected PasteBar format: {e}"
        )
    })?;
    drop(conn);
    drop(bytes);

    let multi_collection = {
        let mut names: Vec<&str> = items.values().map(|i| i.collection.as_str()).collect();
        names.sort_unstable();
        names.dedup();
        names.len() > 1
    };
    // Nombre plano de un tablero: "Colección / Pestaña / Padre / Hijo", omitiendo lo obvio.
    let board_name = |id: &str| -> String {
        let mut parts = Vec::new();
        let mut cur = items.get(id);
        while let Some(b) = cur {
            parts.push(b.name.trim().to_owned());
            cur = b
                .parent
                .as_deref()
                .and_then(|p| items.get(p))
                .filter(|p| p.is_board);
        }
        let first = &items[id];
        if first.tab_order > 0 {
            parts.push(first.tab.clone());
        }
        if multi_collection {
            parts.push(first.collection.clone());
        }
        parts.reverse();
        parts.join(" / ")
    };
    // Orden de un tablero: pestaña y luego posición de cada ancestro.
    let board_sort = |id: &str| -> (i64, Vec<i64>) {
        let mut path = Vec::new();
        let mut cur = items.get(id);
        while let Some(b) = cur {
            path.push(b.order);
            cur = b
                .parent
                .as_deref()
                .and_then(|p| items.get(p))
                .filter(|p| p.is_board);
        }
        path.reverse();
        (items[id].tab_order, path)
    };

    let mut clips: Vec<(&str, &Item)> = items
        .values()
        .filter(|i| !i.is_board)
        .map(|i| (i.parent.as_deref().unwrap_or(""), i))
        .collect();
    clips.sort_by(|a, b| {
        let ka = (items.contains_key(a.0).then(|| board_sort(a.0)), a.1.order);
        let kb = (items.contains_key(b.0).then(|| board_sort(b.0)), b.1.order);
        ka.cmp(&kb)
    });

    let mut summary = Summary {
        boards: 0,
        clips: 0,
        skipped: 0,
    };
    let mut board_ids: HashMap<String, i64> = HashMap::new();
    for (parent, clip) in clips {
        if clip.is_image || clip.value.trim().is_empty() {
            summary.skipped += 1;
            continue;
        }
        // Un clip suelto en una pestaña va a un tablero con el nombre de la pestaña.
        let name = if items.get(parent).is_some_and(|p| p.is_board) {
            board_name(parent)
        } else {
            clip.tab.clone()
        };
        let board_id = match board_ids.get(&name) {
            Some(id) => *id,
            None => {
                let existing = state.boards.iter().find(|b| b.name == name).map(|b| b.id);
                let id = match existing {
                    Some(id) => id,
                    None => {
                        summary.boards += 1;
                        state
                            .add_board(&name)
                            .ok_or(t("No se pudo crear un tablero", "Couldn't create a board"))?
                    }
                };
                board_ids.insert(name, id);
                id
            }
        };
        let clip_name = clip.name.trim();
        let duplicate = state
            .clips
            .iter()
            .any(|c| c.board_id == board_id && c.name == clip_name && c.text == clip.value);
        if !duplicate {
            state.save_clip(None, board_id, clip_name, &clip.value);
            summary.clips += 1;
        }
    }
    Ok(summary)
}
