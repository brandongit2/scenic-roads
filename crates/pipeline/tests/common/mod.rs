//! What the whole-crate checks share (tests/inputs_accessor.rs, tests/write_sets.rs): the
//! repository's files, and a scanner of Rust items by their blocks (a line's code, `#[cfg(test)]`
//! items, the item a line lies in), and of Python functions.
#![allow(dead_code)]

use std::path::{Path, PathBuf};

pub fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().unwrap()
}

pub fn files(dir: &Path, ext: &str, out: &mut Vec<PathBuf>) {
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let p = e.path();
        if p.is_dir() {
            if !matches!(p.file_name().and_then(|n| n.to_str()), Some("target" | "tests" | ".venv" | "node_modules")) {
                files(&p, ext, out);
            }
        } else if p.extension().is_some_and(|x| x == ext) {
            out.push(p);
        }
    }
}

/// A Rust line's code: its string and character literals' contents blanked and its comment cut, so
/// braces can be counted and an accessor call named in a string isn't one.
pub fn code_of(l: &str) -> String {
    let b: Vec<char> = l.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    let mut in_str = false;
    while i < b.len() {
        let c = b[i];
        if in_str {
            if c == '\\' {
                i += 2;
                continue;
            }
            if c == '"' {
                in_str = false;
            }
            out.push(if c == '"' { c } else { ' ' });
        } else if c == '/' && b.get(i + 1) == Some(&'/') {
            break;
        } else if c == '"' {
            in_str = true;
            out.push(c);
        } else if c == '\'' && (b.get(i + 2) == Some(&'\'') || b.get(i + 1) == Some(&'\\')) {
            // (A character literal: '{', '\n'.)
            let end = (i + 1..b.len()).find(|&k| b[k] == '\'' && k > i + 1).unwrap_or(i);
            i = end + 1;
            continue;
        } else {
            out.push(c);
        }
        i += 1;
    }
    out
}

/// Where an item starting at line `at` ends: a function's block by its braces, a `const`'s or a
/// `static`'s at the `;` that closes all it opened (brackets and parentheses too).
pub fn item_end(code: &[String], at: usize, is_fn: bool) -> usize {
    if is_fn {
        return block_end(code, at);
    }
    let mut depth = 0i64;
    for (k, l) in code.iter().enumerate().skip(at) {
        for ch in l.chars() {
            match ch {
                '{' | '[' | '(' => depth += 1,
                '}' | ']' | ')' => depth -= 1,
                _ => {}
            }
        }
        if depth <= 0 && l.trim_end().ends_with(';') {
            return k;
        }
    }
    code.len().saturating_sub(1)
}

/// The line where the block opened at or after line `at` closes (braces counted on `code`).
pub fn block_end(code: &[String], at: usize) -> usize {
    let mut depth = 0i64;
    let mut opened = false;
    for (k, l) in code.iter().enumerate().skip(at) {
        for ch in l.chars() {
            match ch {
                '{' => {
                    depth += 1;
                    opened = true;
                }
                '}' => depth -= 1,
                _ => {}
            }
        }
        if opened && depth <= 0 {
            return k;
        }
        // (An item without a block: a `const` or a `fn` declared, ending at its `;`.)
        if !opened && l.trim_end().ends_with(';') {
            return k;
        }
    }
    code.len().saturating_sub(1)
}

/// The lines inside `#[cfg(test)]` items (a test module, a test helper), each attribute's item to
/// its block's end.
pub fn test_lines(code: &[String]) -> Vec<bool> {
    let mut mask = vec![false; code.len()];
    let mut k = 0;
    while k < code.len() {
        let t = code[k].trim();
        if t.starts_with("#[cfg(test)]") || t.starts_with("#[cfg(all(test") {
            let end = block_end(code, k + 1);
            for m in mask.iter_mut().take(end + 1).skip(k) {
                *m = true;
            }
            k = end + 1;
        } else {
            k += 1;
        }
    }
    mask
}

/// The name an item line starts (a function's, `const fn` and `async fn` too; a `const`'s or a
/// `static`'s), if it starts one.
pub fn item_name(l: &str) -> Option<(String, bool)> {
    let mut t = l.trim_start();
    for p in ["pub(crate) ", "pub(super) ", "pub "] {
        t = t.strip_prefix(p).unwrap_or(t);
    }
    for p in ["const fn ", "async fn ", "unsafe fn ", "fn "] {
        if let Some(r) = t.strip_prefix(p) {
            return Some((r.split(|c: char| !(c.is_alphanumeric() || c == '_')).next().unwrap_or("").to_string(), true));
        }
    }
    for p in ["const ", "static "] {
        if let Some(r) = t.strip_prefix(p) {
            return Some((r.split(|c: char| !(c.is_alphanumeric() || c == '_')).next().unwrap_or("").to_string(), false));
        }
    }
    None
}

/// The innermost item line `i` lies in (a function, else a `const` or `static`), by its block:
/// its name and its code; None at the top level of a file, outside any.
pub fn enclosing_rs(code: &[String], i: usize) -> Option<(String, String)> {
    let mut best: Option<(usize, usize, String)> = None;
    for k in (0..=i).rev() {
        let Some((name, is_fn)) = item_name(&code[k]) else { continue };
        let end = item_end(code, k, is_fn);
        if end >= i && best.as_ref().is_none_or(|b| k > b.0) {
            best = Some((k, end, name));
            // (The nearest that holds it is the innermost.)
            break;
        }
    }
    best.map(|(k, end, name)| (name, code[k..=end].join("\n")))
}

/// The Python function or top-level assignment line `i` lies in (by indentation), its name and its
/// code (comments cut); None outside any.
pub fn enclosing_py(lines: &[&str], i: usize) -> Option<(String, String)> {
    let indent = |l: &str| l.len() - l.trim_start().len();
    let code = |l: &str| py_code(l);
    for k in (0..=i).rev() {
        let t = lines[k].trim_start();
        if let Some(r) = t.strip_prefix("def ").or_else(|| t.strip_prefix("async def ")) {
            let end = (k + 1..lines.len()).find(|&m| !lines[m].trim().is_empty() && indent(lines[m]) <= indent(lines[k])).unwrap_or(lines.len());
            if end > i {
                let name = r.split(|c: char| !(c.is_alphanumeric() || c == '_')).next().unwrap_or("").to_string();
                return Some((name, lines[k..end].iter().map(|l| code(l)).collect::<Vec<_>>().join("\n")));
            }
        }
        if indent(lines[k]) == 0 && !t.is_empty() && !t.starts_with('#') {
            // (A top-level statement: an assignment's name.)
            let name = t.split(|c: char| !(c.is_alphanumeric() || c == '_')).next().unwrap_or("").to_string();
            return (!name.is_empty() && k == i || t.contains('=')).then(|| (name, code(lines[k])));
        }
    }
    None
}

/// A Python line's code: its comment cut and its string literals' contents blanked.
pub fn py_code(l: &str) -> String {
    let mut out = String::new();
    let mut q: Option<char> = None;
    let mut esc = false;
    for c in l.chars() {
        match q {
            Some(open) => {
                if esc {
                    esc = false;
                } else if c == '\\' {
                    esc = true;
                } else if c == open {
                    q = None;
                    out.push(c);
                    continue;
                }
                out.push(' ');
            }
            None if c == '#' => break,
            None => {
                if c == '"' || c == '\'' {
                    q = Some(c);
                }
                out.push(c);
            }
        }
    }
    out
}

