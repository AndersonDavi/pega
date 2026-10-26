//! "Pegar como…": transformaciones del texto antes de pegarlo. Sin dependencias: el
//! formateo de JSON es un recorrido de caracteres que respeta las cadenas.

use crate::i18n::t;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Transform {
    Upper,
    Lower,
    Title,
    OneLine,
    Squeeze,
    JsonPretty,
    JsonCompact,
    UrlEncode,
    UrlDecode,
}

pub const ALL: [Transform; 9] = [
    Transform::Upper,
    Transform::Lower,
    Transform::Title,
    Transform::OneLine,
    Transform::Squeeze,
    Transform::JsonPretty,
    Transform::JsonCompact,
    Transform::UrlEncode,
    Transform::UrlDecode,
];

impl Transform {
    pub fn label(self) -> &'static str {
        match self {
            Transform::Upper => t("MAYÚSCULAS", "UPPERCASE"),
            Transform::Lower => t("minúsculas", "lowercase"),
            Transform::Title => t("Tipo Título", "Title Case"),
            Transform::OneLine => t("Una sola línea", "Single line"),
            Transform::Squeeze => t("Quitar espacios extra", "Remove extra spaces"),
            Transform::JsonPretty => t("JSON con formato", "Pretty JSON"),
            Transform::JsonCompact => t("JSON compacto", "Compact JSON"),
            Transform::UrlEncode => t("Codificar URL", "URL-encode"),
            Transform::UrlDecode => t("Decodificar URL", "URL-decode"),
        }
    }

    /// `None` si no aplica (por ejemplo, JSON inválido).
    pub fn apply(self, text: &str) -> Option<String> {
        Some(match self {
            Transform::Upper => text.to_uppercase(),
            Transform::Lower => text.to_lowercase(),
            Transform::Title => title_case(text),
            Transform::OneLine => text.split_whitespace().collect::<Vec<_>>().join(" "),
            Transform::Squeeze => text
                .lines()
                .map(|l| l.split_whitespace().collect::<Vec<_>>().join(" "))
                .collect::<Vec<_>>()
                .join("\n")
                .trim()
                .to_owned(),
            Transform::JsonPretty => json(text, true)?,
            Transform::JsonCompact => json(text, false)?,
            Transform::UrlEncode => url_encode(text),
            Transform::UrlDecode => url_decode(text)?,
        })
    }
}

fn title_case(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut start = true;
    for c in text.chars() {
        if c.is_alphanumeric() {
            if start {
                out.extend(c.to_uppercase());
            } else {
                out.extend(c.to_lowercase());
            }
            start = false;
        } else {
            out.push(c);
            start = true;
        }
    }
    out
}

/// Reformatea JSON: con sangría de 2 espacios (`pretty`) o sin espacios. Valida que
/// llaves, corchetes y comillas estén balanceados; no interpreta los valores.
fn json(text: &str, pretty: bool) -> Option<String> {
    let src = text.trim();
    if !(src.starts_with('{') || src.starts_with('[')) {
        return None;
    }
    let mut out = String::with_capacity(src.len() * 2);
    let mut stack: Vec<char> = Vec::new();
    let mut in_str = false;
    let mut escape = false;
    // Sangría tope: con miles de "[" anidados la salida crecía al cuadrado (gigas).
    let newline = |out: &mut String, depth: usize| {
        if pretty {
            out.push('\n');
            out.extend(std::iter::repeat_n("  ", depth.min(32)));
        }
    };
    let chars: Vec<char> = src.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if in_str {
            out.push(c);
            if escape {
                escape = false;
            } else if c == '\\' {
                escape = true;
            } else if c == '"' {
                in_str = false;
            }
            i += 1;
            continue;
        }
        match c {
            '"' => {
                in_str = true;
                out.push(c);
            }
            '{' | '[' => {
                stack.push(if c == '{' { '}' } else { ']' });
                out.push(c);
                // Objeto o arreglo vacío: queda en la misma línea.
                let next = chars[i + 1..].iter().find(|ch| !ch.is_whitespace());
                if next != Some(stack.last()?) {
                    newline(&mut out, stack.len());
                }
            }
            '}' | ']' => {
                if stack.pop()? != c {
                    return None;
                }
                let prev = out.trim_end().chars().last();
                if prev != Some(if c == '}' { '{' } else { '[' }) {
                    newline(&mut out, stack.len());
                } else {
                    let trimmed = out.trim_end().len();
                    out.truncate(trimmed);
                }
                out.push(c);
            }
            ',' => {
                out.push(c);
                newline(&mut out, stack.len());
            }
            ':' => {
                out.push(':');
                if pretty {
                    out.push(' ');
                }
            }
            c if c.is_whitespace() => {}
            c => out.push(c),
        }
        i += 1;
    }
    (stack.is_empty() && !in_str).then_some(out)
}

fn url_encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len() * 3);
    for b in text.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn url_decode(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' => {
                // Dos dígitos hexadecimales exactos ("%+1" no vale, aunque el parser lo acepte).
                let hex = bytes.get(i + 1..i + 3)?;
                if !hex.iter().all(u8::is_ascii_hexdigit) {
                    return None;
                }
                out.push(u8::from_str_radix(std::str::from_utf8(hex).ok()?, 16).ok()?);
                i += 3;
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8(out).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn case_and_spaces() {
        assert_eq!(Transform::Upper.apply("año ñandú").unwrap(), "AÑO ÑANDÚ");
        assert_eq!(
            Transform::Title.apply("hola MUNDO-cruel").unwrap(),
            "Hola Mundo-Cruel"
        );
        assert_eq!(Transform::OneLine.apply("a\n  b\r\n\tc").unwrap(), "a b c");
        assert_eq!(
            Transform::Squeeze.apply("  a   b \n\n c  ").unwrap(),
            "a b\n\nc"
        );
    }

    #[test]
    fn json_pretty_and_compact() {
        let src = r#"{"a":1,"b":[true,null,"x, y"],"c":{},"d":[]}"#;
        let pretty = Transform::JsonPretty.apply(src).unwrap();
        assert_eq!(
            pretty,
            "{\n  \"a\": 1,\n  \"b\": [\n    true,\n    null,\n    \"x, y\"\n  ],\n  \"c\": {},\n  \"d\": []\n}"
        );
        assert_eq!(Transform::JsonCompact.apply(&pretty).unwrap(), src);
        // Comillas escapadas dentro de cadenas.
        assert_eq!(
            Transform::JsonCompact
                .apply(r#"{ "q" : "dijo \"hola\" }" }"#)
                .unwrap(),
            r#"{"q":"dijo \"hola\" }"}"#
        );
    }

    #[test]
    fn deep_nesting_does_not_explode() {
        // 100 000 "[" anidados: antes la salida crecía al cuadrado (decenas de GB).
        let deep = format!("{}{}", "[".repeat(100_000), "]".repeat(100_000));
        let out = Transform::JsonPretty.apply(&deep).unwrap();
        assert!(out.len() < 20_000_000, "{} bytes", out.len());
    }

    #[test]
    fn url_decode_needs_two_hex_digits() {
        assert_eq!(Transform::UrlDecode.apply("a%20b").unwrap(), "a b");
        assert!(Transform::UrlDecode.apply("%+1").is_none());
        assert!(Transform::UrlDecode.apply("%zz").is_none());
    }

    #[test]
    fn invalid_json_is_rejected() {
        assert!(Transform::JsonPretty.apply("{\"a\": [1, 2}").is_none());
        assert!(Transform::JsonPretty.apply("hola").is_none());
        assert!(
            Transform::JsonPretty
                .apply("{\"a\": \"sin cerrar}")
                .is_none()
        );
    }

    #[test]
    fn url_roundtrip() {
        let s = "Dirección: Calle 10 #20-30 & más";
        let enc = Transform::UrlEncode.apply(s).unwrap();
        assert_eq!(
            enc,
            "Direcci%C3%B3n%3A%20Calle%2010%20%2320-30%20%26%20m%C3%A1s"
        );
        assert_eq!(Transform::UrlDecode.apply(&enc).unwrap(), s);
        assert_eq!(Transform::UrlDecode.apply("a+b%20c").unwrap(), "a b c");
        assert!(Transform::UrlDecode.apply("%ZZ").is_none());
    }
}
