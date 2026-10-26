//! `pega-cli`: el historial y los clips desde la terminal. Los números son los mismos de
//! la ventana (fijados primero, luego lo más reciente), así `copy 3` copia lo que `list`
//! mostró como 3.

use crate::i18n::t;
use crate::query::Query;
use crate::state::{self, Entry, State};
use crate::tr;

#[derive(Debug, PartialEq)]
pub enum Command {
    List(usize),
    Search(String),
    Copy(usize),
    Clips,
    Clip(String),
    Help,
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
         pega-cli list [N]          las últimas N copias (10 por defecto)\n  \
         pega-cli search <texto>    busca; acepta app:, lang:, tipo:, fijado, hace:2h\n  \
         pega-cli copy <N>          copia la entrada N de la lista\n  \
         pega-cli clips             los clips de los tableros\n  \
         pega-cli clip <nombre>     copia un clip (\"Tablero/Nombre\" si se repite)\n",
        "Pega from the terminal\n\n\
         Usage:\n  \
         pega-cli list [N]          the last N copies (default 10)\n  \
         pega-cli search <text>     search; supports app:, lang:, type:, pinned, within:2h\n  \
         pega-cli copy <N>          copy entry N from the list\n  \
         pega-cli clips             board clips\n  \
         pega-cli clip <name>       copy a clip (\"Board/Name\" if repeated)\n",
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
    let rest = || args[1..].join(" ");
    match args.first().map(String::as_str) {
        None | Some("help" | "ayuda" | "-h" | "--help" | "/?") => Ok(Command::Help),
        Some("list" | "lista") => Ok(Command::List(num(args.get(1), Some(10))?)),
        Some("search" | "buscar") if args.len() > 1 => Ok(Command::Search(rest())),
        Some("copy" | "copiar") => Ok(Command::Copy(num(args.get(1), None)?)),
        Some("clips") => Ok(Command::Clips),
        Some("clip") if args.len() > 1 => Ok(Command::Clip(rest())),
        Some(other) => Err(tr!(
            "Comando desconocido: {other}",
            "Unknown command: {other}"
        )),
    }
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

pub fn run(cmd: &Command, state: &State) -> Result<Output, String> {
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
        Command::Copy(n) => {
            let e = ordered(state)
                .get(n - 1)
                .copied()
                .ok_or_else(|| tr!("No hay entrada {n}.", "There's no entry {n}."))?;
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
            let wanted = state::fold(name);
            let board_of = |id: i64| {
                state
                    .boards
                    .iter()
                    .find(|b| b.id == id)
                    .map_or("", |b| b.name.as_str())
            };
            let found: Vec<_> = state
                .clips
                .iter()
                .filter(|c| {
                    state::fold(&c.name) == wanted
                        || state::fold(&format!("{}/{}", board_of(c.board_id), c.name))
                            == wanted.replace(" / ", "/")
                })
                .collect();
            match found.as_slice() {
                [c] => Ok(Output {
                    text: tr!(
                        "Copiado el clip {} / {}",
                        "Copied clip {} / {}",
                        board_of(c.board_id),
                        c.name
                    ),
                    copy: Some(Copy::Text {
                        text: c.text.clone(),
                        private: true,
                    }),
                }),
                [] => Err(tr!(
                    "No hay un clip llamado \"{name}\".",
                    "There's no clip named \"{name}\"."
                )),
                many => Err(tr!(
                    "Hay {} clips con ese nombre; usa Tablero/Nombre: {}",
                    "{} clips share that name; use Board/Name: {}",
                    many.len(),
                    many.iter()
                        .map(|c| format!("{}/{}", board_of(c.board_id), c.name))
                        .collect::<Vec<_>>()
                        .join(", ")
                )),
            }
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
        let s = sample();
        let list = run(&Command::List(10), &s).unwrap().text;
        assert!(list.starts_with("  1  tercero"), "{list}");
        assert!(list.contains("[json · code.exe]"));
        let found = run(&Command::Search("lang:json".into()), &s).unwrap().text;
        assert!(found.trim_start().starts_with("2  {\"a\": 1}"), "{found}");
        let out = run(&Command::Copy(2), &s).unwrap();
        assert_eq!(
            out.copy,
            Some(Copy::Text {
                text: "{\"a\": 1}".into(),
                private: false
            })
        );
        assert!(run(&Command::Copy(9), &s).is_err());
    }

    #[test]
    fn clips_by_name_or_board() {
        let s = sample();
        assert!(
            run(&Command::Clips, &s)
                .unwrap()
                .text
                .contains("Trabajo / Email")
        );
        let out = run(&Command::Clip("cedula".into()), &s).unwrap();
        assert_eq!(
            out.copy,
            Some(Copy::Text {
                text: "123".into(),
                private: true
            })
        );
        assert!(run(&Command::Clip("email".into()), &s).is_err(), "ambiguo");
        let out = run(&Command::Clip("trabajo/email".into()), &s).unwrap();
        assert_eq!(
            out.copy,
            Some(Copy::Text {
                text: "yo@trabajo.com".into(),
                private: true
            })
        );
    }
}
