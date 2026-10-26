//! Oculta en pantalla palabras sensibles y números de tarjeta. Solo afecta lo que
//! se muestra: lo que se copia o pega es el texto original.

const DOT: char = '•';

/// Reemplaza por puntos cada aparición de `words` (sin importar mayúsculas) y, si
/// `cards`, los números de tarjeta válidos (Luhn) menos sus últimos 4 dígitos.
pub fn mask(text: &str, words: &[String], cards: bool) -> String {
    if words.is_empty() && !cards {
        return text.to_owned();
    }
    let mut chars: Vec<char> = text.chars().collect();
    let lower: Vec<char> = chars
        .iter()
        .map(|c| c.to_lowercase().next().unwrap_or(*c))
        .collect();
    for word in words {
        let w: Vec<char> = word.chars().flat_map(char::to_lowercase).collect();
        if w.is_empty() || w.len() > lower.len() {
            continue;
        }
        let mut i = 0;
        while i + w.len() <= lower.len() {
            if lower[i..i + w.len()] == w[..] {
                chars[i..i + w.len()].fill(DOT);
                i += w.len();
            } else {
                i += 1;
            }
        }
    }
    if cards {
        mask_cards(&mut chars);
    }
    chars.into_iter().collect()
}

/// Secuencias de 13 a 19 dígitos (con espacios o guiones entre grupos) que pasan Luhn.
fn mask_cards(chars: &mut [char]) {
    let mut i = 0;
    while i < chars.len() {
        if !chars[i].is_ascii_digit() {
            i += 1;
            continue;
        }
        // Toma dígitos y separadores sueltos; termina en el último dígito.
        let start = i;
        let mut end = i;
        let mut digits = Vec::new();
        let mut j = i;
        while j < chars.len() {
            let c = chars[j];
            if c.is_ascii_digit() {
                digits.push(j);
                end = j + 1;
            } else if !((c == ' ' || c == '-')
                && j + 1 < chars.len()
                && chars[j + 1].is_ascii_digit())
            {
                break;
            }
            j += 1;
        }
        if (13..=19).contains(&digits.len()) && luhn(digits.iter().map(|&k| chars[k])) {
            for &k in &digits[..digits.len() - 4] {
                chars[k] = DOT;
            }
        }
        i = end.max(start + 1);
    }
}

fn luhn(digits: impl DoubleEndedIterator<Item = char>) -> bool {
    let sum: u32 = digits
        .rev()
        .enumerate()
        .map(|(i, c)| {
            let d = c.to_digit(10).unwrap_or(0);
            if i % 2 == 1 {
                let x = d * 2;
                if x > 9 { x - 9 } else { x }
            } else {
                d
            }
        })
        .sum();
    sum % 10 == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn masks_words_case_insensitive() {
        let w = vec!["secreto".to_owned()];
        assert_eq!(
            mask("Mi SECRETO es secreto.", &w, false),
            "Mi ••••••• es •••••••."
        );
    }

    #[test]
    fn masks_valid_cards_but_keeps_last_four() {
        // 4111 1111 1111 1111 es la tarjeta de prueba de Visa (pasa Luhn).
        assert_eq!(
            mask("tarjeta 4111 1111 1111 1111 ok", &[], true),
            "tarjeta •••• •••• •••• 1111 ok"
        );
        assert_eq!(
            mask("4111-1111-1111-1111", &[], true),
            "••••-••••-••••-1111"
        );
    }

    #[test]
    fn leaves_other_numbers_alone() {
        // Cédula y teléfono: muy cortos. Número largo que no pasa Luhn: intacto.
        assert_eq!(
            mask("1098765432 y 3001234567", &[], true),
            "1098765432 y 3001234567"
        );
        assert_eq!(mask("4111111111111112", &[], true), "4111111111111112");
    }

    #[test]
    fn handles_accents() {
        let w = vec!["dirección".to_owned()];
        assert_eq!(mask("Mi Dirección: x", &w, false), "Mi •••••••••: x");
    }
}
