//! `pega-cli.exe`: ejecutable de consola. Un programa con ventana no puede escribir bien
//! en la terminal (la consola no lo espera), por eso esto va aparte.

use pega::cli::{self, Command, Copy};
use pega::{paths, state::State, store::Store, win};

fn fail(msg: &str) -> ! {
    eprintln!("{msg}");
    std::process::exit(1)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cmd = match cli::parse(&args) {
        Ok(cmd) => cmd,
        Err(e) => fail(&format!("{e}\n\n{}", cli::help())),
    };
    if cmd == Command::Help {
        print!("{}", cli::help());
        return;
    }
    let db = paths::database();
    if !db.exists() {
        fail("Pega todavía no tiene datos en este usuario.");
    }
    let store = Store::open(&db).unwrap_or_else(|e| fail(&e.to_string()));
    let mut state = State::load(store).unwrap_or_else(|e| fail(&e.to_string()));
    let out = cli::run(&cmd, &state).unwrap_or_else(|e| fail(&e));
    if let Some(copy) = out.copy {
        win::init_cli_clipboard();
        let ok = match copy {
            Copy::Text { text, private } => win::write_text(&text, private),
            Copy::Image(id) => state
                .image_for_paste(id)
                .is_some_and(|(img, png)| win::write_image(&img, &png)),
        };
        if !ok {
            fail("No se pudo escribir en el portapapeles.");
        }
    }
    println!("{}", out.text);
}
