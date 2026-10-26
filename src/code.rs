//! Detección de lenguaje y resaltado de sintaxis, livianos a propósito: heurísticas
//! por puntaje y un tokenizador genérico. syntect (lo estándar) sumaría 5-10 MB de RAM
//! solo en definiciones de gramáticas; esto no guarda nada en memoria y el resaltado
//! se arma solo al pasar el mouse.

use egui::text::{LayoutJob, TextFormat};
use egui::{Color32, FontId};

/// Solo se mira el principio: suficiente para decidir y acota el costo en textos largos.
const SAMPLE: usize = 4096;
const PREVIEW_LINES: usize = 30;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Lang {
    Json,
    Html,
    Xml,
    Css,
    JavaScript,
    Rust,
    Python,
    Sql,
    Shell,
    PowerShell,
    CSharp,
    Markdown,
    Yaml,
}

impl Lang {
    pub fn name(self) -> &'static str {
        match self {
            Lang::Json => "json",
            Lang::Html => "html",
            Lang::Xml => "xml",
            Lang::Css => "css",
            Lang::JavaScript => "js",
            Lang::Rust => "rust",
            Lang::Python => "python",
            Lang::Sql => "sql",
            Lang::Shell => "bash",
            Lang::PowerShell => "powershell",
            Lang::CSharp => "c#",
            Lang::Markdown => "markdown",
            Lang::Yaml => "yaml",
        }
    }

    fn keywords(self) -> &'static [&'static str] {
        match self {
            Lang::Rust => &[
                "fn", "let", "mut", "pub", "use", "mod", "impl", "struct", "enum", "trait",
                "match", "if", "else", "for", "while", "loop", "return", "self", "Self", "crate",
                "where", "async", "await", "move", "const", "static", "unsafe", "true", "false",
                "in", "as", "dyn", "ref", "type",
            ],
            Lang::Python => &[
                "def", "class", "import", "from", "as", "return", "if", "elif", "else", "for",
                "while", "in", "not", "and", "or", "is", "None", "True", "False", "with", "try",
                "except", "finally", "raise", "lambda", "yield", "pass", "break", "continue",
                "self", "async", "await",
            ],
            Lang::JavaScript => &[
                "const",
                "let",
                "var",
                "function",
                "return",
                "if",
                "else",
                "for",
                "while",
                "import",
                "export",
                "from",
                "default",
                "class",
                "new",
                "this",
                "async",
                "await",
                "true",
                "false",
                "null",
                "undefined",
                "typeof",
                "interface",
                "type",
                "extends",
                "implements",
                "try",
                "catch",
                "throw",
                "of",
                "in",
            ],
            Lang::CSharp => &[
                "using",
                "namespace",
                "class",
                "public",
                "private",
                "protected",
                "internal",
                "static",
                "void",
                "var",
                "new",
                "return",
                "if",
                "else",
                "for",
                "foreach",
                "while",
                "in",
                "async",
                "await",
                "true",
                "false",
                "null",
                "string",
                "int",
                "bool",
                "this",
                "interface",
                "override",
                "readonly",
            ],
            Lang::Sql => &[
                "select", "from", "where", "and", "or", "not", "insert", "into", "values",
                "update", "set", "delete", "create", "table", "alter", "drop", "join", "left",
                "right", "inner", "outer", "on", "group", "by", "order", "having", "limit", "as",
                "distinct", "null", "is", "in", "like", "with", "union", "primary", "key", "count",
                "case", "when", "then", "end",
            ],
            Lang::Shell => &[
                "if", "then", "else", "fi", "for", "do", "done", "while", "case", "esac",
                "function", "echo", "export", "sudo", "cd", "local", "return", "in",
            ],
            Lang::PowerShell => &[
                "function", "param", "if", "else", "elseif", "foreach", "for", "while", "return",
                "try", "catch", "finally", "throw", "in", "true", "false", "null",
            ],
            Lang::Yaml | Lang::Json => &["true", "false", "null"],
            _ => &[],
        }
    }

    fn line_comment(self) -> Option<&'static str> {
        match self {
            Lang::Rust | Lang::JavaScript | Lang::CSharp | Lang::Css => Some("//"),
            Lang::Python | Lang::Shell | Lang::PowerShell | Lang::Yaml => Some("#"),
            Lang::Sql => Some("--"),
            _ => None,
        }
    }

    fn case_insensitive(self) -> bool {
        matches!(self, Lang::Sql | Lang::PowerShell)
    }
}

/// Lenguaje más probable, o `None` si parece texto común.
pub fn detect(text: &str) -> Option<Lang> {
    let t = text.trim();
    if t.len() < 8 {
        return None;
    }
    let sample = &t[..floor_char_boundary(t, SAMPLE)];
    let lines: Vec<&str> = sample.lines().collect();
    let multi = lines.len() >= 2;
    let lower = sample.to_lowercase();
    let first = lower.lines().next().unwrap_or("").trim_start();

    // Señales fuertes, aun en una sola línea.
    if (t.starts_with('{') && t.ends_with('}') || t.starts_with('[') && t.ends_with(']'))
        && sample.contains('"')
        && sample.contains(':')
    {
        return Some(Lang::Json);
    }
    if t.starts_with('<') && (sample.contains("</") || sample.contains("/>")) {
        let html = [
            "<!doctype html",
            "<html",
            "<div",
            "<span",
            "<p>",
            "<a ",
            "<body",
            "<head",
            "<script",
        ];
        return Some(if html.iter().any(|h| lower.contains(h)) {
            Lang::Html
        } else {
            Lang::Xml
        });
    }
    let sql_start = [
        "select ",
        "insert into",
        "update ",
        "delete from",
        "create table",
        "with ",
        "alter table",
    ];
    if sql_start.iter().any(|s| first.starts_with(s))
        && [" from ", " into ", " set ", " table ", " as ("]
            .iter()
            .any(|k| lower.contains(k))
    {
        return Some(Lang::Sql);
    }
    if first.starts_with("#!/bin/") || first.starts_with("#!/usr/bin/env bash") {
        return Some(Lang::Shell);
    }
    if !multi {
        return None;
    }

    let count = |needles: &[&str]| needles.iter().filter(|n| sample.contains(*n)).count() as i32;
    let lines_matching =
        |f: &dyn Fn(&str) -> bool| lines.iter().filter(|l| f(l.trim())).count() as i32;
    let mut scores = [
        (
            Lang::Rust,
            count(&[
                "fn ", "let mut ", "impl ", "::", "-> ", "pub ", "use ", "&self", "println!",
                "Vec<",
            ]) * 2,
        ),
        (
            Lang::Python,
            count(&[
                "def ", "import ", "self.", "elif ", "None", "print(", "__init__",
            ]) * 2
                + lines_matching(&|l| l.ends_with(':') && !l.starts_with('{')),
        ),
        (
            Lang::JavaScript,
            count(&[
                "const ",
                "function",
                "=> ",
                "console.",
                "export ",
                "===",
                "document.",
                "require(",
                "async ",
            ]) * 2,
        ),
        (
            Lang::CSharp,
            count(&[
                "using System",
                "namespace ",
                "public class",
                "Console.Write",
                "private ",
                "public void",
            ]) * 2,
        ),
        (
            Lang::PowerShell,
            count(&[
                "Get-",
                "Set-",
                "$env:",
                "Write-Host",
                "-ErrorAction",
                "New-Object",
                "| Where-Object",
            ]) * 2,
        ),
        (
            Lang::Shell,
            count(&[
                "sudo ", "apt ", "echo ", "export ", "fi\n", "done", "&& ", "$HOME", "chmod ",
            ]) * 2,
        ),
        (
            Lang::Css,
            lines_matching(&|l| {
                let Some((prop, val)) = l.split_once(':') else {
                    return false;
                };
                !prop.is_empty()
                    && prop.chars().all(|c| c.is_ascii_lowercase() || c == '-')
                    && val.trim_end().ends_with(';')
            }) * 2
                + count(&["{\n", "}\n"]),
        ),
        (
            Lang::Markdown,
            lines_matching(&|l| {
                l.starts_with("# ") || l.starts_with("## ") || l.starts_with("```")
            }) * 2
                + lines_matching(&|l| l.starts_with("- ") || l.starts_with("* ")),
        ),
        (
            Lang::Yaml,
            lines_matching(&|l| {
                let Some((k, _)) = l.split_once(": ") else {
                    return l.ends_with(':') && !l.contains(' ');
                };
                !k.is_empty() && !k.contains(' ') && !l.ends_with(';') && !l.contains('{')
            }) + count(&["---\n"]) * 2,
        ),
    ];
    // TypeScript cuenta como JS; con anotaciones de tipo es aún más claro.
    scores[2].1 += count(&[": string", ": number", "interface "]) * 2;
    // YAML suelto se confunde con texto "Clave: valor"; exige más evidencia.
    scores[8].1 -= 2;
    let (lang, best) = scores.iter().copied().max_by_key(|(_, s)| *s)?;
    (best >= 4).then_some(lang)
}

fn floor_char_boundary(s: &str, mut i: usize) -> usize {
    if i >= s.len() {
        return s.len();
    }
    while !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

struct Palette {
    text: Color32,
    keyword: Color32,
    string: Color32,
    number: Color32,
    comment: Color32,
    function: Color32,
    key: Color32,
}

fn palette(dark: bool) -> Palette {
    let c = |r, g, b| Color32::from_rgb(r, g, b);
    if dark {
        Palette {
            text: c(0xD4, 0xD4, 0xD4),
            keyword: c(0xC6, 0x78, 0xDD),
            string: c(0x98, 0xC3, 0x79),
            number: c(0xD1, 0x9A, 0x66),
            comment: c(0x7F, 0x84, 0x8E),
            function: c(0x61, 0xAF, 0xEF),
            key: c(0xE0, 0x6C, 0x75),
        }
    } else {
        Palette {
            text: c(0x38, 0x3A, 0x42),
            keyword: c(0xA6, 0x26, 0xA4),
            string: c(0x50, 0xA1, 0x4F),
            number: c(0x98, 0x68, 0x01),
            comment: c(0xA0, 0xA1, 0xA7),
            function: c(0x40, 0x78, 0xF2),
            key: c(0xE4, 0x56, 0x49),
        }
    }
}

/// Primeras líneas del texto, coloreadas, listas para un `ui.label`.
pub fn highlight(text: &str, lang: Lang, dark: bool) -> LayoutJob {
    let p = palette(dark);
    let font = FontId::monospace(12.0);
    let mut job = LayoutJob::default();
    let mut push = |s: &str, color: Color32| {
        job.append(
            s,
            0.0,
            TextFormat {
                font_id: font.clone(),
                color,
                ..Default::default()
            },
        );
    };
    let mut src: String = text
        .lines()
        .take(PREVIEW_LINES)
        .collect::<Vec<_>>()
        .join("\n");
    if text.lines().count() > PREVIEW_LINES {
        src.push_str("\n…");
    }
    let chars: Vec<char> = src.chars().collect();
    let keywords = lang.keywords();
    let line_comment: Option<Vec<char>> = lang.line_comment().map(|c| c.chars().collect());
    let block_comment = matches!(
        lang,
        Lang::Rust | Lang::JavaScript | Lang::CSharp | Lang::Css
    );
    let markup = matches!(lang, Lang::Html | Lang::Xml);
    let starts =
        |i: usize, pat: &[char]| chars.len() >= i + pat.len() && chars[i..i + pat.len()] == *pat;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let take = |from: usize, to: usize| -> String { chars[from..to].iter().collect() };
        // Comentarios.
        if let Some(lc) = &line_comment {
            if starts(i, lc) {
                let end = (i..chars.len())
                    .find(|&j| chars[j] == '\n')
                    .unwrap_or(chars.len());
                push(&take(i, end), p.comment);
                i = end;
                continue;
            }
        }
        if (block_comment && starts(i, &['/', '*'])) || (markup && starts(i, &['<', '!', '-', '-']))
        {
            let close: &[char] = if markup {
                &['-', '-', '>']
            } else {
                &['*', '/']
            };
            let end = (i + 2..chars.len())
                .find(|&j| starts(j, close))
                .map_or(chars.len(), |j| j + close.len());
            push(&take(i, end), p.comment);
            i = end;
            continue;
        }
        if lang == Lang::Markdown && (i == 0 || chars[i - 1] == '\n') && c == '#' {
            let end = (i..chars.len())
                .find(|&j| chars[j] == '\n')
                .unwrap_or(chars.len());
            push(&take(i, end), p.keyword);
            i = end;
            continue;
        }
        // Cadenas (en JSON, si siguen dos puntos, son claves).
        if c == '"' || c == '\'' || (c == '`' && lang == Lang::JavaScript) {
            let mut j = i + 1;
            while j < chars.len() && chars[j] != c && (chars[j] != '\n' || c == '`') {
                j += if chars[j] == '\\' { 2 } else { 1 };
            }
            let end = (j + 1).min(chars.len());
            let is_key = lang == Lang::Json
                && chars[end..].iter().find(|ch| !ch.is_whitespace()) == Some(&':');
            push(&take(i, end), if is_key { p.key } else { p.string });
            i = end;
            continue;
        }
        if markup && c == '<' {
            let end = (i..chars.len())
                .find(|&j| chars[j].is_whitespace() || chars[j] == '>')
                .unwrap_or(chars.len());
            push(&take(i, end), p.key);
            i = end;
            continue;
        }
        if c.is_ascii_digit() && (i == 0 || !is_ident(chars[i - 1])) {
            let end = (i..chars.len())
                .find(|&j| {
                    !(chars[j].is_ascii_alphanumeric() || chars[j] == '.' || chars[j] == '_')
                })
                .unwrap_or(chars.len());
            push(&take(i, end), p.number);
            i = end;
            continue;
        }
        if is_ident(c) {
            let end = (i..chars.len())
                .find(|&j| !is_ident(chars[j]))
                .unwrap_or(chars.len());
            let word = take(i, end);
            let next = chars[end..].iter().find(|ch| **ch != ' ');
            let is_kw = if lang.case_insensitive() {
                keywords.iter().any(|k| k.eq_ignore_ascii_case(&word))
            } else {
                keywords.contains(&word.as_str())
            };
            let color = if is_kw {
                p.keyword
            } else if next == Some(&'(') || next == Some(&'!') && lang == Lang::Rust {
                p.function
            } else if matches!(lang, Lang::Css | Lang::Yaml) && next == Some(&':') {
                p.key
            } else {
                p.text
            };
            push(&word, color);
            i = end;
            continue;
        }
        // Cualquier otro carácter, agrupando hasta el próximo que pueda empezar un token.
        let end = (i + 1..chars.len())
            .find(|&j| {
                let ch = chars[j];
                is_ident(ch) || ch.is_ascii_digit() || "\"'`#/<-".contains(ch)
            })
            .unwrap_or(chars.len());
        push(&take(i, end), p.text);
        i = end;
    }
    job
}

fn is_ident(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '$'
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_common_languages() {
        let cases = [
            ("{\"a\": 1, \"b\": [true, null]}", Some(Lang::Json)),
            ("<div class=\"x\">hola</div>", Some(Lang::Html)),
            ("SELECT id, name FROM users WHERE id = 1", Some(Lang::Sql)),
            (
                "fn main() {\n    let mut v = Vec::new();\n    println!(\"{}\", v.len());\n}",
                Some(Lang::Rust),
            ),
            (
                "def suma(a, b):\n    return a + b\n\nprint(suma(1, 2))",
                Some(Lang::Python),
            ),
            (
                "const x = (a) => a * 2;\nconsole.log(x(3));",
                Some(Lang::JavaScript),
            ),
            (
                "Get-Process | Where-Object CPU\n$env:PATH",
                Some(Lang::PowerShell),
            ),
            (
                ".btn {\n  color: red;\n  margin: 0 auto;\n}",
                Some(Lang::Css),
            ),
            (
                "# Título\n\n- uno\n- dos\n```\ncode\n```",
                Some(Lang::Markdown),
            ),
        ];
        for (text, want) in cases {
            assert_eq!(detect(text), want, "{text}");
        }
    }

    #[test]
    fn plain_text_is_not_code() {
        for text in [
            "Hola, nos vemos mañana a las 3.",
            "Calle 10 # 20-30, Bogotá\nApto 501",
            "Nombre: Anderson\nCiudad: Bogotá",
            "1098765432",
        ] {
            assert_eq!(detect(text), None, "{text}");
        }
    }

    #[test]
    fn highlight_keeps_all_text() {
        let src = "fn main() {\n    // hola\n    let s = \"x\";\n}";
        let job = highlight(src, Lang::Rust, true);
        assert_eq!(job.text, src);
    }
}
