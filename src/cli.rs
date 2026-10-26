//! `pega-cli`: el historial y los clips desde la terminal. Los números son los mismos de
//! la ventana (fijados primero, luego lo más reciente), así `copy 3` copia lo que `list`
//! mostró como 3.

use crate::i18n::t;
use crate::query::Query;
use crate::state::{self, Clip, Entry, State};
use crate::tr;

#[derive(Debug, PartialEq)]
pub enum Command {
    List(usize),
    Search(String),
    Show(usize),
    Copy(usize),
    /// Texto a guardar en el historial; `None`: leerlo de la entrada estándar.
    Add(Option<String>),
    Delete(usize),
    Pin(usize),
    Clips,
    Clip(String),
    ShowClip(String),
    /// "Tablero/Nombre" y el texto (`None`: de la entrada estándar). Crea o reemplaza.
    NewClip(String, Option<String>),
    DeleteClip(String),
    Help,
}

impl Command {
    /// ¿Cambia la base? Entonces hay que avisarle a la app abierta.
    pub fn writes(&self) -> bool {
        matches!(
            self,
            Self::Add(_) | Self::Delete(_) | Self::Pin(_) | Self::NewClip(..) | Self::DeleteClip(_)
        )
    }
}

/// Lo que hay que poner en el portapapeles al terminar (lo hace el binario).
#[derive(Debug, PartialEq)]
pub enum Copy {
    Text { text: String, private: bool },
    Image(i64),
}

#[derive(Debug, PartialEq)]
pub struct Output {
    pub text: String,
    pub copy: Option<Copy>,
}

pub fn help() -> String {
    t(
        "Pega desde la terminal\n\n\
         Uso:\n  \
         pega-cli list [N]                  las últimas N copias (10 por defecto)\n  \
         pega-cli search <texto>            busca; acepta app:, lang:, tipo:, fijado, hace:2h\n  \
         pega-cli show <N>                  muestra completa la entrada N\n  \
         pega-cli copy <N>                  copia la entrada N de la lista\n  \
         pega-cli add [texto]               guarda un texto en el historial (sin texto: lo lee de la entrada)\n  \
         pega-cli delete <N>                borra la entrada N\n  \
         pega-cli pin <N>                   fija o suelta la entrada N\n  \
         pega-cli clips                     los clips de los tableros\n  \
         pega-cli clip <nombre>             copia un clip (\"Tablero/Nombre\" si se repite)\n  \
         pega-cli show-clip <nombre>        muestra el texto de un clip\n  \
         pega-cli new-clip <Tablero/Nombre> [texto]   crea o reemplaza un clip\n  \
         pega-cli delete-clip <nombre>      borra un clip\n",
        "Pega from the terminal\n\n\
         Usage:\n  \
         pega-cli list [N]                  the last N copies (default 10)\n  \
         pega-cli search <text>             search; supports app:, lang:, type:, pinned, within:2h\n  \
         pega-cli show <N>                  print entry N in full\n  \
         pega-cli copy <N>                  copy entry N from the list\n  \
         pega-cli add [text]                save a text to history (no text: read from stdin)\n  \
         pega-cli delete <N>                delete entry N\n  \
         pega-cli pin <N>                   pin or unpin entry N\n  \
         pega-cli clips                     board clips\n  \
         pega-cli clip <name>               copy a clip (\"Board/Name\" if repeated)\n  \
         pega-cli show-clip <name>          print a clip's text\n  \
         pega-cli new-clip <Board/Name> [text]   create or replace a clip\n  \
         pega-cli delete-clip <name>        delete a clip\n",
    )
    .to_owned()
}

pub fn parse(args: &[String]) -> Result<Command, String> {
    let num = |s: Option<&String>, default: Option<usize>| -> Result<usize, String> {
        match s {
            None => default.ok_or_else(|| t("Falta el número.", "Missing number.").to_owned()),
            Some(s) => s.parse::<usize>().ok().filter(|n| *n > 0).ok_or_else(|| {
                tr!(
                    "\"{s}\" no es un número válido.",
                    "\"{s}\" is not a valid number."
                )
            }),
        }
    };
    let rest = |from: usize| args[from..].join(" ");
    // Sin texto o con "-": de la entrada estándar.
    let text_or_stdin = |from: usize| match rest(from) {
        r if r.is_empty() || r == "-" => None,
        r => Some(r),
    };
    match args.first().map(String::as_str) {
        None | Some("help" | "ayuda" | "-h" | "--help" | "/?") => Ok(Command::Help),
        Some("list" | "lista") => Ok(Command::List(num(args.get(1), Some(10))?)),
        Some("search" | "buscar") if args.len() > 1 => Ok(Command::Search(rest(1))),
        Some("show" | "ver") => Ok(Command::Show(num(args.get(1), None)?)),
        Some("copy" | "copiar") => Ok(Command::Copy(num(args.get(1), None)?)),
        Some("add" | "agregar") => Ok(Command::Add(text_or_stdin(1))),
        Some("delete" | "borrar") => Ok(Command::Delete(num(args.get(1), None)?)),
        Some("pin" | "fijar") => Ok(Command::Pin(num(args.get(1), None)?)),
        Some("clips") => Ok(Command::Clips),
        Some("clip") if args.len() > 1 => Ok(Command::Clip(rest(1))),
        Some("show-clip" | "ver-clip") if args.len() > 1 => Ok(Command::ShowClip(rest(1))),
        Some("new-clip" | "nuevo-clip") => match args.get(1) {
            Some(path) if split_path(path).is_some() => {
                Ok(Command::NewClip(path.clone(), text_or_stdin(2)))
            }
            _ => Err(t(
                "Indica el clip como Tablero/Nombre (entre comillas si tiene espacios).",
                "Give the clip as Board/Name (quoted if it has spaces).",
            )
            .into()),
        },
        Some("delete-clip" | "borrar-clip") if args.len() > 1 => Ok(Command::DeleteClip(rest(1))),
        Some(other) => Err(tr!(
            "Comando desconocido: {other}",
            "Unknown command: {other}"
        )),
    }
}

/// "Tablero/Nombre" → ("Tablero", "Nombre"), sin espacios de más y sin partes vacías.
fn split_path(path: &str) -> Option<(&str, &str)> {
    let (board, name) = path.split_once('/')?;
    let (board, name) = (board.trim(), name.trim());
    (!board.is_empty() && !name.is_empty()).then_some((board, name))
}

/// Historial en el orden de la ventana.
fn ordered(state: &State) -> Vec<&Entry> {
    let mut hist: Vec<&Entry> = state.history.iter().collect();
    hist.sort_by_key(|e| !e.pinned);
    hist
}

fn line(n: usize, e: &Entry, state: &State) -> String {
    let body = match &e.image {
        Some(img) => tr!("[imagen {}×{}]", "[image {}×{}]", img.width, img.height),
        None => {
            let one: String = e.text.split_whitespace().collect::<Vec<_>>().join(" ");
            let mut s: String = one.chars().take(100).collect();
            if one.chars().count() > 100 {
                s.push('…');
            }
            s
        }
    };
    let pin = if e.pinned { "* " } else { "" };
    let mut tags: Vec<&str> = Vec::new();
    if let Some(l) = e.lang {
        tags.push(l.name());
    }
    if let Some(a) = state.app_name(e.app) {
        tags.push(a);
    }
    let tags = if tags.is_empty() {
        String::new()
    } else {
        format!("   [{}]", tags.join(" · "))
    };
    format!("{n:>3}  {pin}{body}{tags}")
}

/// La entrada N de la lista (1 = la primera).
fn nth<'a>(state: &'a State, n: usize) -> Result<&'a Entry, String> {
    ordered(state)
        .get(n - 1)
        .copied()
        .ok_or_else(|| tr!("No hay entrada {n}.", "There's no entry {n}."))
}

/// El clip por nombre, o por "Tablero/Nombre" si el nombre se repite.
fn find_clip<'a>(state: &'a State, name: &str) -> Result<&'a Clip, String> {
    let wanted = state::fold(name);
    let found: Vec<&Clip> = state
        .clips
        .iter()
        .filter(|c| {
            state::fold(&c.name) == wanted
                || state::fold(&format!("{}/{}", board_name(state, c.board_id), c.name))
                    == wanted.replace(" / ", "/")
        })
        .collect();
    match found.as_slice() {
        [c] => Ok(c),
        [] => Err(tr!(
            "No hay un clip llamado \"{name}\".",
            "There's no clip named \"{name}\"."
        )),
        many => Err(tr!(
            "Hay {} clips con ese nombre; usa Tablero/Nombre: {}",
            "{} clips share that name; use Board/Name: {}",
            many.len(),
            many.iter()
                .map(|c| format!("{}/{}", board_name(state, c.board_id), c.name))
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

fn board_name(state: &State, id: i64) -> &str {
    state
        .boards
        .iter()
        .find(|b| b.id == id)
        .map_or("", |b| b.name.as_str())
}

/// Un error de la base que dejó la operación (State los guarda en vez de devolverlos).
fn check(state: &mut State) -> Result<(), String> {
    state.error.take().map_or(Ok(()), Err)
}

pub fn run(cmd: &Command, state: &mut State) -> Result<Output, String> {
    let text_only = |text: String| Ok(Output { text, copy: None });
    match cmd {
        Command::Help => text_only(help()),
        Command::List(n) => {
            let lines: Vec<String> = ordered(state)
                .into_iter()
                .take(*n)
                .enumerate()
                .map(|(i, e)| line(i + 1, e, state))
                .collect();
            text_only(if lines.is_empty() {
                t("El historial está vacío.", "History is empty.").into()
            } else {
                lines.join("\n")
            })
        }
        Command::Search(q) => {
            let query = Query::parse(q);
            let now = state::now();
            let lines: Vec<String> = ordered(state)
                .into_iter()
                .enumerate()
                .filter(|(_, e)| query.matches_entry(e, state.app_name(e.app), now))
                .map(|(i, e)| line(i + 1, e, state))
                .collect();
            text_only(if lines.is_empty() {
                t("Sin resultados.", "No results.").into()
            } else {
                lines.join("\n")
            })
        }
        Command::Show(n) => {
            let e = nth(state, *n)?;
            text_only(match &e.image {
                Some(img) => {
                    let head = tr!("[imagen {}×{}]", "[image {}×{}]", img.width, img.height);
                    match &img.ocr {
                        Some(text) if !text.is_empty() => format!("{head}\n{text}"),
                        _ => head,
                    }
                }
                None => e.text.clone(),
            })
        }
        Command::Copy(n) => {
            let e = nth(state, *n)?;
            let copy = match e.image {
                Some(_) => Copy::Image(e.id),
                None => Copy::Text {
                    text: e.text.clone(),
                    private: false,
                },
            };
            Ok(Output {
                text: tr!("Copiado: {}", "Copied: {}", line(*n, e, state).trim_start()),
                copy: Some(copy),
            })
        }
        Command::Add(text) => {
            let text = text.as_deref().unwrap_or_default();
            if text.trim().is_empty() {
                return Err(t("No hay texto para guardar.", "There's no text to save.").into());
            }
            if text.len() > state::MAX_TEXT_BYTES {
                return Err(t(
                    "El texto es demasiado grande (más de 256 KB).",
                    "The text is too large (over 256 KB).",
                )
                .into());
            }
            state.capture(text.to_owned(), Some("pega-cli".into()));
            check(state)?;
            text_only(t("Guardado en el historial.", "Saved to history.").into())
        }
        Command::Delete(n) => {
            let e = nth(state, *n)?;
            let (id, shown) = (e.id, line(*n, e, state));
            state.delete_entry(id);
            check(state)?;
            text_only(tr!("Borrado: {}", "Deleted: {}", shown.trim_start()))
        }
        Command::Pin(n) => {
            let id = nth(state, *n)?.id;
            state.toggle_pin(id);
            check(state)?;
            let pinned = state.history.iter().any(|e| e.id == id && e.pinned);
            text_only(
                if pinned {
                    t(
                        "Fijada (ahora va primero en la lista).",
                        "Pinned (now first in the list).",
                    )
                } else {
                    t("Ya no está fijada.", "No longer pinned.")
                }
                .into(),
            )
        }
        Command::Clips => {
            let lines: Vec<String> = state
                .boards
                .iter()
                .flat_map(|b| {
                    state
                        .clips
                        .iter()
                        .filter(move |c| c.board_id == b.id)
                        .map(move |c| format!("{} / {}", b.name, c.name))
                })
                .collect();
            text_only(if lines.is_empty() {
                t("No hay clips.", "There are no clips.").into()
            } else {
                lines.join("\n")
            })
        }
        Command::Clip(name) => {
            let c = find_clip(state, name)?;
            Ok(Output {
                text: tr!(
                    "Copiado el clip {} / {}",
                    "Copied clip {} / {}",
                    board_name(state, c.board_id),
                    c.name
                ),
                copy: Some(Copy::Text {
                    text: c.text.clone(),
                    private: true,
                }),
            })
        }
        Command::ShowClip(name) => text_only(find_clip(state, name)?.text.clone()),
        Command::NewClip(path, text) => {
            let (board, name) = split_path(path).ok_or_else(|| {
                t(
                    "Indica el clip como Tablero/Nombre.",
                    "Give the clip as Board/Name.",
                )
                .to_owned()
            })?;
            let text = text.as_deref().unwrap_or_default();
            if text.trim().is_empty() {
                return Err(t("El clip no tiene texto.", "The clip has no text.").into());
            }
            let board_id = match state
                .boards
                .iter()
                .find(|b| state::fold(&b.name) == state::fold(board))
            {
                Some(b) => b.id,
                None => {
                    let id = state.add_board(board);
                    check(state)?;
                    id.ok_or_else(|| {
                        t("No se pudo crear el tablero.", "Couldn't create the board.").to_owned()
                    })?
                }
            };
            let existing = state
                .clips
                .iter()
                .find(|c| c.board_id == board_id && state::fold(&c.name) == state::fold(name))
                .map(|c| (c.id, c.name.clone()));
            // Al reemplazar se conserva el nombre como estaba escrito.
            let (id, name) = match existing {
                Some((id, old)) => (Some(id), old),
                None => (None, name.to_owned()),
            };
            state.save_clip(id, board_id, &name, text);
            check(state)?;
            text_only(match id {
                Some(_) => tr!(
                    "Clip reemplazado: {board} / {name}",
                    "Clip replaced: {board} / {name}"
                ),
                None => tr!(
                    "Clip creado: {board} / {name}",
                    "Clip created: {board} / {name}"
                ),
            })
        }
        Command::DeleteClip(name) => {
            let c = find_clip(state, name)?;
            let (id, shown) = (
                c.id,
                format!("{} / {}", board_name(state, c.board_id), c.name),
            );
            state.delete_clip(id);
            check(state)?;
            text_only(tr!("Clip borrado: {shown}", "Clip deleted: {shown}"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;
    use std::path::Path;

    fn args(s: &str) -> Vec<String> {
        s.split_whitespace().map(str::to_owned).collect()
    }

    fn sample() -> State {
        let mut s = State::load(Store::open(Path::new(":memory:")).unwrap()).unwrap();
        s.capture("primero".into(), None);
        s.capture("{\"a\": 1}".into(), Some("code.exe".into()));
        s.capture("tercero".into(), None);
        let a = s.add_board("Info").unwrap();
        let b = s.add_board("Trabajo").unwrap();
        s.save_clip(None, a, "Email", "yo@casa.com");
        s.save_clip(None, b, "Email", "yo@trabajo.com");
        s.save_clip(None, a, "Cédula", "123");
        s
    }

    #[test]
    fn parses_commands() {
        assert_eq!(parse(&args("")).unwrap(), Command::Help);
        assert_eq!(parse(&args("list")).unwrap(), Command::List(10));
        assert_eq!(parse(&args("lista 3")).unwrap(), Command::List(3));
        assert_eq!(
            parse(&args("search hola app:chrome")).unwrap(),
            Command::Search("hola app:chrome".into())
        );
        assert_eq!(parse(&args("copy 2")).unwrap(), Command::Copy(2));
        assert!(parse(&args("copy")).is_err());
        assert!(parse(&args("copy 0")).is_err());
        assert!(parse(&args("volar")).is_err());
        assert_eq!(
            parse(&args("clip Info/Email")).unwrap(),
            Command::Clip("Info/Email".into())
        );
    }

    #[test]
    fn list_search_and_copy_share_numbers() {
        let mut s = sample();
        let list = run(&Command::List(10), &mut s).unwrap().text;
        assert!(list.starts_with("  1  tercero"), "{list}");
        assert!(list.contains("[json · code.exe]"));
        let found = run(&Command::Search("lang:json".into()), &mut s)
            .unwrap()
            .text;
        assert!(found.trim_start().starts_with("2  {\"a\": 1}"), "{found}");
        let out = run(&Command::Copy(2), &mut s).unwrap();
        assert_eq!(
            out.copy,
            Some(Copy::Text {
                text: "{\"a\": 1}".into(),
                private: false
            })
        );
        assert!(run(&Command::Copy(9), &mut s).is_err());
    }

    #[test]
    fn parses_write_commands() {
        assert_eq!(
            parse(&args("add hola mundo")).unwrap(),
            Command::Add(Some("hola mundo".into()))
        );
        assert_eq!(parse(&args("add")).unwrap(), Command::Add(None));
        assert_eq!(parse(&args("agregar -")).unwrap(), Command::Add(None));
        assert_eq!(parse(&args("borrar 2")).unwrap(), Command::Delete(2));
        assert_eq!(parse(&args("fijar 1")).unwrap(), Command::Pin(1));
        assert_eq!(parse(&args("ver 3")).unwrap(), Command::Show(3));
        assert_eq!(
            parse(&args("new-clip Info/Tel 300 123")).unwrap(),
            Command::NewClip("Info/Tel".into(), Some("300 123".into()))
        );
        assert!(parse(&args("new-clip SinBarra texto")).is_err());
        assert!(parse(&args("new-clip /Nombre texto")).is_err());
        assert!(Command::Delete(1).writes() && !Command::Show(1).writes());
    }

    #[test]
    fn show_add_pin_and_delete() {
        let mut s = sample();
        assert_eq!(run(&Command::Show(2), &mut s).unwrap().text, "{\"a\": 1}");
        run(&Command::Add(Some("desde la terminal".into())), &mut s).unwrap();
        assert_eq!(s.history[0].text, "desde la terminal");
        assert_eq!(s.app_name(s.history[0].app), Some("pega-cli"));
        assert!(run(&Command::Add(Some("   ".into())), &mut s).is_err());

        // Fijar la 3 la pasa a ser la 1.
        run(&Command::Pin(3), &mut s).unwrap();
        assert_eq!(run(&Command::Show(1), &mut s).unwrap().text, "{\"a\": 1}");
        run(&Command::Delete(1), &mut s).unwrap();
        assert!(!s.history.iter().any(|e| e.text == "{\"a\": 1}"));
        assert!(run(&Command::Delete(99), &mut s).is_err());
    }

    #[test]
    fn new_show_and_delete_clips() {
        let mut s = sample();
        run(
            &Command::NewClip("Nuevo/Tel".into(), Some("300".into())),
            &mut s,
        )
        .unwrap();
        assert!(
            s.boards.iter().any(|b| b.name == "Nuevo"),
            "crea el tablero"
        );
        let out = run(
            &Command::NewClip("nuevo/tel".into(), Some("301".into())),
            &mut s,
        )
        .unwrap();
        assert!(out.text.contains("reemplazado"), "{}", out.text);
        assert_eq!(
            run(&Command::ShowClip("tel".into()), &mut s).unwrap().text,
            "301"
        );
        assert_eq!(s.clips.iter().filter(|c| c.name == "Tel").count(), 1);
        run(&Command::DeleteClip("Nuevo/Tel".into()), &mut s).unwrap();
        assert!(run(&Command::ShowClip("tel".into()), &mut s).is_err());
        assert!(run(&Command::NewClip("Info/X".into(), Some(" ".into())), &mut s).is_err());
    }

    #[test]
    fn clips_by_name_or_board() {
        let mut s = sample();
        assert!(
            run(&Command::Clips, &mut s)
                .unwrap()
                .text
                .contains("Trabajo / Email")
        );
        let out = run(&Command::Clip("cedula".into()), &mut s).unwrap();
        assert_eq!(
            out.copy,
            Some(Copy::Text {
                text: "123".into(),
                private: true
            })
        );
        assert!(
            run(&Command::Clip("email".into()), &mut s).is_err(),
            "ambiguo"
        );
        let out = run(&Command::Clip("trabajo/email".into()), &mut s).unwrap();
        assert_eq!(
            out.copy,
            Some(Copy::Text {
                text: "yo@trabajo.com".into(),
                private: true
            })
        );
    }
}
