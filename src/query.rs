//! Búsqueda con filtros: palabras sueltas más `app:chrome`, `lang:json`, `tipo:imagen`,
//! `fijado` y `hace:2h`. Sin tildes ni mayúsculas, en español o inglés.

use crate::state::{Entry, fold};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Text,
    Image,
    Code,
}

#[derive(Default, Debug, PartialEq)]
pub struct Query {
    /// Palabras que deben aparecer en el contenido (todas).
    pub terms: Vec<String>,
    pub app: Option<String>,
    pub lang: Option<String>,
    pub kind: Option<Kind>,
    pub pinned: bool,
    /// Solo lo usado en los últimos N milisegundos.
    pub within_ms: Option<i64>,
}

impl Query {
    pub fn parse(input: &str) -> Self {
        let mut q = Query::default();
        for word in fold(input).split_whitespace() {
            let (key, value) = word.split_once(':').unwrap_or((word, ""));
            match (key, value) {
                ("app", v) if !v.is_empty() => q.app = Some(v.to_owned()),
                ("lang" | "lenguaje", v) if !v.is_empty() => q.lang = Some(v.to_owned()),
                ("tipo" | "type", v) => {
                    q.kind = match v {
                        "imagen" | "image" | "img" => Some(Kind::Image),
                        "texto" | "text" => Some(Kind::Text),
                        "codigo" | "code" => Some(Kind::Code),
                        _ => {
                            q.terms.push(word.to_owned());
                            None
                        }
                    }
                }
                ("fijado" | "fijados" | "pinned", "") => q.pinned = true,
                ("hace" | "within", v) => match parse_age(v) {
                    Some(ms) => q.within_ms = Some(ms),
                    None => q.terms.push(word.to_owned()),
                },
                _ => q.terms.push(word.to_owned()),
            }
        }
        q
    }

    /// Hay filtros que solo tienen sentido en el historial (los tableros no tienen app ni fecha).
    pub fn has_history_filters(&self) -> bool {
        self.app.is_some()
            || self.lang.is_some()
            || self.kind.is_some()
            || self.pinned
            || self.within_ms.is_some()
    }

    pub fn matches_text(&self, key: &str) -> bool {
        self.terms.iter().all(|t| key.contains(t.as_str()))
    }

    pub fn matches_entry(&self, e: &Entry, app_name: Option<&str>, now: i64) -> bool {
        if !self.matches_text(&e.key) || (self.pinned && !e.pinned) {
            return false;
        }
        if let Some(app) = &self.app {
            if !app_name.is_some_and(|a| fold(a).contains(app.as_str())) {
                return false;
            }
        }
        if let Some(lang) = &self.lang {
            if !e.lang.is_some_and(|l| l.name() == lang) {
                return false;
            }
        }
        match self.kind {
            Some(Kind::Image) if e.image.is_none() => return false,
            Some(Kind::Text) if e.image.is_some() || e.lang.is_some() => return false,
            Some(Kind::Code) if e.lang.is_none() => return false,
            _ => {}
        }
        self.within_ms.is_none_or(|ms| now - e.used_at <= ms)
    }
}

/// "30m", "2h", "3d" (o "2s" para semanas) a milisegundos.
fn parse_age(v: &str) -> Option<i64> {
    let unit = v.chars().last()?;
    let n: i64 = v[..v.len() - unit.len_utf8()].parse().ok()?;
    let ms = match unit {
        'm' => 60_000,
        'h' => 3_600_000,
        'd' => 86_400_000,
        's' | 'w' => 7 * 86_400_000,
        _ => return None,
    };
    // checked_mul: "hace:9999999999999999d" no cabe en i64.
    (n > 0).then(|| n.checked_mul(ms)).flatten()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::code::Lang;

    fn entry(text: &str, lang: Option<Lang>, image: bool, pinned: bool, age_ms: i64) -> Entry {
        Entry {
            id: 1,
            text: text.into(),
            key: fold(text),
            used_at: 1_000_000_000 - age_ms,
            pinned,
            image: image.then(|| crate::state::ImageInfo {
                width: 1,
                height: 1,
                hash: 0,
                thumb_png: vec![],
                ocr: None,
                sensitive: None,
            }),
            app: None,
            lang,
        }
    }
    const NOW: i64 = 1_000_000_000;

    #[test]
    fn huge_ages_do_not_overflow() {
        assert_eq!(parse_age("9999999999999999d"), None);
        assert_eq!(parse_age("2h"), Some(7_200_000));
    }

    #[test]
    fn parses_filters_and_terms() {
        let q = Query::parse("Factura app:Chrome lang:JSON tipo:código fijado hace:2h");
        assert_eq!(q.terms, ["factura"]);
        assert_eq!(q.app.as_deref(), Some("chrome"));
        assert_eq!(q.lang.as_deref(), Some("json"));
        assert_eq!(q.kind, Some(Kind::Code));
        assert!(q.pinned);
        assert_eq!(q.within_ms, Some(2 * 3_600_000));
    }

    #[test]
    fn unknown_filters_are_plain_words() {
        let q = Query::parse("hace:mucho tipo:raro http://x");
        assert_eq!(q.terms, ["hace:mucho", "tipo:raro", "http://x"]);
        assert!(!q.has_history_filters());
    }

    #[test]
    fn filters_entries() {
        let json = entry("{\"a\": 1}", Some(Lang::Json), false, false, 0);
        let img = entry("", None, true, true, 3 * 86_400_000);
        let text = entry("hola", None, false, false, 0);
        assert!(Query::parse("lang:json").matches_entry(&json, None, NOW));
        assert!(!Query::parse("lang:json").matches_entry(&text, None, NOW));
        assert!(Query::parse("tipo:imagen").matches_entry(&img, None, NOW));
        assert!(Query::parse("tipo:texto").matches_entry(&text, None, NOW));
        assert!(!Query::parse("tipo:texto").matches_entry(&json, None, NOW));
        assert!(Query::parse("fijado").matches_entry(&img, None, NOW));
        assert!(!Query::parse("hace:1d").matches_entry(&img, None, NOW));
        assert!(Query::parse("app:chro").matches_entry(&text, Some("chrome.exe"), NOW));
        assert!(!Query::parse("app:code").matches_entry(&text, Some("chrome.exe"), NOW));
    }
}
