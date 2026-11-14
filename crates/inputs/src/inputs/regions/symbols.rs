//! The regions' symbols (docs/inputs.md §6.1.3, "Symbols"): a region's or a folder's flag, coat of
//! arms or seal, declared in its region file's or `_group.toml`'s `[symbol]` table, its image a
//! file of the inputs folder named by file name alone: a loose `.svg`, `.png` or `.jpg` anywhere,
//! or a member of a `.zip` anywhere (a bundle). The check reads the table (`faults`), each image's
//! bytes (`image`), each bundle's members (`bundle`), and, over a version, that every image named
//! is there once (crate::inputs::regions's `whole`). Nothing the build makes reads them: the map's
//! server serves them from its copy of the accepted version (`Images`, crates/server/src/regions.rs).

use crate::inputs::Index;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The most bytes an image may have (a member of a bundle too, inflated).
pub const MAX_BYTES: u64 = 64 << 10;
/// The most bytes a bundle's members may have together, inflated (refused before they're inflated).
pub const MAX_BUNDLE: u64 = 64 << 20;
/// The most pixels a PNG or JPEG may have on a side.
pub const MAX_PX: u32 = 512;
/// The most elements an SVG may draw, each `<use>` counting the elements it copies (`svg`).
pub const MAX_DRAWN: u64 = 1 << 16;
/// What a symbol may be.
pub const KINDS: &[&str] = &["flag", "coat of arms", "seal"];
/// The licences a symbol may be under, a line each (`free`); the producer reads the same file.
const FREE: &str = include_str!("free-licences.txt");

/// A `[symbol]` table: the image's name, what it is, where it comes from and its credit.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Symbol {
    /// A file of the folder by name (`Q1000.svg`): loose anywhere, or a bundle's member.
    pub image: String,
    pub kind: String,
    /// The page it comes from (an `https://` URL), where its author and licence are.
    pub source: String,
    /// Its author as the source names them; empty when the source page is the only credit.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub author: String,
    pub licence: String,
}

/// An image's type, by its name's extension.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Svg,
    Png,
    Jpeg,
}

impl Kind {
    /// The type of a file named `name`, if it's an image's name.
    pub fn of(name: &str) -> Option<Kind> {
        match name.rsplit_once('.')?.1 {
            "svg" => Some(Kind::Svg),
            "png" => Some(Kind::Png),
            "jpg" => Some(Kind::Jpeg),
            _ => None,
        }
    }

    pub fn ext(self) -> &'static str {
        match self {
            Kind::Svg => "svg",
            Kind::Png => "png",
            Kind::Jpeg => "jpg",
        }
    }

    pub fn mime(self) -> &'static str {
        match self {
            Kind::Svg => "image/svg+xml; charset=utf-8",
            Kind::Png => "image/png",
            Kind::Jpeg => "image/jpeg",
        }
    }
}

/// Whether a path is a bundle of images (`.zip`).
pub fn is_bundle(path: &str) -> bool {
    path.ends_with(".zip")
}

/// Whether `licence` is on the free list (word by word: `{version}` a version such as 4.0,
/// `{port}` two lower-case letters).
pub fn free(licence: &str) -> bool {
    let words: Vec<&str> = licence.split(' ').collect();
    FREE.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')).any(|line| {
        let pat: Vec<&str> = line.split(' ').collect();
        pat.len() == words.len()
            && pat.iter().zip(&words).all(|(p, w)| match *p {
                "{version}" => {
                    let b = w.as_bytes();
                    b.len() == 3 && b[0].is_ascii_digit() && b[1] == b'.' && b[2].is_ascii_digit()
                }
                "{port}" => w.len() == 2 && w.bytes().all(|c| c.is_ascii_lowercase()),
                p => p == *w,
            })
    })
}

/// What's wrong with a table (§6.1.8 `rg-symbol`), each a message; none when it's sound.
pub fn faults(s: &Symbol) -> Vec<String> {
    let mut out = Vec::new();
    if s.image.contains('/') || Kind::of(&s.image).is_none() || s.image.starts_with(['.', '_']) {
        out.push(format!("symbol image {:?}: a file's name alone, ending .svg, .png or .jpg", s.image));
    }
    if !KINDS.contains(&s.kind.as_str()) {
        out.push(format!("symbol kind {:?}: one of {}", s.kind, KINDS.join(", ")));
    }
    if !s.source.starts_with("https://") || s.source.len() <= "https://".len() {
        out.push(format!("symbol source {:?}: the page it comes from, an https:// address", s.source));
    }
    if !free(&s.licence) {
        out.push(format!(
            "symbol licence {:?} isn't on the free list (crates/inputs/src/inputs/regions/free-licences.txt)",
            s.licence
        ));
    }
    out
}

/// Why an image named `name` can't be served, if it can't: its size, its type by its bytes (the
/// name's), a raster's sides, an SVG's content (`svg`).
pub fn image(name: &str, bytes: &[u8]) -> Result<(), String> {
    let Some(kind) = Kind::of(name) else { return Err(format!("{name}: not an .svg, .png or .jpg")) };
    if bytes.len() as u64 > MAX_BYTES {
        return Err(format!("{name}: {} bytes, more than the {} an image may have", bytes.len(), MAX_BYTES));
    }
    let sides = match kind {
        Kind::Svg => {
            let text = std::str::from_utf8(bytes).map_err(|_| format!("{name}: not UTF-8 text (an SVG)"))?;
            return svg(text).map_err(|e| format!("{name}: {e}"));
        }
        Kind::Png => png_sides(bytes).ok_or_else(|| format!("{name}: not a PNG"))?,
        Kind::Jpeg => jpeg_sides(bytes).ok_or_else(|| format!("{name}: not a JPEG"))?,
    };
    if sides.0 == 0 || sides.1 == 0 || sides.0 > MAX_PX || sides.1 > MAX_PX {
        return Err(format!("{name}: {} × {} px; at most {MAX_PX} on a side", sides.0, sides.1));
    }
    Ok(())
}

/// A bundle's images by name: its members' file names (their folders, macOS's `__MACOSX/` and names
/// starting with `.` passed over, as are members that aren't images), each checked (`image`).
/// Errors: the zip unreadable, a name twice, each bad image.
pub fn bundle(bytes: &[u8]) -> Result<BTreeMap<String, Vec<u8>>, Vec<String>> {
    let members = crate::inputs::gtfs::members(bytes, MAX_BYTES, MAX_BUNDLE).map_err(|e| vec![format!("{e:#}")])?;
    let (mut out, mut bad) = (BTreeMap::new(), Vec::new());
    for (path, data) in members {
        let name = path.rsplit('/').next().unwrap_or(&path).to_string();
        if path.starts_with("__MACOSX/") || name.starts_with('.') || Kind::of(&name).is_none() {
            continue;
        }
        if let Err(e) = image(&name, &data) {
            bad.push(e);
        } else if out.insert(name.clone(), data).is_some() {
            bad.push(format!("{name} is in the bundle twice (an image's name is unique across the folder)"));
        }
    }
    if bad.is_empty() {
        Ok(out)
    } else {
        Err(bad)
    }
}

/// The accepted version's images, for the map's server: by content name (`<hash16>.<ext>`), the
/// accepted copy holding it and, for a bundle's, its name in it; and by image name, its content name.
#[derive(Default, Debug, PartialEq)]
pub struct Images {
    pub by_content: BTreeMap<String, (String, Option<String>)>,
    pub by_name: BTreeMap<String, String>,
}

impl Images {
    /// From an accepted index: a loose image's entry (its copy's content name), a bundle's
    /// (`facts.images`: name to hash).
    pub fn of(index: &Index) -> Images {
        let mut out = Images::default();
        for (p, e) in &index.files {
            let name = p.rsplit('/').next().unwrap_or(p);
            if let (Some(k), Some(h)) = (Kind::of(name), crate::inputs::hash_of(&e.file)) {
                let c = format!("{h}.{}", k.ext());
                out.by_name.insert(name.to_string(), c.clone());
                out.by_content.insert(c, (e.file.clone(), None));
            } else if let Some(m) = e.facts.get("images").and_then(|v| v.as_object()) {
                for (n, h) in m {
                    let (Some(k), Some(h)) = (Kind::of(n), h.as_str()) else { continue };
                    let c = format!("{h}.{}", k.ext());
                    out.by_name.insert(n.clone(), c.clone());
                    out.by_content.insert(c, (e.file.clone(), Some(n.clone())));
                }
            }
        }
        out
    }
}

// ---- The bytes ----------------------------------------------------------------------------------

fn be16(b: &[u8], i: usize) -> Option<u32> {
    Some(u32::from(u16::from_be_bytes(b.get(i..i + 2)?.try_into().ok()?)))
}

/// A PNG's width and height (its header's).
fn png_sides(b: &[u8]) -> Option<(u32, u32)> {
    (b.get(..8)? == b"\x89PNG\r\n\x1a\n" && b.get(12..16)? == b"IHDR").then_some(())?;
    Some((u32::from_be_bytes(b.get(16..20)?.try_into().ok()?), u32::from_be_bytes(b.get(20..24)?.try_into().ok()?)))
}

/// A JPEG's width and height (its first frame header's).
fn jpeg_sides(b: &[u8]) -> Option<(u32, u32)> {
    (b.get(..3)? == [0xff, 0xd8, 0xff]).then_some(())?;
    let mut i = 2;
    loop {
        (*b.get(i)? == 0xff).then_some(())?;
        let m = *b.get(i + 1)?;
        match m {
            0xff => i += 1,
            0x01 | 0xd0..=0xd8 => i += 2,
            0xc0..=0xcf if m != 0xc4 && m != 0xc8 && m != 0xcc => return Some((be16(b, i + 7)?, be16(b, i + 5)?)),
            0xda | 0xd9 => return None,
            _ => i += 2 + be16(b, i + 2)? as usize,
        }
    }
}

// ---- An SVG: what may be served -----------------------------------------------------------------

/// Elements refused, by local name (lower case): scripts, content from elsewhere, animation (which
/// can change a reference).
const REFUSED: &[&str] = &[
    "script",
    "foreignobject",
    "iframe",
    "object",
    "embed",
    "audio",
    "video",
    "handler",
    "listener",
    "animate",
    "animatemotion",
    "animatetransform",
    "animatecolor",
    "set",
    "discard",
];

/// The XHTML namespace: its elements in an SVG load, submit and navigate (`img`, `form`, `meta`).
const XHTML: &str = "http://www.w3.org/1999/xhtml";

/// An element as `svg` reads it: its children, and the element it copies (a `<use>`'s or
/// `<feImage>`'s `href`), for the count of what it draws (`drawn`).
#[derive(Default)]
struct Node {
    children: Vec<usize>,
    copies: Option<String>,
}

/// Why an SVG can't be served as is, if it can't. The map shows symbols the owner didn't draw, so
/// one is taken only with nothing in it that runs or reaches out, read as an XML parser reads it:
/// no control character but tab and line ends; declared in an encoding other than UTF-8 only when
/// it's ASCII (read alike in any of them); its root `<svg>`; no element of `REFUSED`; no event
/// attribute (`on…`), no `xml:base`, no XHTML namespace; a reference (`href`, `xlink:href`) only to
/// a part of itself (`#…`); CSS (a `<style>`'s text and CDATA as one, no element in it; `style`;
/// any `url(…)` in an attribute) with no `@import`, no `image-set(`, no backslash escape, and
/// `url(…)` only to a part of itself; no `<?xml-stylesheet?>`; comments as XML has them (no `--`
/// in one); a DOCTYPE's internal subset only comments and internal entities whose values hold no
/// `<`, `&` or `%`, an entity of its own nowhere but in text outside `<style>` and in a namespace
/// declaration, and all its uses together at most `MAX_BYTES` once expanded; at most `MAX_DRAWN`
/// elements drawn, each `<use>` counting what it copies (`drawn`). (The server's headers stand
/// behind it: crates/server/src/regions.rs.)
pub fn svg(text: &str) -> Result<(), String> {
    let t = text.strip_prefix('\u{feff}').unwrap_or(text);
    if let Some(c) = t.chars().find(|&c| c < ' ' && !matches!(c, '\t' | '\n' | '\r')) {
        return Err(format!("a control character (U+{:04X})", c as u32));
    }
    let mut i = 0;
    // (Open elements' local names, and their nodes.)
    let mut stack: Vec<String> = Vec::new();
    let mut open: Vec<usize> = Vec::new();
    let mut nodes: Vec<Node> = Vec::new();
    let mut ids: BTreeMap<String, usize> = BTreeMap::new();
    let mut ents: BTreeMap<String, String> = BTreeMap::new();
    let mut budget = MAX_BYTES as usize;
    // (The open `<style>`'s text so far: its text and CDATA as one, as the browser reads it.)
    let mut style: Option<String> = None;
    let mut root = false;
    while i < t.len() {
        let rest = &t[i..];
        let k = rest.find('<').unwrap_or(rest.len());
        let (text, own) = decode(&rest[..k], &ents, &mut budget)?;
        if let Some(css) = style.as_mut() {
            if own {
                return Err("an entity of the file's own in a <style>".into());
            }
            css.push_str(&text);
        }
        i += k;
        let rest = &t[i..];
        if rest.is_empty() {
            break;
        }
        if let Some(c) = rest.strip_prefix("<!--") {
            i += 4 + comment(c)?;
        } else if let Some(cdata) = rest.strip_prefix("<![CDATA[") {
            let e = cdata.find("]]>").ok_or("a CDATA section left open")?;
            if let Some(css) = style.as_mut() {
                css.push_str(&cdata[..e]);
            }
            i += 9 + e + 3;
        } else if rest.starts_with("<!") {
            if root || !rest[2..].get(..7).is_some_and(|d| d.eq_ignore_ascii_case("doctype")) {
                return Err("a markup declaration other than a DOCTYPE before the root".into());
            }
            i += doctype(rest, &mut ents)?;
        } else if let Some(pi) = rest.strip_prefix("<?") {
            let e = pi.find("?>").ok_or("a processing instruction left open")?;
            let target = pi[..e].split(|c: char| c.is_whitespace()).next().unwrap_or("");
            if target.eq_ignore_ascii_case("xml-stylesheet") {
                return Err("a stylesheet from elsewhere (<?xml-stylesheet?>)".into());
            }
            if target == "xml" {
                declared_encoding(&pi[3..e], t)?;
            }
            i += 2 + e + 2;
        } else if let Some(close) = rest.strip_prefix("</") {
            let e = close.find('>').ok_or("a closing tag left open")?;
            let name = local(close[..e].trim());
            match stack.pop() {
                Some(n) if n == name => {}
                _ => return Err(format!("</{}> closes nothing open", close[..e].trim())),
            }
            open.pop();
            if let Some(text) = style.take() {
                css(&text).map_err(|e| format!("<style>: {e}"))?;
            }
            i += 2 + e + 1;
        } else {
            let (len, name, attrs, empty) = tag(rest)?;
            let ln = local(&name);
            if style.is_some() {
                return Err(format!("a <{name}> in a <style>"));
            }
            if !root {
                if ln != "svg" {
                    return Err(format!("its root is <{name}>, not <svg>"));
                }
                root = true;
            } else if stack.is_empty() {
                return Err(format!("<{name}> after the root"));
            }
            if REFUSED.contains(&ln.as_str()) {
                return Err(format!(
                    "a <{name}> element (scripts, content from elsewhere and animation aren't served)"
                ));
            }
            let node = nodes.len();
            nodes.push(Node::default());
            if let Some(&parent) = open.last() {
                nodes[parent].children.push(node);
            }
            for (an, av) in &attrs {
                let v = attribute(an, av, &ents, &mut budget)?;
                match local(an).as_str() {
                    "id" => {
                        ids.entry(v.trim().to_string()).or_insert(node);
                    }
                    "href" if ln == "use" || ln == "feimage" => {
                        nodes[node].copies = v.trim().strip_prefix('#').map(str::to_string);
                    }
                    _ => {}
                }
            }
            if !empty {
                if ln == "style" {
                    style = Some(String::new());
                }
                stack.push(ln);
                open.push(node);
            }
            i += len;
        }
    }
    if let Some(open) = stack.last() {
        return Err(format!("<{open}> left open"));
    }
    if !root {
        return Err("no <svg> element".into());
    }
    let n = drawn(&nodes, &ids)?;
    if n > MAX_DRAWN {
        return Err(format!("more than {MAX_DRAWN} elements drawn, its <use> copies counted"));
    }
    Ok(())
}

/// A comment's text after its `<!--`: the length up to its end (`-->`), as XML has one (no `--`
/// in it, nor a `-` just before its end: a parser would end it, or refuse it, where this didn't).
fn comment(c: &str) -> Result<usize, String> {
    let e = c.find("-->").ok_or("a comment left open")?;
    if c[..e].contains("--") || c[..e].ends_with('-') {
        return Err("a comment holding \"--\" (not XML)".into());
    }
    Ok(e + 3)
}

/// The XML declaration's (`<?xml …?>`'s) encoding, if it names one: UTF-8, as the check reads it,
/// or another only for a file in ASCII (which every encoding a browser reads an SVG in reads alike).
fn declared_encoding(decl: &str, text: &str) -> Result<(), String> {
    let Some(at) = decl.find("encoding") else { return Ok(()) };
    let v = decl[at + 8..].trim_start().strip_prefix('=').map(str::trim_start).unwrap_or("");
    let Some(q) = v.chars().next().filter(|c| *c == '"' || *c == '\'') else {
        return Err("an XML declaration's encoding not in quotes".into());
    };
    let name = v[1..].split(q).next().unwrap_or("");
    if name.eq_ignore_ascii_case("utf-8") || text.is_ascii() {
        Ok(())
    } else {
        Err(format!("declared in {name}, not UTF-8, and not ASCII (a browser would read it otherwise)"))
    }
}

/// The elements an SVG draws, each `<use>` or `<feImage>` counting the element it copies with all
/// in it (a copy of copies of copies multiplies them: a few kilobytes can draw millions), at most
/// `MAX_DRAWN + 1`. Worked out without recursion (a file may nest thousands deep). A copy of
/// something holding it is refused (browsers draw none of it; nothing needs one).
fn drawn(nodes: &[Node], ids: &BTreeMap<String, usize>) -> Result<u64, String> {
    if nodes.is_empty() {
        return Ok(0);
    }
    let deps =
        |n: usize| nodes[n].children.iter().copied().chain(nodes[n].copies.as_ref().and_then(|c| ids.get(c)).copied());
    let mut count: Vec<Option<u64>> = vec![None; nodes.len()];
    let mut on_path = vec![false; nodes.len()];
    let mut todo = vec![(0usize, false)];
    while let Some((n, done)) = todo.pop() {
        if count[n].is_some() {
            continue;
        }
        if done {
            let c = deps(n).fold(1u64, |a, d| a.saturating_add(count[d].unwrap_or(0)));
            count[n] = Some(c.min(MAX_DRAWN + 1));
            on_path[n] = false;
            continue;
        }
        on_path[n] = true;
        todo.push((n, true));
        for d in deps(n) {
            if on_path[d] {
                return Err("a <use> copying an element that holds it".into());
            }
            if count[d].is_none() {
                todo.push((d, false));
            }
        }
    }
    Ok(count[0].unwrap_or(0))
}

/// A name's local part, lower case (`xlink:href` → `href`).
fn local(name: &str) -> String {
    name.rsplit(':').next().unwrap_or(name).to_ascii_lowercase()
}

/// A start tag at the start of `s`: its length, name, attributes (raw values) and whether it's
/// empty (`/>`).
type Tag = (usize, String, Vec<(String, String)>, bool);
fn tag(s: &str) -> Result<Tag, String> {
    let b = s.as_bytes();
    let name_end =
        s[1..].find(|c: char| c.is_whitespace() || c == '/' || c == '>').map(|e| e + 1).ok_or("a tag left open")?;
    let name = s[1..name_end].to_string();
    if name.is_empty() {
        return Err("a '<' that starts no tag".into());
    }
    let mut i = name_end;
    let mut attrs = Vec::new();
    loop {
        while b.get(i).is_some_and(u8::is_ascii_whitespace) {
            i += 1;
        }
        match b.get(i) {
            None => return Err(format!("<{name}> left open")),
            Some(b'>') => return Ok((i + 1, name, attrs, false)),
            Some(b'/') if b.get(i + 1) == Some(&b'>') => return Ok((i + 2, name, attrs, true)),
            _ => {}
        }
        let an_end = s[i..]
            .find(|c: char| c.is_whitespace() || c == '=' || c == '>' || c == '/')
            .map(|e| e + i)
            .ok_or("an attribute left open")?;
        let an = s[i..an_end].to_string();
        i = an_end;
        while b.get(i).is_some_and(u8::is_ascii_whitespace) {
            i += 1;
        }
        if b.get(i) != Some(&b'=') || an.is_empty() {
            return Err(format!("<{name}>: an attribute without a value"));
        }
        i += 1;
        while b.get(i).is_some_and(u8::is_ascii_whitespace) {
            i += 1;
        }
        let q = *b.get(i).ok_or("an attribute left open")?;
        if q != b'"' && q != b'\'' {
            return Err(format!("<{name}> {an}: a value not in quotes"));
        }
        let ve = s[i + 1..].find(q as char).map(|e| e + i + 1).ok_or("an attribute's value left open")?;
        let v = &s[i + 1..ve];
        if v.contains('<') {
            return Err(format!("<{name}> {an}: a '<' in its value"));
        }
        attrs.push((an, v.to_string()));
        i = ve + 1;
    }
}

/// Text with its references replaced: character references, XML's five entities and the file's own
/// (`ents`, each use taken from `budget`, the bytes its entities may expand to), and whether it uses
/// one of its own. A reference to no entity it has is refused (a parser refuses it).
fn decode(s: &str, ents: &BTreeMap<String, String>, budget: &mut usize) -> Result<(String, bool), String> {
    let (mut out, mut own) = (String::with_capacity(s.len()), false);
    let mut rest = s;
    while let Some(k) = rest.find('&') {
        out.push_str(&rest[..k]);
        rest = &rest[k..];
        let Some(e) = rest.find(';') else {
            out.push_str(rest);
            return Ok((out, own));
        };
        let r = &rest[1..e];
        let c = match r {
            "lt" => Some('<'),
            "gt" => Some('>'),
            "amp" => Some('&'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            _ if r.starts_with('#') => Some(
                r.strip_prefix("#x")
                    .map(|h| u32::from_str_radix(h, 16))
                    .or_else(|| r.strip_prefix('#').map(str::parse::<u32>))
                    .and_then(Result::ok)
                    .and_then(char::from_u32)
                    .ok_or_else(|| format!("&{r}; is no character"))?,
            ),
            _ => None,
        };
        match c {
            Some(c) => out.push(c),
            None => {
                let v = ents.get(r).ok_or_else(|| format!("&{r}; is no entity it declares"))?;
                *budget = budget
                    .checked_sub(v.len())
                    .ok_or_else(|| format!("its entities expanded to more than {MAX_BYTES} bytes"))?;
                own = true;
                out.push_str(v);
            }
        }
        rest = &rest[e + 1..];
    }
    out.push_str(rest);
    Ok((out, own))
}

/// An attribute's value, decoded: no event handler, no base moving its references, no XHTML, a
/// reference only within the image, CSS checked.
fn attribute(name: &str, raw: &str, ents: &BTreeMap<String, String>, budget: &mut usize) -> Result<String, String> {
    let ln = local(name);
    let (v, own) = decode(raw, ents, budget).map_err(|e| format!("{name}: {e}"))?;
    if ln.starts_with("on") {
        return Err(format!("an event attribute ({name})"));
    }
    if name == "xml:base" {
        return Err("an xml:base (it sends its references elsewhere)".into());
    }
    if name == "xmlns" || name.starts_with("xmlns:") {
        if v.trim() == XHTML {
            return Err(format!("{name}: the XHTML namespace (its elements load and navigate)"));
        }
        return Ok(v);
    }
    if own {
        return Err(format!("{name}: an entity of the file's own in an attribute"));
    }
    if ln == "href" {
        if !v.trim_start().starts_with('#') {
            return Err(format!(
                "{name}=\"{}\": a reference outside the image",
                v.chars().take(60).collect::<String>()
            ));
        }
        return Ok(v);
    }
    if ln == "style" || v.to_ascii_lowercase().contains("url(") {
        css(&v).map_err(|e| format!("{name}: {e}"))?;
    }
    Ok(v)
}

/// CSS that reaches nothing outside the image.
fn css(s: &str) -> Result<(), String> {
    if s.contains('\\') {
        return Err("a backslash in CSS (an escape)".into());
    }
    let l = s.to_ascii_lowercase();
    if l.contains("@import") {
        return Err("an @import in CSS".into());
    }
    if l.contains("image-set(") {
        return Err("an image-set() in CSS".into());
    }
    let mut rest = l.as_str();
    while let Some(k) = rest.find("url(") {
        rest = &rest[k + 4..];
        let inner = rest.trim_start().trim_start_matches(['"', '\'']).trim_start();
        if !inner.starts_with('#') {
            let shown: String = inner.chars().take_while(|&c| c != ')').take(60).collect();
            return Err(format!("url({shown}): a reference outside the image"));
        }
    }
    Ok(())
}

/// A DOCTYPE at the start of `s`: its length, its internal subset only comments and internal
/// entities whose values hold no `<`, `&` or `%` (an entity holding markup could make an element
/// the check never saw, one holding a reference could nest, a billion expansions from a few
/// bytes; an external one is read from elsewhere; a declaration of attributes could add an event
/// handler to every element), each put in `ents` (the first of a name, as XML takes it).
fn doctype(s: &str, ents: &mut BTreeMap<String, String>) -> Result<usize, String> {
    let b = s.as_bytes();
    let mut i = 2;
    let mut quote = None;
    // The head: up to its internal subset or its end.
    loop {
        let c = *b.get(i).ok_or("a DOCTYPE left open")?;
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => {}
            None if c == b'"' || c == b'\'' => quote = Some(c),
            None if c == b'>' => return Ok(i + 1),
            None if c == b'[' => break,
            None => {}
        }
        i += 1;
    }
    i += 1;
    loop {
        while b.get(i).is_some_and(u8::is_ascii_whitespace) {
            i += 1;
        }
        let rest = s.get(i..).ok_or("a DOCTYPE left open")?;
        if let Some(r) = rest.strip_prefix(']') {
            let e = r.find('>').ok_or("a DOCTYPE left open")?;
            if !r[..e].trim().is_empty() {
                return Err("a DOCTYPE with more after its internal subset".into());
            }
            return Ok(i + 1 + e + 1);
        } else if let Some(c) = rest.strip_prefix("<!--") {
            i += 4 + comment(c)?;
        } else if let Some(ent) = rest.strip_prefix("<!ENTITY") {
            let ent = ent.trim_start();
            if ent.starts_with('%') {
                return Err("a parameter entity in the DOCTYPE".into());
            }
            let name_end = ent.find(char::is_whitespace).ok_or("an entity left open")?;
            let after = ent[name_end..].trim_start();
            let q = after
                .chars()
                .next()
                .filter(|c| *c == '"' || *c == '\'')
                .ok_or("an entity read from elsewhere (SYSTEM or PUBLIC)")?;
            let ve = after[1..].find(q).ok_or("an entity's value left open")?;
            let value = &after[1..1 + ve];
            if value.contains(['<', '&', '%']) {
                return Err("an entity whose value holds markup or references".into());
            }
            let tail = &after[1 + ve + 1..];
            let close = tail.find('>').ok_or("an entity left open")?;
            if !tail[..close].trim().is_empty() {
                return Err("an entity read from elsewhere (NDATA)".into());
            }
            ents.entry(ent[..name_end].to_string()).or_insert_with(|| value.to_string());
            // (Its length: up to the end of `tail`'s `>`.)
            i = s.len() - tail.len() + close + 1;
        } else {
            return Err("a declaration in the DOCTYPE other than an internal entity".into());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_free_list_is_matched_word_by_word() {
        for l in
            ["Public domain", "CC0", "CC BY 4.0", "CC BY-SA 3.0", "CC BY 2.5 ar", "Copyrighted free use", "OGL-om 1.0"]
        {
            assert!(free(l), "{l}");
        }
        for l in
            ["CC BY-NC 4.0", "CC0 1.0", "CC BY 4", "CC BY 2.5 AR", "public domain", "GFDL", "", "CC BY-SA 4.0 ar x"]
        {
            assert!(!free(l), "{l}");
        }
    }

    #[test]
    fn a_table_is_checked_field_by_field() {
        let ok = Symbol {
            image: "Q1.svg".into(),
            kind: "flag".into(),
            source: "https://example.org/File:Q1.svg".into(),
            author: String::new(),
            licence: "CC0".into(),
        };
        assert!(faults(&ok).is_empty());
        let bad = Symbol {
            image: "a/b.gif".into(),
            kind: "banner".into(),
            source: "http://example.org/".into(),
            author: "x".into(),
            licence: "All rights reserved".into(),
        };
        assert_eq!(faults(&bad).len(), 4);
    }

    #[test]
    fn rasters_are_told_by_their_bytes() {
        let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".to_vec();
        png.extend(60u32.to_be_bytes());
        png.extend(40u32.to_be_bytes());
        assert_eq!(png_sides(&png), Some((60, 40)));
        assert!(image("a.png", &png).is_ok());
        // A JPEG named .png (as Commons' renders of a JPEG are): refused.
        let jpeg = [0xff, 0xd8, 0xff, 0xe0, 0, 4, 0, 0, 0xff, 0xc0, 0, 11, 8, 0, 89, 0, 120, 3, 0, 0, 0];
        assert_eq!(jpeg_sides(&jpeg), Some((120, 89)));
        assert!(image("a.jpg", &jpeg).is_ok());
        assert!(image("a.png", &jpeg).unwrap_err().contains("not a PNG"));
        let mut big = png.clone();
        big[16..20].copy_from_slice(&600u32.to_be_bytes());
        assert!(image("a.png", &big).unwrap_err().contains("at most 512"));
        assert!(image("a.svg", &vec![b' '; MAX_BYTES as usize + 1]).unwrap_err().contains("more than"));
    }

    #[test]
    fn an_svg_is_served_only_with_nothing_that_runs_or_reaches_out() {
        let ok = [
            r##"<?xml version="1.0"?><!-- x --><svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 2 1"><rect width="2" height="1" fill="#c00"/></svg>"##,
            "\u{feff}<svg xmlns='http://www.w3.org/2000/svg'><defs><linearGradient id='g'/></defs><use xlink:href='#g' fill='url(#g)'/></svg>",
            r##"<!DOCTYPE svg PUBLIC "-//W3C//DTD SVG 1.1//EN" "http://www.w3.org/Graphics/SVG/1.1/DTD/svg11.dtd" [
	<!ENTITY ns_flows "http://ns.adobe.com/Flows/1.0/">
]><svg xmlns:x="&ns_flows;"><style type="text/css"><![CDATA[ .a{fill:url(#g)} ]]></style><style>.b{fill:#fff}</style><text>&quot;A&amp;B&quot; &ns_flows;</text></svg>"##,
            r#"<svg><g inkscape:export-filename="C:\drawing.png"/></svg>"#,
            // (A comment whose text starts with '>', as XML reads it; ASCII declared Latin-1.)
            "<?xml version='1.0' encoding='ISO-8859-1'?><svg><!--><script>alert(1)</script>--></svg>",
            // (Copies of copies, within the bound: 10 × 10 × 10.)
            r##"<svg><defs><g id="a"><rect/><rect/><rect/><rect/><rect/><rect/><rect/><rect/><rect/><rect/></g>
<g id="b"><use href="#a"/><use href="#a"/><use href="#a"/><use href="#a"/><use href="#a"/><use href="#a"/><use href="#a"/><use href="#a"/><use href="#a"/><use href="#a"/></g></defs>
<use href="#b"/><use href="#b"/><use href="#b"/><use href="#b"/><use href="#b"/><use href="#b"/><use href="#b"/><use href="#b"/><use href="#b"/><use href="#b"/></svg>"##,
        ];
        for s in ok {
            assert_eq!(svg(s), Ok(()), "{s}");
        }
        let refused = [
            ("<svg><script>alert(1)</script></svg>", "<script>"),
            ("<svg><svg:script/></svg>", "script"),
            ("<svg><foreignObject><p/></foreignObject></svg>", "foreignObject"),
            ("<svg onload=\"alert(1)\"/>", "event"),
            ("<svg><a href=\"https://example.org/\"/></svg>", "outside"),
            ("<svg><image xlink:href=\"data:image/png;base64,AA\"/></svg>", "outside"),
            ("<svg><use href=\"&#x68;ttp://x/#a\"/></svg>", "outside"),
            ("<svg><rect fill=\"url(https://example.org/a.svg#g)\"/></svg>", "outside"),
            ("<svg><rect style=\"fill:u\\72l(http://x)\"/></svg>", "backslash"),
            ("<svg><style>@import url(#a);</style></svg>", "@import"),
            ("<svg><style>.a{background:url(http://x/)}</style></svg>", "outside"),
            ("<svg><style>.a{background:u&#114;l(http://x/)}</style></svg>", "outside"),
            ("<svg><style>.a{background:image-set(\"x.png\" 1x)}</style></svg>", "image-set"),
            ("<?xml-stylesheet href=\"http://x/a.css\"?><svg/>", "stylesheet"),
            ("<!DOCTYPE svg [<!ENTITY x SYSTEM \"http://x/\">]><svg>&x;</svg>", "elsewhere"),
            ("<!DOCTYPE svg [<!ENTITY x \"&#60;script&#62;\">]><svg>&x;</svg>", "markup"),
            ("<!DOCTYPE svg [<!ATTLIST svg onload CDATA \"alert(1)\">]><svg/>", "other than"),
            ("<!DOCTYPE svg [<!ENTITY % p \"x\">]><svg/>", "parameter"),
            ("<!DOCTYPE svg [<!ENTITY x \"url(http://x/)\">]><svg><style>&x;</style></svg>", "entity"),
            ("<!DOCTYPE svg [<!ENTITY x \"http://x/\">]><svg><a href=\"&x;\"/></svg>", "entity"),
            ("<svg><set attributeName=\"href\" to=\"http://x/\"/></svg>", "<set>"),
            // (Links to scripts however written: a scheme in any case, after a tab, as references.)
            ("<svg><a xlink:href=\"javascript:alert(1)\"/></svg>", "outside"),
            ("<svg><a href=\" JavaScript:alert(1)\"/></svg>", "outside"),
            ("<svg><a href=\"&#x09;javascript:alert(1)\"/></svg>", "outside"),
            ("<svg><a href=\"&#106;avascript:alert(1)\"/></svg>", "outside"),
            ("<svg><a href=\"&#X6a;avascript:alert(1)\"/></svg>", "no character"),
            ("<svg><g ONCLICK=\"alert(1)\"/></svg>", "event"),
            // (Content from elsewhere, and data: URIs, an image in the image the check can't read:
            // an SVG in one would escape it.)
            ("<svg><use href=\"https://x/s.svg#a\"/></svg>", "outside"),
            ("<svg><image href=\"http://x/a.png\"/></svg>", "outside"),
            ("<svg><image href=\"data:image/svg+xml;base64,PHN2Zy8+\"/></svg>", "outside"),
            ("<svg><rect style=\"background:url( 'http://x/')\"/></svg>", "outside"),
            ("<svg><style>@IMPORT \"x.css\";</style></svg>", "@import"),
            ("<svg><animate attributeName=\"href\" values=\"http://x/\"/></svg>", "<animate>"),
            ("<svg xml:base=\"http://x/\"><use href=\"#a\"/></svg>", "xml:base"),
            // (A style's text read as one, as the browser reads it, across CDATA, comments and
            // processing instructions; no element in it.)
            ("<svg><style>.a{fill:ur<![CDATA[l(http://x/)]]>}</style></svg>", "outside"),
            ("<svg><style>.a{fill:ur<!---->l(http://x/)}</style></svg>", "outside"),
            ("<svg><style>@imp<?x?>ort url(#a);</style></svg>", "@import"),
            ("<svg><style><g>.a{fill:url(http://x/)}</g></style></svg>", "in a <style>"),
            // (XHTML's elements, however its namespace is named.)
            ("<svg xmlns:h=\"http://www.w3.org/1999/xhtml\"><h:img src=\"http://x/\"/></svg>", "XHTML"),
            ("<svg><g xmlns=\"http://www.w3.org/1999/xhtml\"><meta http-equiv=\"refresh\" content=\"0;url=http://x/\"/></g></svg>", "XHTML"),
            ("<!DOCTYPE svg [<!ENTITY h \"http://www.w3.org/1999/xhtml\">]><svg xmlns:h=\"&h;\"/>", "XHTML"),
            // (Comments, entities and encodings as a parser has them.)
            ("<svg><!-- a -- b --></svg>", "--"),
            ("<svg><!-- a ---></svg>", "--"),
            ("<svg><text>&nbsp;</text></svg>", "no entity"),
            ("<!DOCTYPE svg [<!ENTITY a \"x\"><!ENTITY b \"&a;&a;\">]><svg/>", "markup or references"),
            ("<?xml version=\"1.0\" encoding=\"Shift_JIS\"?><svg><text>ā]]></text></svg>", "not UTF-8"),
            ("<svg><text>\u{1b}$B</text></svg>", "control character"),
            ("<\0s\0v\0g\0/\0>\0", "control character"),
            ("<svg><g id=\"a\"><use href=\"#a\"/></g></svg>", "holds it"),
            ("<html><svg/></html>", "root"),
            ("<svg><g></svg>", "closes"),
            ("<svg>", "left open"),
            ("hello", "no <svg>"),
        ];
        for (s, why) in refused {
            let e = svg(s).expect_err(s);
            assert!(e.contains(why), "{s}: {e}");
        }
    }

    #[test]
    fn an_svg_expanding_past_its_bounds_is_refused() {
        // A copy of copies of copies (the billion laughs of <use>): 10⁶ drawn from a kilobyte.
        let mut bomb = String::from("<svg><defs><g id=\"a0\"><rect/></g>");
        for l in 1..=6 {
            bomb.push_str(&format!("<g id=\"a{l}\">{}</g>", format!("<use href=\"#a{}\"/>", l - 1).repeat(10)));
        }
        bomb.push_str("</defs><use href=\"#a6\"/></svg>");
        assert!(bomb.len() < 2000);
        assert!(svg(&bomb).unwrap_err().contains("elements drawn"));
        // Nested thousands deep: counted, not recursed into.
        let deep = format!("<svg>{}{}</svg>", "<g>".repeat(20_000), "</g>".repeat(20_000));
        assert_eq!(svg(&deep), Ok(()));
        // An entity used more than its bytes allow (entities can't nest: linear, bounded).
        let big = format!(
            "<!DOCTYPE svg [<!ENTITY a \"{}\">]><svg><text>{}</text></svg>",
            "x".repeat(1000),
            "&a;".repeat(70)
        );
        assert!(svg(&big).unwrap_err().contains("expanded to more than"));
        // A bundle whose members together pass the bound, refused by its directory's sizes.
        let zip = crate::inputs::gtfs::tests::zip_bytes(&[("Q1.svg", &[b' '; 100][..]), ("Q2.svg", &[b' '; 100][..])]);
        let e = crate::inputs::gtfs::members(&zip, 100, 150).unwrap_err();
        assert!(format!("{e:#}").contains("together"), "{e:#}");
        // UTF-16, with its byte-order mark: not UTF-8.
        assert!(image("a.svg", b"\xff\xfe<\0s\0v\0g\0/\0>\0").unwrap_err().contains("UTF-8"));
    }

    #[test]
    fn a_bundle_gives_its_images_by_name() {
        let png = {
            let mut p = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".to_vec();
            p.extend(2u32.to_be_bytes());
            p.extend(1u32.to_be_bytes());
            p
        };
        let zip = crate::inputs::gtfs::tests::zip_bytes(&[
            ("flags/Q1.svg", &b"<svg/>"[..]),
            ("Q2.png", png.as_slice()),
            ("__MACOSX/flags/._Q1.svg", &b"junk"[..]),
            // (A path out of the folder is a name alone: nothing is written; a zip in it isn't read.)
            ("../../Q3.svg", &b"<svg/>"[..]),
            ("inner.zip", &b"PK"[..]),
            ("README.txt", &b"notes"[..]),
        ]);
        let b = bundle(&zip).unwrap();
        assert_eq!(b.keys().collect::<Vec<_>>(), ["Q1.svg", "Q2.png", "Q3.svg"]);
        let twice =
            crate::inputs::gtfs::tests::zip_bytes(&[("a/Q1.svg", &b"<svg/>"[..]), ("b/Q1.svg", &b"<svg/>"[..])]);
        assert!(bundle(&twice).unwrap_err()[0].contains("twice"));
        let bad =
            crate::inputs::gtfs::tests::zip_bytes(&[("Q1.svg", &b"<svg onclick='x'/>"[..]), ("Q2.png", &b"no"[..])]);
        assert_eq!(bundle(&bad).unwrap_err().len(), 2);
    }
}
