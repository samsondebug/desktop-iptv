//! Name normalization at index time: `Pokémon` == `pokemon` (CLAUDE.md §0B, §5).
//!
//! lowercase → NFD → drop combining marks → drop punctuation → collapse whitespace.

use unicode_normalization::char::is_combining_mark;
use unicode_normalization::UnicodeNormalization;

pub fn normalize_name(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut last_space = true; // trims leading whitespace
    for ch in input.nfd() {
        if is_combining_mark(ch) {
            continue;
        }
        let ch = ch.to_lowercase().next().unwrap_or(ch);
        if ch.is_alphanumeric() {
            out.push(ch);
            last_space = false;
        } else if (ch.is_whitespace() || ch.is_ascii_punctuation() || is_separator_like(ch)) && !last_space {
            out.push(' ');
            last_space = true;
        }
        // everything else (control chars, symbols like ★) is dropped
    }
    if out.ends_with(' ') {
        out.pop();
    }
    out
}

fn is_separator_like(ch: char) -> bool {
    matches!(ch, '|' | '·' | '•' | '–' | '—' | '«' | '»' | '“' | '”' | '‘' | '’' | '…')
}

#[cfg(test)]
mod tests {
    use super::normalize_name;

    #[test]
    fn diacritics_and_case() {
        assert_eq!(normalize_name("Pokémon"), "pokemon");
        assert_eq!(normalize_name("POKEMON"), "pokemon");
        assert_eq!(normalize_name("Télé Québec"), "tele quebec");
        assert_eq!(normalize_name("Straße"), "straße");
    }

    #[test]
    fn punctuation_and_spacing() {
        assert_eq!(normalize_name("US: ESPN | HD ★"), "us espn hd");
        assert_eq!(normalize_name("  BBC   One (UK)  "), "bbc one uk");
        assert_eq!(normalize_name("FR| TF1 4K"), "fr tf1 4k");
        assert_eq!(normalize_name(""), "");
        assert_eq!(normalize_name("★★★"), "");
    }

    #[test]
    fn non_latin_preserved() {
        assert_eq!(normalize_name("Россия 1"), "россия 1");
        assert_eq!(normalize_name("العربية HD"), "العربية hd");
    }
}
