//! Plantillas en los clips de tablero: `{fecha}`, `{fecha_iso}`, `{hora}`, `{portapapeles}`
//! y `{campo:Nombre}` (se pide al pegar). También en inglés: `{date}`, `{date_iso}`,
//! `{time}`, `{clipboard}`, `{field:Name}`. `{{` y `}}` escriben llaves literales; lo
//! que no se reconoce queda tal cual.

use std::collections::HashMap;
use windows_sys::Win32::Foundation::SYSTEMTIME;
use windows_sys::Win32::System::SystemInformation::GetLocalTime;

/// Valores del momento de pegar.
pub struct Vars {
    pub date: String,
    pub date_iso: String,
    pub time: String,
    pub clipboard: String,
}

impl Vars {
    pub fn now(clipboard: String) -> Self {
        let mut t: SYSTEMTIME = unsafe { std::mem::zeroed() };
        unsafe { GetLocalTime(&mut t) };
        Self {
            date: format!("{:02}/{:02}/{:04}", t.wDay, t.wMonth, t.wYear),
            date_iso: format!("{:04}-{:02}-{:02}", t.wYear, t.wMonth, t.wDay),
            time: format!("{:02}:{:02}", t.wHour, t.wMinute),
            clipboard,
        }
    }
}

enum Part<'a> {
    Literal(&'a str),
    Var(&'a str),
    Field(&'a str),
}

/// Recorre el texto separando literales, variables y campos.
fn parse(text: &str) -> Vec<Part<'_>> {
    let mut parts = Vec::new();
    let mut rest = text;
    while let Some(i) = rest.find(['{', '}']) {
        if i > 0 {
            parts.push(Part::Literal(&rest[..i]));
        }
        let tail = &rest[i..];
        if tail.starts_with("{{") || tail.starts_with("}}") {
            parts.push(Part::Literal(&tail[..1]));
            rest = &tail[2..];
            continue;
        }
        if tail.starts_with('}') {
            parts.push(Part::Literal("}"));
            rest = &tail[1..];
            continue;
        }
        match tail[1..].find(['}', '{', '\n']) {
            Some(j) if tail.as_bytes()[1 + j] == b'}' => {
                let inner = tail[1..1 + j].trim();
                let field = inner
                    .strip_prefix("campo:")
                    .or_else(|| inner.strip_prefix("field:"))
                    .map(str::trim);
                parts.push(match field {
                    Some(name) if !name.is_empty() => Part::Field(name),
                    _ => Part::Var(&tail[..j + 2]),
                });
                rest = &tail[j + 2..];
            }
            _ => {
                parts.push(Part::Literal("{"));
                rest = &tail[1..];
            }
        }
    }
    if !rest.is_empty() {
        parts.push(Part::Literal(rest));
    }
    parts
}

fn var_value<'a>(token: &str, vars: &'a Vars) -> Option<&'a str> {
    let name = token.trim_matches(['{', '}']).trim().to_lowercase();
    Some(match name.as_str() {
        "fecha" | "date" => &vars.date,
        "fecha_iso" | "date_iso" => &vars.date_iso,
        "hora" | "time" => &vars.time,
        "portapapeles" | "clipboard" => &vars.clipboard,
        _ => return None,
    })
}

/// ¿Tiene una variable conocida o un campo? Solo entonces se procesa (y `{{`/`}}` se
/// convierten): así un JSON anidado, que termina en `}}`, se copia intacto.
pub fn is_template(text: &str) -> bool {
    if !text.contains('{') {
        return false;
    }
    let dummy = Vars {
        date: String::new(),
        date_iso: String::new(),
        time: String::new(),
        clipboard: String::new(),
    };
    parse(text).iter().any(|p| match p {
        Part::Field(_) => true,
        Part::Var(v) => var_value(v, &dummy).is_some(),
        Part::Literal(_) => false,
    })
}

/// Nombres de los campos a pedir, sin repetir y en orden de aparición.
pub fn fields(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for p in parse(text) {
        if let Part::Field(name) = p {
            if !out.iter().any(|n| n == name) {
                out.push(name.to_owned());
            }
        }
    }
    out
}

pub fn render(text: &str, vars: &Vars, values: &HashMap<String, String>) -> String {
    let mut out = String::with_capacity(text.len());
    for p in parse(text) {
        match p {
            Part::Literal(l) => out.push_str(l),
            Part::Field(name) => out.push_str(values.get(name).map_or("", String::as_str)),
            Part::Var(token) => out.push_str(var_value(token, vars).unwrap_or(token)),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars() -> Vars {
        Vars {
            date: "26/10/2026".into(),
            date_iso: "2026-10-26".into(),
            time: "13:05".into(),
            clipboard: "copiado".into(),
        }
    }

    #[test]
    fn replaces_variables() {
        let out = render(
            "Hoy {fecha} ({date_iso}) a las {hora}: {portapapeles}",
            &vars(),
            &HashMap::new(),
        );
        assert_eq!(out, "Hoy 26/10/2026 (2026-10-26) a las 13:05: copiado");
    }

    #[test]
    fn asks_for_fields_once_in_order() {
        let text =
            "Hola {campo:Nombre}, tu pedido {field:Pedido} está listo. Gracias {campo:Nombre}.";
        assert_eq!(fields(text), ["Nombre", "Pedido"]);
        let values = HashMap::from([
            ("Nombre".to_owned(), "Ana".to_owned()),
            ("Pedido".to_owned(), "#42".to_owned()),
        ]);
        assert_eq!(
            render(text, &vars(), &values),
            "Hola Ana, tu pedido #42 está listo. Gracias Ana."
        );
    }

    #[test]
    fn escapes_and_unknown_stay() {
        assert_eq!(
            render("{{fecha}} {otra} {fecha", &vars(), &HashMap::new()),
            "{fecha} {otra} {fecha"
        );
        assert_eq!(
            render("json: {\"a\": 1}", &vars(), &HashMap::new()),
            "json: {\"a\": 1}"
        );
    }

    #[test]
    fn detects_templates() {
        assert!(is_template("Fecha: {fecha}"));
        assert!(is_template("{campo:X}"));
        assert!(!is_template("Calle 10 # 20-30"));
        assert!(!is_template("{\"a\": 1}"));
        assert!(!is_template("{desconocido}"));
        assert!(
            !is_template("{\"a\": {\"b\": 1}}"),
            "JSON anidado no es plantilla"
        );
    }
}
