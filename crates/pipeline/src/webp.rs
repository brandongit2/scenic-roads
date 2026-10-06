//! Lossless WebP (VP8L) encoding, in Rust: the same bytes on every target (its choices made by
//! integer counts and det's log2). Made for the tree cover's Terrarium tiles, one channel that varies:
//! - **Palette:** up to 16 colours, their indices packed 2, 4 or 8 to a byte; more, the pixels as
//!   they are (spatial prediction, the colour cache and LZ77 at other distances gained nothing on
//!   the tiles, or cost more than they saved).
//! - **LZ77:** copies from the pixel before (runs) and from the row above; one is left for a longer
//!   one starting a pixel later, and taken when its symbols cost less than the pixels' would
//!   (costed by a first pass's codes).
//! - **Huffman codes:** a group of five per region of 32 × 32 pixels (an entropy image), regions
//!   merged into groups while that saves bits; code lengths at most 15.
//!
//! It writes what decoders check (libwebp's, image-webp's): complete codes, a two-symbol simple code
//! in ascending order.

use det::Det;

/// An opaque RGB image (`w` × `h`, 3 bytes a pixel, row by row) as a lossless WebP file.
pub fn encode_rgb(rgb: &[u8], w: u32, h: u32) -> Vec<u8> {
    assert!((1..=16384).contains(&w) && (1..=16384).contains(&h) && rgb.len() == w as usize * h as usize * 3);
    let px: Vec<u32> = rgb.as_chunks::<3>().0.iter().map(|p| 0xff00_0000 | (p[0] as u32) << 16 | (p[1] as u32) << 8 | p[2] as u32).collect();
    let mut b = Bits::default();
    b.put(0x2f, 8);
    b.put(w - 1, 14);
    b.put(h - 1, 14);
    b.put(0, 1); // no alpha
    b.put(0, 3); // version 0
    let (w, h) = (w as usize, h as usize);
    let img = match palette(&px) {
        Some(pal) => indexed(&mut b, &px, w, h, &pal),
        None => Image { px, w, h },
    };
    b.put(0, 1); // no more transforms
    entropy_coded(&mut b, &img, true);
    let data = b.finish();
    let chunk = data.len() as u32;
    let mut out = Vec::with_capacity(data.len() + 21);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(12 + chunk + (chunk & 1)).to_le_bytes());
    out.extend_from_slice(b"WEBPVP8L");
    out.extend_from_slice(&chunk.to_le_bytes());
    out.extend_from_slice(&data);
    if chunk & 1 == 1 {
        out.push(0);
    }
    out
}

/// Bits, least significant first.
#[derive(Default)]
struct Bits {
    out: Vec<u8>,
    acc: u64,
    n: u32,
}

impl Bits {
    fn put(&mut self, v: u32, n: u32) {
        debug_assert!(n <= 32 && (n == 32 || v >> n == 0));
        self.acc |= (v as u64) << self.n;
        self.n += n;
        while self.n >= 8 {
            self.out.push(self.acc as u8);
            self.acc >>= 8;
            self.n -= 8;
        }
    }

    fn finish(mut self) -> Vec<u8> {
        if self.n > 0 {
            self.out.push(self.acc as u8);
        }
        self.out
    }
}

/// An image to entropy-code: ARGB pixels, `w` × `h`.
struct Image {
    px: Vec<u32>,
    w: usize,
    h: usize,
}

/// The image's colours, sorted, when there are at most 16.
fn palette(px: &[u32]) -> Option<Vec<u32>> {
    let mut seen: Vec<u32> = Vec::new();
    let mut last = None;
    for &p in px {
        if last == Some(p) {
            continue;
        }
        last = Some(p);
        if let Err(i) = seen.binary_search(&p) {
            if seen.len() == 16 {
                return None;
            }
            seen.insert(i, p);
        }
    }
    Some(seen)
}

/// Writes the colour-indexing transform for palette `pal`: the palette, each colour less the one
/// before; and returns the image as indices packed into bytes (8, 4 or 2 a byte, the first pixel
/// in the lowest bits).
fn indexed(b: &mut Bits, px: &[u32], w: usize, h: usize, pal: &[u32]) -> Image {
    b.put(1, 1);
    b.put(3, 2);
    b.put(pal.len() as u32 - 1, 8);
    let deltas: Vec<u32> = pal.iter().enumerate().map(|(i, &c)| if i == 0 { c } else { sub_pixels(c, pal[i - 1]) }).collect();
    entropy_coded(b, &Image { px: deltas, w: pal.len(), h: 1 }, false);
    let xbits = match pal.len() {
        0..=2 => 3,
        3..=4 => 2,
        _ => 1,
    };
    let (per, bits) = (1usize << xbits, 8 >> xbits);
    let pw = w.div_ceil(per);
    let mut out = vec![0xff00_0000u32; pw * h];
    for y in 0..h {
        for x in 0..w {
            let i = pal.binary_search(&px[y * w + x]).unwrap() as u32;
            out[y * pw + x / per] |= i << (8 + bits * (x % per));
        }
    }
    Image { px: out, w: pw, h }
}

/// Channel by channel, a - b mod 256.
fn sub_pixels(a: u32, b: u32) -> u32 {
    let mut out = 0;
    for s in [0, 8, 16, 24] {
        out |= ((a >> s).wrapping_sub(b >> s) & 0xff) << s;
    }
    out
}

/// n · log2(n), from a table up to 2^16.
fn nlog2(n: u64) -> f64 {
    static TABLE: std::sync::OnceLock<Vec<f64>> = std::sync::OnceLock::new();
    let t = TABLE.get_or_init(|| (0..=1u64 << 16).map(|v| if v < 2 { 0.0 } else { v as f64 * (v as f64).dlog2() }).collect());
    match t.get(n as usize) {
        Some(&v) => v,
        None => n as f64 * (n as f64).dlog2(),
    }
}

// ---- the pixels' symbols ------------------------------------------------------------------------

/// A group of codes' alphabets, one after another: green with the copies' lengths, red, blue,
/// alpha, distance.
const ALPHABETS: [usize; 5] = [256 + 24, 256, 256, 256, 40];
const AT: [usize; 6] = [0, 280, 536, 792, 1048, 1088];

/// A symbol of the pixel stream: a literal pixel, or a copy of `len` pixels from `dist` back.
#[derive(Clone, Copy)]
enum Sym {
    Lit(u32),
    Copy { len: u32, dist: u32 },
}

/// A length or distance code (≥ 1) as VP8L codes it: its prefix symbol, and its extra bits' count
/// and value.
fn prefix(v: u32) -> (u32, u32, u32) {
    let d = v - 1;
    if d < 4 {
        return (d, 0, 0);
    }
    let h = 31 - d.leading_zeros();
    let extra = h - 1;
    (2 * h + ((d >> extra) & 1), extra, d & ((1 << extra) - 1))
}

/// The distance code of a copy `dist` pixels back in an image `w` wide: the pixel above is plane
/// code 1, the one before plane code 2, anything else `dist` + 120.
fn dist_code(dist: u32, w: usize) -> u32 {
    if dist as usize == w {
        1
    } else if dist == 1 {
        2
    } else {
        dist + 120
    }
}

/// The symbols of `s`'s parts (indices into the alphabets one after another), each given to `f`;
/// its extra bits.
fn parts(s: Sym, w: usize, mut f: impl FnMut(usize)) -> u32 {
    match s {
        Sym::Lit(p) => {
            f(AT[0] + ((p >> 8) & 0xff) as usize);
            f(AT[1] + ((p >> 16) & 0xff) as usize);
            f(AT[2] + (p & 0xff) as usize);
            f(AT[3] + (p >> 24) as usize);
            0
        }
        Sym::Copy { len, dist } => {
            let (ls, le, _) = prefix(len);
            let (ds, de, _) = prefix(dist_code(dist, w));
            f(AT[0] + 256 + ls as usize);
            f(AT[4] + ds as usize);
            le + de
        }
    }
}

/// LZ77 over the pixels: copies from the pixel before or the one above, at least 2 long, one left
/// for a longer one starting a pixel later; with `cost`, a copy taken only when it costs less than
/// its pixels as literals.
fn lz77(px: &[u32], w: usize, cost: Option<&Costs>) -> Vec<Sym> {
    const MAX_LEN: usize = 4096;
    let n = px.len();
    // How far each pixel matches the pixels `d` back (counted from the end).
    let run = |d: usize| -> Vec<u16> {
        let mut r = vec![0u16; n + 1];
        for i in (d..n).rev() {
            if px[i] == px[i - d] {
                r[i] = (r[i + 1] + 1).min(MAX_LEN as u16);
            }
        }
        r
    };
    let (r1, rw) = (run(1), run(w));
    let longest = |i: usize| -> (usize, usize) {
        let (a, b) = (r1[i] as usize, rw[i] as usize);
        if a >= b {
            (a, 1)
        } else {
            (b, w)
        }
    };
    // (The bits of the pixels before each as literals.)
    let lit: Vec<u32> = match cost {
        Some(c) => std::iter::once(0).chain(px.iter().scan(0u32, |a, &p| {
            *a += c.lit(p);
            Some(*a)
        })).collect(),
        None => Vec::new(),
    };
    let worth = |i: usize, l: usize, d: usize| match cost {
        None => l >= 2,
        Some(c) => l >= 2 && c.copy(l as u32, dist_code(d as u32, w)) < lit[i + l] - lit[i],
    };
    let mut out = Vec::with_capacity(n / 2);
    let mut i = 0;
    while i < n {
        let (l, d) = longest(i);
        if worth(i, l, d) && !(i + 1 < n && longest(i + 1).0 > l + 1) {
            out.push(Sym::Copy { len: l as u32, dist: d as u32 });
            i += l;
        } else {
            out.push(Sym::Lit(px[i]));
            i += 1;
        }
    }
    out
}

/// What symbols cost in bits, by a stream's code lengths (a symbol it never used: 16).
struct Costs {
    lens: Vec<u8>,
}

impl Costs {
    fn of(h: &Histo) -> Costs {
        let mut lens = vec![16u8; AT[5]];
        for k in 0..5 {
            let l = lengths(&h.dense(k), 15);
            let used = l.iter().filter(|&&x| x > 0).count();
            for (s, &x) in l.iter().enumerate() {
                if x > 0 {
                    lens[AT[k] + s] = if used == 1 { 0 } else { x };
                }
            }
        }
        Costs { lens }
    }

    fn lit(&self, p: u32) -> u32 {
        let mut c = 0;
        parts(Sym::Lit(p), 1, |k| c += self.lens[k] as u32);
        c
    }

    fn copy(&self, len: u32, code: u32) -> u32 {
        let (ls, le, _) = prefix(len);
        let (ds, de, _) = prefix(code);
        self.lens[AT[0] + 256 + ls as usize] as u32 + le + self.lens[AT[4] + ds as usize] as u32 + de
    }
}

/// A group's symbol counts, sparse: (index into the alphabets one after another, count), sorted;
/// per alphabet the total, the symbols used and the sum of n · log2(n); the copies' extra bits.
#[derive(Clone, Default)]
struct Histo {
    c: Vec<(u16, u32)>,
    total: [u64; 5],
    used: [u32; 5],
    sum: [f64; 5],
    extra: u64,
}

/// The alphabet of an index into them, one after another.
fn alphabet(s: u16) -> usize {
    AT[1..].partition_point(|&a| a <= s as usize)
}

impl Histo {
    fn of(syms: impl Iterator<Item = Sym>, w: usize) -> Histo {
        let mut dense = vec![0u32; AT[5]];
        let mut extra = 0u64;
        for s in syms {
            extra += parts(s, w, |k| dense[k] += 1) as u64;
        }
        let mut h = Histo { extra, ..Default::default() };
        for (k, &v) in dense.iter().enumerate().filter(|e| *e.1 > 0) {
            h.c.push((k as u16, v));
            let a = alphabet(k as u16);
            h.total[a] += v as u64;
            h.used[a] += 1;
            h.sum[a] += nlog2(v as u64);
        }
        h
    }

    /// Alphabet `k`'s counts.
    fn dense(&self, k: usize) -> Vec<u32> {
        let mut d = vec![0u32; ALPHABETS[k]];
        for &(s, v) in &self.c {
            if (AT[k]..AT[k + 1]).contains(&(s as usize)) {
                d[s as usize - AT[k]] = v;
            }
        }
        d
    }

    /// An estimate of the bits its codes and symbols take: the entropy, the extra bits, and a guess
    /// at each code's own lengths.
    fn cost(&self) -> f64 {
        cost_of(&self.total, &self.used, &self.sum, self.extra)
    }

    /// `cost` of this and `o` merged: theirs, the symbols both use counted as one.
    fn cost_with(&self, o: &Histo) -> f64 {
        let (mut total, mut used, mut sum) = (self.total, self.used, self.sum);
        for k in 0..5 {
            total[k] += o.total[k];
            used[k] += o.used[k];
            sum[k] += o.sum[k];
        }
        let (a, b) = (&self.c, &o.c);
        let (mut i, mut j) = (0, 0);
        while i < a.len() && j < b.len() {
            if a[i].0 < b[j].0 {
                i += 1;
            } else if a[i].0 > b[j].0 {
                j += 1;
            } else {
                let k = alphabet(a[i].0);
                used[k] -= 1;
                sum[k] += nlog2((a[i].1 + b[j].1) as u64) - nlog2(a[i].1 as u64) - nlog2(b[j].1 as u64);
                i += 1;
                j += 1;
            }
        }
        cost_of(&total, &used, &sum, self.extra + o.extra)
    }

    fn merge(&mut self, o: &Histo) {
        let (a, b) = (std::mem::take(&mut self.c), &o.c);
        let (mut i, mut j) = (0, 0);
        while i < a.len() || j < b.len() {
            let x = match (a.get(i), b.get(j)) {
                (Some(&x), Some(&y)) if x.0 == y.0 => {
                    (i, j) = (i + 1, j + 1);
                    (x.0, x.1 + y.1)
                }
                (Some(&x), Some(&y)) if x.0 < y.0 => {
                    i += 1;
                    x
                }
                (Some(&x), None) => {
                    i += 1;
                    x
                }
                (_, Some(&y)) => {
                    j += 1;
                    y
                }
                (None, None) => unreachable!(),
            };
            self.c.push(x);
        }
        self.total = [0; 5];
        self.used = [0; 5];
        self.sum = [0.0; 5];
        for &(s, v) in &self.c {
            let k = alphabet(s);
            self.total[k] += v as u64;
            self.used[k] += 1;
            self.sum[k] += nlog2(v as u64);
        }
        self.extra += o.extra;
    }
}

fn cost_of(total: &[u64; 5], used: &[u32; 5], sum: &[f64; 5], extra: u64) -> f64 {
    let mut bits = extra as f64;
    for k in 0..5 {
        bits += nlog2(total[k]) - sum[k];
        bits += match used[k] {
            0 | 1 => 4.0,
            2 => 20.0,
            n => 30.0 + 4.5 * n as f64,
        };
    }
    bits
}

/// Writes an image's entropy-coded data: no colour cache; for the main image (`meta`), Huffman
/// codes per region, clustered; then its codes and symbols.
fn entropy_coded(b: &mut Bits, img: &Image, meta: bool) {
    // A first pass's codes cost the second's copies.
    let first = lz77(&img.px, img.w, None);
    let costs = Costs::of(&Histo::of(first.iter().copied(), img.w));
    let syms = lz77(&img.px, img.w, Some(&costs));
    b.put(0, 1); // no colour cache
    let (groups, of_region, rbits, rw) = if meta { regions(&syms, img) } else { (vec![Histo::of(syms.iter().copied(), img.w)], vec![0u16], 0, 1) };
    if meta {
        if groups.len() > 1 {
            b.put(1, 1);
            b.put(rbits - 2, 3);
            let sub: Vec<u32> = of_region.iter().map(|&g| 0xff00_0000 | (g as u32) << 8).collect();
            entropy_coded(b, &Image { px: sub, w: rw, h: img.h.div_ceil(1 << rbits) }, false);
        } else {
            b.put(0, 1);
        }
    }
    let codes: Vec<[Code; 5]> = groups.iter().map(|h| std::array::from_fn(|k| Code::new(&h.dense(k), 15))).collect();
    for g in &codes {
        for c in g {
            c.write_header(b);
        }
    }
    let mut i = 0;
    for &s in &syms {
        let g = if groups.len() > 1 { of_region[((i / img.w) >> rbits) * rw + ((i % img.w) >> rbits)] as usize } else { 0 };
        let c = &codes[g];
        match s {
            Sym::Lit(p) => {
                c[0].put(b, ((p >> 8) & 0xff) as usize);
                c[1].put(b, ((p >> 16) & 0xff) as usize);
                c[2].put(b, (p & 0xff) as usize);
                c[3].put(b, (p >> 24) as usize);
                i += 1;
            }
            Sym::Copy { len, dist } => {
                let (ls, le, lv) = prefix(len);
                c[0].put(b, 256 + ls as usize);
                b.put(lv, le);
                let (ds, de, dv) = prefix(dist_code(dist, img.w));
                c[4].put(b, ds as usize);
                b.put(dv, de);
                i += len as usize;
            }
        }
    }
}

/// The groups of codes for regions of 32 × 32 pixels: each region's histogram, then the pair whose
/// merging saves most merged, while one does. The groups' histograms, each region's group, the
/// regions' bits and how many there are across.
fn regions(syms: &[Sym], img: &Image) -> (Vec<Histo>, Vec<u16>, u32, usize) {
    const RBITS: u32 = 5;
    let (rw, rh) = (img.w.div_ceil(1 << RBITS), img.h.div_ceil(1 << RBITS));
    let n = rw * rh;
    let mut of: Vec<Vec<Sym>> = vec![Vec::new(); n];
    let mut i = 0;
    for &s in syms {
        of[((i / img.w) >> RBITS) * rw + ((i % img.w) >> RBITS)].push(s);
        i += match s {
            Sym::Copy { len, .. } => len as usize,
            Sym::Lit(_) => 1,
        };
    }
    let mut hs: Vec<Histo> = of.into_iter().map(|v| Histo::of(v.into_iter(), img.w)).collect();
    let mut costs: Vec<f64> = hs.iter().map(Histo::cost).collect();
    let mut members: Vec<Vec<usize>> = (0..n).map(|r| vec![r]).collect();
    let mut alive = vec![true; n];
    let mut save = vec![f64::NEG_INFINITY; n * n];
    for a in 0..n {
        for c in a + 1..n {
            save[a * n + c] = costs[a] + costs[c] - hs[a].cost_with(&hs[c]);
        }
    }
    loop {
        let mut best = (0.0f64, 0usize, 0usize);
        for a in (0..n).filter(|&a| alive[a]) {
            for c in (a + 1..n).filter(|&c| alive[c]) {
                if save[a * n + c] > best.0 {
                    best = (save[a * n + c], a, c);
                }
            }
        }
        let (gain, a, c) = best;
        if gain <= 0.0 {
            break;
        }
        let other = std::mem::take(&mut hs[c]);
        hs[a].merge(&other);
        costs[a] = hs[a].cost();
        alive[c] = false;
        let moved = std::mem::take(&mut members[c]);
        members[a].extend(moved);
        for k in (0..n).filter(|&k| alive[k] && k != a) {
            let (x, y) = (a.min(k), a.max(k));
            save[x * n + y] = costs[x] + costs[y] - hs[x].cost_with(&hs[y]);
        }
    }
    let mut groups = Vec::new();
    let mut of_region = vec![0u16; n];
    for (k, h) in hs.into_iter().enumerate() {
        if alive[k] {
            for &r in &members[k] {
                of_region[r] = groups.len() as u16;
            }
            groups.push(h);
        }
    }
    (groups, of_region, RBITS, rw)
}

// ---- Huffman codes --------------------------------------------------------------------------

/// A canonical Huffman code: each symbol's length and code (bits reversed, to be put least
/// significant first).
struct Code {
    lens: Vec<u8>,
    codes: Vec<u16>,
    /// Symbols used: one or none take no bits.
    n: usize,
}

impl Code {
    /// The code for `counts`, lengths at most `limit`.
    fn new(counts: &[u32], limit: u8) -> Code {
        let lens = lengths(counts, limit);
        let codes = canonical(&lens);
        let n = lens.iter().filter(|&&l| l > 0).count();
        Code { lens, codes, n }
    }

    fn put(&self, b: &mut Bits, s: usize) {
        if self.n > 1 {
            b.put(self.codes[s] as u32, self.lens[s] as u32);
        }
    }

    /// Writes the code's lengths: the simple form for one or two symbols under 256 (none used:
    /// symbol 0), else the lengths run-length coded under a code of their own.
    fn write_header(&self, b: &mut Bits) {
        let used: Vec<usize> = (0..self.lens.len()).filter(|&s| self.lens[s] > 0).collect();
        if used.len() <= 2 && used.iter().all(|&s| s < 256) {
            b.put(1, 1);
            match *used.as_slice() {
                [] => put_simple(b, 0, 0),
                [s] => put_simple(b, 0, s),
                [s, t] => {
                    put_simple(b, 1, s);
                    b.put(t as u32, 8);
                }
                _ => unreachable!(),
            }
            return;
        }
        b.put(0, 1);
        let tokens = rle(&self.lens);
        let mut counts = [0u32; 19];
        for t in &tokens {
            counts[t.0 as usize] += 1;
        }
        let cl = Code::new(&counts, 7);
        const ORDER: [usize; 19] = [17, 18, 0, 1, 2, 3, 4, 5, 16, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15];
        let mut n = 19;
        while n > 4 && cl.lens[ORDER[n - 1]] == 0 {
            n -= 1;
        }
        b.put(n as u32 - 4, 4);
        for &k in &ORDER[..n] {
            b.put(cl.lens[k] as u32, 3);
        }
        // Trailing zero lengths left out (`max_symbol`: the tokens written), where that's shorter.
        let bits = |ts: &[(u8, u32)]| -> u32 { ts.iter().map(|t| if cl.n > 1 { cl.lens[t.0 as usize] as u32 } else { 0 } + [2, 3, 7].get((t.0 as usize).wrapping_sub(16)).copied().unwrap_or(0)).sum() };
        let last = tokens.iter().rposition(|t| !matches!(t.0, 0 | 17 | 18)).map_or(0, |p| p + 1);
        let pairs = if last >= 2 { (32 - ((last - 2) as u32).leading_zeros()).div_ceil(2).max(1) } else { 0 };
        let toks = if last >= 2 && 3 + 2 * pairs + bits(&tokens[..last]) < bits(&tokens) {
            b.put(1, 1);
            b.put(pairs - 1, 3);
            b.put(last as u32 - 2, 2 * pairs);
            &tokens[..last]
        } else {
            b.put(0, 1);
            &tokens[..]
        };
        for &(t, extra) in toks {
            cl.put(b, t as usize);
            match t {
                16 => b.put(extra, 2),
                17 => b.put(extra, 3),
                18 => b.put(extra, 7),
                _ => {}
            }
        }
    }
}

/// A simple code's first symbol: one bit for 0 or 1, else eight.
fn put_simple(b: &mut Bits, more: u32, s: usize) {
    b.put(more, 1);
    if s <= 1 {
        b.put(0, 1);
        b.put(s as u32, 1);
    } else {
        b.put(1, 1);
        b.put(s as u32, 8);
    }
}

/// Code lengths run-length coded: (token, its extra bits' value). 16 repeats the last non-zero
/// length (8 before any) 3–6 times, 17 a zero 3–10 times, 18 a zero 11–138 times.
fn rle(lens: &[u8]) -> Vec<(u8, u32)> {
    let mut out = Vec::new();
    let mut prev = 8u8;
    let mut i = 0;
    while i < lens.len() {
        let v = lens[i];
        let mut run = 1;
        while i + run < lens.len() && lens[i + run] == v {
            run += 1;
        }
        i += run;
        if v == 0 {
            while run >= 11 {
                let k = run.min(138);
                out.push((18, (k - 11) as u32));
                run -= k;
            }
            if run >= 3 {
                out.push((17, (run - 3) as u32));
                run = 0;
            }
        } else {
            if v != prev {
                out.push((v, 0));
                run -= 1;
                prev = v;
            }
            while run >= 3 {
                let k = run.min(6);
                out.push((16, (k - 3) as u32));
                run -= k;
            }
        }
        for _ in 0..run {
            out.push((v, 0));
        }
    }
    out
}

/// Huffman code lengths for `counts`, at most `limit` bits: counts under a floor raised to it, the
/// floor doubling until the tree fits (libwebp's way). One symbol used: its length 1, which a
/// decoder takes as no bits.
fn lengths(counts: &[u32], limit: u8) -> Vec<u8> {
    let mut lens = vec![0u8; counts.len()];
    let used: Vec<usize> = (0..counts.len()).filter(|&s| counts[s] > 0).collect();
    if used.len() <= 1 {
        if let Some(&s) = used.first() {
            lens[s] = 1;
        }
        return lens;
    }
    let n = used.len();
    assert!(n <= 1 << limit, "{n} symbols in codes of {limit} bits");
    let mut floor = 1u64;
    loop {
        // Two queues: the leaves by (count, symbol), then the nodes merged, in the order made.
        let mut leaves: Vec<(u64, usize)> = used.iter().map(|&s| ((counts[s] as u64).max(floor), s)).collect();
        leaves.sort_unstable();
        let mut weight: Vec<u64> = leaves.iter().map(|l| l.0).collect();
        let mut parent = vec![0usize; 2 * n - 1];
        let (mut li, mut ni) = (0, n);
        let mut take = |weight: &[u64]| {
            if li < n && (ni >= weight.len() || weight[li] <= weight[ni]) {
                li += 1;
                li - 1
            } else {
                ni += 1;
                ni - 1
            }
        };
        while weight.len() < 2 * n - 1 {
            let (a, c) = (take(&weight), take(&weight));
            parent[a] = weight.len();
            parent[c] = weight.len();
            weight.push(weight[a] + weight[c]);
        }
        let mut depth = vec![0u8; 2 * n - 1];
        for k in (0..2 * n - 2).rev() {
            depth[k] = depth[parent[k]] + 1;
        }
        if depth[..n].iter().all(|&d| d <= limit) {
            for (k, l) in leaves.iter().enumerate() {
                lens[l.1] = depth[k];
            }
            return lens;
        }
        floor *= 2;
    }
}

/// Canonical codes for `lens`, bits reversed.
fn canonical(lens: &[u8]) -> Vec<u16> {
    let mut count = [0u16; 16];
    for &l in lens {
        count[l as usize] += (l > 0) as u16;
    }
    let mut next = [0u16; 16];
    let mut code = 0u16;
    for l in 1..16 {
        code = (code + count[l - 1]) << 1;
        next[l] = code;
    }
    lens.iter()
        .map(|&l| {
            if l == 0 {
                return 0;
            }
            let c = next[l as usize];
            next[l as usize] += 1;
            c.reverse_bits() >> (16 - l)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode(webp: &[u8]) -> (u32, u32, Vec<u8>) {
        let mut d = image_webp::WebPDecoder::new(std::io::Cursor::new(webp)).unwrap();
        let mut out = vec![0u8; d.output_buffer_size().unwrap()];
        d.read_image(&mut out).unwrap();
        let (w, h) = d.dimensions();
        let rgb = if d.has_alpha() { out.as_chunks::<4>().0.iter().flat_map(|p| [p[0], p[1], p[2]]).collect() } else { out };
        (w, h, rgb)
    }

    #[test]
    fn images_decode_back() {
        let mut seed = 7u64;
        let mut rnd = |m: u32| {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            ((seed >> 33) as u32) % m
        };
        for (w, h) in [(1, 1), (2, 1), (1, 3), (5, 7), (33, 17), (64, 64), (256, 256), (300, 41)] {
            for kind in 0..7 {
                let mut rgb = vec![0u8; (w * h * 3) as usize];
                for y in 0..h {
                    for x in 0..w {
                        let i = ((y * w + x) * 3) as usize;
                        let v = match kind {
                            0 => 0,
                            1 => rnd(2),
                            2 => rnd(4),
                            3 => rnd(51) * 2,
                            4 => ((x / 7 + y / 5) % 9) * 11,
                            5 => (x * 3 + y * 5) % 256,
                            _ => rnd(256),
                        };
                        rgb[i] = if kind == 6 { rnd(256) as u8 } else { 128 + (v > 200) as u8 };
                        rgb[i + 1] = v as u8;
                        rgb[i + 2] = if kind == 6 { rnd(256) as u8 } else { 0 };
                    }
                }
                let webp = encode_rgb(&rgb, w, h);
                assert!(webp.len().is_multiple_of(2) && &webp[..4] == b"RIFF" && &webp[8..16] == b"WEBPVP8L");
                assert_eq!(decode(&webp), (w, h, rgb), "{w}x{h}, kind {kind}");
            }
        }
    }

    #[test]
    fn prefixes_as_decoders_read_them() {
        for v in 1..5000u32 {
            let (code, extra, bits) = prefix(v);
            let back = if code < 4 { code + 1 } else { ((2 + (code & 1)) << ((code - 2) >> 1)) + bits + 1 };
            assert_eq!((back, extra), (v, if code < 4 { 0 } else { (code - 2) >> 1 }));
        }
    }

    #[test]
    fn codes_complete_and_limited() {
        let counts: Vec<u32> = (0..280).map(|i| if i % 3 == 0 { 0 } else { 1 + (i * i) % 977 }).collect();
        // (Fibonacci counts would want lengths past 15.)
        let mut fib = vec![1u32, 1];
        while fib.len() < 40 {
            fib.push(fib[fib.len() - 1] + fib[fib.len() - 2]);
        }
        for (c, limit) in [(&counts, 15u8), (&counts[..19].to_vec(), 7), (&fib, 15), (&fib[..19].to_vec(), 7)] {
            let l = lengths(c, limit);
            assert!(l.iter().all(|&x| x <= limit));
            let kraft: f64 = l.iter().filter(|&&x| x > 0).map(|&x| 0.5f64.powi(x as i32)).sum();
            assert_eq!(kraft, 1.0);
        }
        assert_eq!(lengths(&[0, 5, 0], 15), [0, 1, 0]);
    }
}
