//! Python's text tests as the steps ported from it need them, so their outputs stay the bytes the
//! Python made: `str.strip`, `float(str)`, `\w`, and whether a name's English truly differs from it
//! (crate::labels, crate::heritage_layers).

use unicode_normalization::char::is_combining_mark;
use unicode_normalization::UnicodeNormalization;

/// Python's `str.isspace` (bidirectional class WS, B or S, or category Zs; Unicode 16).
pub fn space(c: char) -> bool {
    matches!(
        c,
        '\t'..='\r'
            | '\x1c'..='\x1f'
            | ' '
            | '\u{85}'
            | '\u{a0}'
            | '\u{1680}'
            | '\u{2000}'..='\u{200a}'
            | '\u{2028}'
            | '\u{2029}'
            | '\u{202f}'
            | '\u{205f}'
            | '\u{3000}'
    )
}

/// Python's `str.strip()`.
pub fn strip(s: &str) -> &str {
    s.trim_matches(space)
}

/// The zero digit of each run of ten decimal digits (category Nd; Unicode 16).
const ZEROS: [u32; 76] = [
    0x30, 0x660, 0x6f0, 0x7c0, 0x966, 0x9e6, 0xa66, 0xae6, 0xb66, 0xbe6, 0xc66, 0xce6, 0xd66, 0xde6, 0xe50, 0xed0,
    0xf20, 0x1040, 0x1090, 0x17e0, 0x1810, 0x1946, 0x19d0, 0x1a80, 0x1a90, 0x1b50, 0x1bb0, 0x1c40, 0x1c50, 0xa620,
    0xa8d0, 0xa900, 0xa9d0, 0xa9f0, 0xaa50, 0xabf0, 0xff10, 0x104a0, 0x10d30, 0x10d40, 0x11066, 0x110f0, 0x11136,
    0x111d0, 0x112f0, 0x11450, 0x114d0, 0x11650, 0x116c0, 0x116d0, 0x116da, 0x11730, 0x118e0, 0x11950, 0x11bf0,
    0x11c50, 0x11d50, 0x11da0, 0x11f50, 0x16130, 0x16a60, 0x16ac0, 0x16b50, 0x16d70, 0x1ccf0, 0x1d7ce, 0x1d7d8,
    0x1d7e2, 0x1d7ec, 0x1d7f6, 0x1e140, 0x1e2f0, 0x1e4f0, 0x1e5f1, 0x1e950, 0x1fbf0,
];

/// A decimal digit's value (Python's `unicodedata.decimal`).
fn decimal(c: char) -> Option<u32> {
    let c = c as u32;
    let i = ZEROS.partition_point(|&z| z <= c).checked_sub(1)?;
    (c - ZEROS[i] < 10).then(|| c - ZEROS[i])
}

/// Python's `float(s)` of a string: any script's decimal digits, surrounding white space, and
/// underscores between digits; None where it raises ValueError.
pub fn float(s: &str) -> Option<f64> {
    let ascii: String = s
        .chars()
        .map(|c| match decimal(c) {
            Some(d) => char::from_digit(d, 10).unwrap(),
            None if space(c) => ' ',
            None if c.is_ascii() => c,
            None => '?',
        })
        .collect();
    let t = ascii.trim_matches(' ').as_bytes();
    let mut clean = String::with_capacity(t.len());
    for (i, &b) in t.iter().enumerate() {
        if b == b'_' {
            let digit = |j: Option<usize>| j.and_then(|j| t.get(j)).is_some_and(u8::is_ascii_digit);
            if !digit(i.checked_sub(1)) || !digit(Some(i + 1)) {
                return None;
            }
        } else {
            clean.push(b as char);
        }
    }
    // (Rust reads what Python's float does but for these: hexadecimal and the like are refused by
    // both, and its "infinity", "inf" and "nan" are Python's.)
    if clean.is_empty() || clean.contains(' ') {
        return None;
    }
    clean.parse().ok()
}

/// Python's `\w` (str.isalnum, or `_`).
pub fn word_char(c: char) -> bool {
    c == '_' || (c.is_alphanumeric() && !is_combining_mark(c) && !circled(c))
}

/// Letters Rust counts alphabetic and Python doesn't (So): the circled and squared Latin letters.
pub fn circled(c: char) -> bool {
    matches!(c as u32, 0x24B6..=0x24E9 | 0x1F130..=0x1F149 | 0x1F150..=0x1F169 | 0x1F170..=0x1F189)
}

/// Lower-cased (Python's `str.lower`), decomposed (NFKD), without combining marks or anything but
/// letters and digits.
pub fn norm(s: &str) -> String {
    s.to_lowercase().nfkd().filter(|c| word_char(*c) && *c != '_').collect()
}

/// Whether `en` is there and differs from `name` more than in accents, case, punctuation or spacing
/// (Montréal, Montreal).
pub fn differs(name: &str, en: Option<&str>) -> bool {
    en.is_some_and(|en| !en.is_empty() && norm(en) != norm(name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn float_as_python() {
        assert_eq!(float(" 1_000 "), Some(1000.0));
        assert_eq!(float("١٢٣"), Some(123.0));
        assert_eq!(float("\u{3000}5.\t"), Some(5.0));
        assert_eq!(float("-.5e1"), Some(-5.0));
        assert_eq!(float("Infinity"), Some(f64::INFINITY));
        assert!(float("nan").unwrap().is_nan());
        for bad in ["", "1__0", "_1", "1_", "1 0", "0x10", "12k", "1,5", "Ⅻ", "1e"] {
            assert_eq!(float(bad), None, "{bad}");
        }
    }

    #[test]
    fn strip_as_python() {
        assert_eq!(strip("\x1f a b\u{3000}"), "a b");
        assert_eq!(strip("\u{200b}a"), "\u{200b}a");
    }

    #[test]
    fn differs_as_names_py() {
        assert!(!differs("Montréal", Some("Montreal")));
        assert!(!differs("Mont-Blanc", Some("Mont Blanc")));
        assert!(differs("Köln", Some("Cologne")));
        assert!(!differs("Köln", None));
    }
}
