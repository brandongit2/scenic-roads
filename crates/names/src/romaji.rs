//! Kana romanised by rule: modified Hepburn without macrons (docs/plan.md §7), for a thing whose
//! only English source is OSM's kana reading of its name (`name:ja-Hira`, `name:ja_kana`).
//!
//! - Long vowels lose their mark: おう, おお and うう are o, o and u (とうきょう Tokyo, おおさか
//!   Osaka), and the katakana bar is dropped; えい stays ei.
//! - ん is n, and n' before a vowel or y (しんおおさか Shin'osaka); っ doubles the next consonant (ch
//!   becomes tch).
//! - Words (split on spaces and the middle dot) are capitalised.
//!
//! A reading with anything but kana, spaces and the marks above gives nothing.

/// The romanisation of a kana reading, or `None` when it holds other characters.
pub fn hepburn(kana: &str) -> Option<String> {
    let chars: Vec<char> = kana.trim().chars().map(to_hiragana).collect();
    if chars.is_empty() {
        return None;
    }
    let mut syl: Vec<String> = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let next = chars.get(i + 1).copied();
        // A yoon: a kana in i followed by a small ya, yu, yo.
        if let Some(small @ ('ゃ' | 'ゅ' | 'ょ')) = next {
            if let Some(r) = yoon(c, small) {
                syl.push(r.to_owned());
                i += 2;
                continue;
            }
        }
        match c {
            ' ' | '　' | '・' => syl.push(" ".into()),
            'ー' | '〜' => syl.push("-".into()),
            'っ' => syl.push("*".into()),
            _ => syl.push(base(c)?.to_owned()),
        }
        i += 1;
    }
    // Sokuon, n before vowels, long vowels.
    let mut out = String::new();
    for k in 0..syl.len() {
        let s = syl[k].as_str();
        match s {
            "*" => {
                if let Some(n) = syl.get(k + 1) {
                    if n.starts_with("ch") {
                        out.push('t');
                    } else if let Some(c) = n.chars().next().filter(|c| c.is_ascii_alphabetic() && !"aeiou".contains(*c)) {
                        out.push(c);
                    }
                }
            }
            "-" => {}
            "n" => {
                out.push('n');
                if syl.get(k + 1).and_then(|n| n.chars().next()).is_some_and(|c| "aeiouy".contains(c)) {
                    out.push('\'');
                }
            }
            "u" if out.ends_with('o') || out.ends_with('u') => {}
            "o" if out.ends_with('o') => {}
            _ => out.push_str(s),
        }
    }
    let words: Vec<String> = out
        .split(' ')
        .filter(|w| !w.is_empty())
        .map(|w| {
            let mut c = w.chars();
            c.next().map(|f| f.to_ascii_uppercase().to_string() + c.as_str()).unwrap_or_default()
        })
        .collect();
    (!words.is_empty()).then(|| words.join(" "))
}

/// Katakana to hiragana (the long-vowel bar and the middle dot kept).
fn to_hiragana(c: char) -> char {
    match c {
        'ァ'..='ヶ' => char::from_u32(c as u32 - 0x60).unwrap_or(c),
        _ => c,
    }
}

fn yoon(c: char, small: char) -> Option<&'static str> {
    let stem = match c {
        'き' => "ky",
        'ぎ' => "gy",
        'し' => "sh",
        'じ' | 'ぢ' => "j",
        'ち' => "ch",
        'に' => "ny",
        'ひ' => "hy",
        'び' => "by",
        'ぴ' => "py",
        'み' => "my",
        'り' => "ry",
        _ => return None,
    };
    let v = match small {
        'ゃ' => "a",
        'ゅ' => "u",
        _ => "o",
    };
    Some(match (stem, v) {
        ("ky", "a") => "kya", ("ky", "u") => "kyu", ("ky", _) => "kyo",
        ("gy", "a") => "gya", ("gy", "u") => "gyu", ("gy", _) => "gyo",
        ("sh", "a") => "sha", ("sh", "u") => "shu", ("sh", _) => "sho",
        ("j", "a") => "ja", ("j", "u") => "ju", ("j", _) => "jo",
        ("ch", "a") => "cha", ("ch", "u") => "chu", ("ch", _) => "cho",
        ("ny", "a") => "nya", ("ny", "u") => "nyu", ("ny", _) => "nyo",
        ("hy", "a") => "hya", ("hy", "u") => "hyu", ("hy", _) => "hyo",
        ("by", "a") => "bya", ("by", "u") => "byu", ("by", _) => "byo",
        ("py", "a") => "pya", ("py", "u") => "pyu", ("py", _) => "pyo",
        ("my", "a") => "mya", ("my", "u") => "myu", ("my", _) => "myo",
        ("ry", "a") => "rya", ("ry", "u") => "ryu", _ => "ryo",
    })
}

fn base(c: char) -> Option<&'static str> {
    Some(match c {
        'あ' | 'ぁ' => "a", 'い' | 'ぃ' => "i", 'う' | 'ぅ' => "u", 'え' | 'ぇ' => "e", 'お' | 'ぉ' => "o",
        'か' => "ka", 'き' => "ki", 'く' => "ku", 'け' => "ke", 'こ' => "ko",
        'が' => "ga", 'ぎ' => "gi", 'ぐ' => "gu", 'げ' => "ge", 'ご' => "go",
        'さ' => "sa", 'し' => "shi", 'す' => "su", 'せ' => "se", 'そ' => "so",
        'ざ' => "za", 'じ' => "ji", 'ず' => "zu", 'ぜ' => "ze", 'ぞ' => "zo",
        'た' => "ta", 'ち' => "chi", 'つ' => "tsu", 'て' => "te", 'と' => "to",
        'だ' => "da", 'ぢ' => "ji", 'づ' => "zu", 'で' => "de", 'ど' => "do",
        'な' => "na", 'に' => "ni", 'ぬ' => "nu", 'ね' => "ne", 'の' => "no",
        'は' => "ha", 'ひ' => "hi", 'ふ' => "fu", 'へ' => "he", 'ほ' => "ho",
        'ば' => "ba", 'び' => "bi", 'ぶ' => "bu", 'べ' => "be", 'ぼ' => "bo",
        'ぱ' => "pa", 'ぴ' => "pi", 'ぷ' => "pu", 'ぺ' => "pe", 'ぽ' => "po",
        'ま' => "ma", 'み' => "mi", 'む' => "mu", 'め' => "me", 'も' => "mo",
        'や' | 'ゃ' => "ya", 'ゆ' | 'ゅ' => "yu", 'よ' | 'ょ' => "yo",
        'ら' => "ra", 'り' => "ri", 'る' => "ru", 'れ' => "re", 'ろ' => "ro",
        'わ' | 'ゎ' => "wa", 'ゐ' => "i", 'ゑ' => "e", 'を' => "o", 'ん' => "n",
        'ゔ' => "vu", 'ゕ' => "ka", 'ゖ' => "ke",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::hepburn;

    #[test]
    fn readings() {
        let h = |s: &str| hepburn(s);
        assert_eq!(h("とうきょう").as_deref(), Some("Tokyo"));
        assert_eq!(h("おおさか").as_deref(), Some("Osaka"));
        assert_eq!(h("キョウト").as_deref(), Some("Kyoto"));
        assert_eq!(h("ほっかいどう").as_deref(), Some("Hokkaido"));
        assert_eq!(h("しんおおさか").as_deref(), Some("Shin'osaka"));
        assert_eq!(h("ぐんま").as_deref(), Some("Gunma"));
        assert_eq!(h("まっちゃ").as_deref(), Some("Matcha"));
        assert_eq!(h("さっぽろ").as_deref(), Some("Sapporo"));
        assert_eq!(h("ふじさん").as_deref(), Some("Fujisan"));
        assert_eq!(h("めいじ じんぐう").as_deref(), Some("Meiji Jingu"));
        assert_eq!(h("ラーメン").as_deref(), Some("Ramen"));
        assert_eq!(h("センター・ビル").as_deref(), Some("Senta Biru"));
        assert_eq!(h("ほんまち").as_deref(), Some("Honmachi"));
        assert_eq!(h("きんかくじ").as_deref(), Some("Kinkakuji"));
        assert_eq!(h("ちゅうおう").as_deref(), Some("Chuo"));
        assert_eq!(h("東京"), None);
        assert_eq!(h(""), None);
        assert_eq!(h("ー"), None);
    }
}
