//! Timetable feeds (GTFS) read as the Python counters read them, so their outputs stay the bytes
//! they made: a zip's tables by their names without folders (the last of a name wins), each a CSV
//! table read as Python's `csv` module reads it (its line ends, quoting and field limit), decoded
//! as UTF-8 with its byte-order mark dropped, and dates as Python's `datetime.date`. The counters:
//! the static files' readings (`statics`, docs/inputs.md §6.7.2), the rail counter (`railcount`) and
//! the ferry counter (`ferrycount`); their callers crate::rail's steps and crate::ferries.
//!
//! Errors keep the class Python would have raised (`PyErr`): each caller catches the ones the
//! Python caught (a feed that can't be read is one of the others' status, not the step's failure).

pub mod ferrycount;
pub mod railcount;
pub mod statics;

use std::collections::HashMap;
use std::io::{BufRead, Read};
use std::path::{Path, PathBuf};

/// The class of error Python raised.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// KeyError: a column a row doesn't have.
    Key,
    /// ValueError (and UnicodeDecodeError, one of them).
    Value,
    /// TypeError or AttributeError: a missing cell (None) used as text.
    Type,
    /// zipfile.BadZipFile.
    BadZip,
    /// csv.Error.
    Csv,
    /// zlib.error.
    Zlib,
    /// EOFError.
    Eof,
    /// OSError.
    Os,
}

/// An error as Python's: its class and `str(e)`.
#[derive(Clone, Debug)]
pub struct PyErr {
    pub kind: Kind,
    pub msg: String,
}

impl std::fmt::Display for PyErr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.msg)
    }
}

impl std::error::Error for PyErr {}

impl PyErr {
    pub fn new(kind: Kind, msg: impl Into<String>) -> PyErr {
        PyErr { kind, msg: msg.into() }
    }
    /// A KeyError's `str`: the key's repr.
    pub fn key(k: &str) -> PyErr {
        PyErr::new(Kind::Key, py_repr(k))
    }
    pub fn value(msg: impl Into<String>) -> PyErr {
        PyErr::new(Kind::Value, msg)
    }
    /// A missing cell used as text.
    pub fn none() -> PyErr {
        PyErr::new(Kind::Type, "'NoneType' object has no attribute 'strip'")
    }
    /// Python's class name.
    pub fn class(&self) -> &'static str {
        match self.kind {
            Kind::Key => "KeyError",
            Kind::Value => "ValueError",
            Kind::Type => "TypeError",
            Kind::BadZip => "BadZipFile",
            Kind::Csv => "Error",
            Kind::Zlib => "error",
            Kind::Eof => "EOFError",
            Kind::Os => "OSError",
        }
    }
    /// One of zipfile's, csv's, zlib's and EOF's, or a KeyError: what a reader of a feed takes for
    /// a feed that can't be read.
    pub fn unreadable(&self) -> bool {
        matches!(self.kind, Kind::Key | Kind::BadZip | Kind::Csv | Kind::Zlib | Kind::Eof)
    }
}

pub type R<T> = std::result::Result<T, PyErr>;

/// Python's `repr` of a string (its quotes as Python picks them).
pub fn py_repr(s: &str) -> String {
    let q = if s.contains('\'') && !s.contains('"') { '"' } else { '\'' };
    let mut o = String::from(q);
    for c in s.chars() {
        match c {
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            '\r' => o.push_str("\\r"),
            '\t' => o.push_str("\\t"),
            c if c == q => {
                o.push('\\');
                o.push(c);
            }
            c if (c as u32) < 0x20 || c as u32 == 0x7f => o.push_str(&format!("\\x{:02x}", c as u32)),
            c => o.push(c),
        }
    }
    o.push(q);
    o
}

/// Python's `int(s)` of a string: surrounding white space, a sign, any script's decimal digits,
/// underscores between digits; None where it raises ValueError.
pub fn py_int(s: &str) -> Option<i64> {
    let t = crate::pytext::strip(s);
    let (neg, digits) = match t.strip_prefix('-') {
        Some(r) => (true, r),
        None => (false, t.strip_prefix('+').unwrap_or(t)),
    };
    let mut v: i64 = 0;
    let mut prev_digit = false;
    let mut any = false;
    for c in digits.chars() {
        if c == '_' {
            if !prev_digit {
                return None;
            }
            prev_digit = false;
            continue;
        }
        let d = crate::pytext::decimal(c)?;
        v = v.checked_mul(10)?.checked_add(d as i64)?;
        prev_digit = true;
        any = true;
    }
    (any && prev_digit).then_some(if neg { -v } else { v })
}

/// Python's `float(s)` of a string.
pub fn py_float(s: &str) -> Option<f64> {
    crate::pytext::float(s)
}

// ---- Numbers -------------------------------------------------------------------------------------

/// A number as Python holds it: an int stays an int (its JSON `10`, not `10.0`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Num {
    Int(i64),
    Float(f64),
}

impl Num {
    pub fn f(self) -> f64 {
        match self {
            Num::Int(i) => i as f64,
            Num::Float(f) => f,
        }
    }
    pub fn of(v: &crate::pyjson::Py) -> Option<Num> {
        use crate::pyjson::Py;
        match v {
            Py::Int(s) => s.parse().ok().map(Num::Int),
            Py::Float(f) => Some(Num::Float(*f)),
            Py::Bool(b) => Some(Num::Int(*b as i64)),
            _ => None,
        }
    }
    pub fn py(self) -> crate::pyjson::Py {
        match self {
            Num::Int(i) => crate::pyjson::Py::Int(i.to_string()),
            Num::Float(f) => crate::pyjson::Py::Float(f),
        }
    }
    /// `round(x, n)`: an int as it is.
    pub fn round(self, n: usize) -> Num {
        match self {
            Num::Int(i) => Num::Int(i),
            Num::Float(f) => Num::Float(crate::interest::py_round(f, n)),
        }
    }
}

/// Python's `sum()` of numbers: ints added exactly while all are ints; from the first float on,
/// Neumaier's compensated sum (CPython 3.12 on), ints after it added as they are.
pub fn py_sum(items: impl IntoIterator<Item = Num>) -> Num {
    let mut it = items.into_iter();
    let mut i_sum: i64 = 0;
    let (mut hi, mut lo) = loop {
        match it.next() {
            None => return Num::Int(i_sum),
            Some(Num::Int(i)) => i_sum += i,
            Some(Num::Float(x)) => break (i_sum as f64 + x, 0.0f64),
        }
    };
    for n in it {
        match n {
            Num::Int(i) => hi += i as f64,
            Num::Float(x) => {
                let t = hi + x;
                if hi.abs() >= x.abs() {
                    lo += (hi - t) + x;
                } else {
                    lo += (x - t) + hi;
                }
                hi = t;
            }
        }
    }
    // (The compensation added only when it's finite and not zero, as CPython does.)
    Num::Float(if lo != 0.0 && lo.is_finite() { hi + lo } else { hi })
}

// ---- Dates ---------------------------------------------------------------------------------------

/// A day, as Python's `datetime.date`: its proleptic Gregorian ordinal (1 January of year 1 is 1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Date(pub i32);

fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn leap(y: i64) -> bool {
    y % 4 == 0 && (y % 100 != 0 || y % 400 == 0)
}

fn month_len(y: i64, m: i64) -> i64 {
    match m {
        2 if leap(y) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// Days from 1970-01-01 to 0001-01-01's ordinal 1.
const ORD_1970: i64 = 719_163;

impl Date {
    /// `date(y, m, d)`, None where it raises ValueError.
    pub fn new(y: i64, m: i64, d: i64) -> Option<Date> {
        if !(1..=9999).contains(&y) || !(1..=12).contains(&m) || d < 1 || d > month_len(y, m) {
            return None;
        }
        Some(Date((days_from_civil(y, m, d) + ORD_1970) as i32))
    }
    /// (year, month, day).
    pub fn ymd(self) -> (i64, i64, i64) {
        let z = self.0 as i64 - ORD_1970 + 719_468;
        let era = z.div_euclid(146_097);
        let doe = z - era * 146_097;
        let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = doy - (153 * mp + 2) / 5 + 1;
        let m = if mp < 10 { mp + 3 } else { mp - 9 };
        (yoe + era * 400 + (m <= 2) as i64, m, d)
    }
    pub fn plus(self, days: i64) -> Date {
        Date(self.0 + days as i32)
    }
    /// `weekday()`: Monday 0.
    pub fn weekday(self) -> i64 {
        (self.0 as i64 + 6).rem_euclid(7)
    }
    pub fn month(self) -> i64 {
        self.ymd().1
    }
    /// `isoformat()`.
    pub fn iso(self) -> String {
        let (y, m, d) = self.ymd();
        format!("{y:04}-{m:02}-{d:02}")
    }
    /// `fromisoformat` of `YYYY-MM-DD`.
    pub fn parse_iso(s: &str) -> Option<Date> {
        let b = s.as_bytes();
        if b.len() != 10 || b[4] != b'-' || b[7] != b'-' {
            return None;
        }
        let n = |r: std::ops::Range<usize>| -> Option<i64> {
            let t = &s[r];
            t.bytes().all(|c| c.is_ascii_digit()).then(|| t.parse().ok())?
        };
        Date::new(n(0..4)?, n(5..7)?, n(8..10)?)
    }
    /// `isocalendar()[:2]`: the ISO year and week.
    pub fn iso_week(self) -> (i64, i64) {
        let thursday = self.plus(3 - self.weekday());
        let y = thursday.ymd().0;
        let jan1 = Date::new(y, 1, 1).unwrap();
        (y, (thursday.0 - jan1.0) as i64 / 7 + 1)
    }
}

/// Today in UTC.
pub fn today() -> Date {
    let s = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0) as i64;
    Date((s.div_euclid(86400) + ORD_1970) as i32)
}

/// `date(int(s[:4]), int(s[4:6]), int(s[6:8]))`: a GTFS date.
pub fn ymd(s: &str) -> R<Date> {
    let c: Vec<char> = s.chars().collect();
    let part = |a: usize, b: usize| -> String { c[a.min(c.len())..b.min(c.len())].iter().collect() };
    let int = |t: String| {
        py_int(&t).ok_or_else(|| PyErr::value(format!("invalid literal for int() with base 10: {}", py_repr(&t))))
    };
    let (y, m, d) = (int(part(0, 4))?, int(part(4, 6))?, int(part(6, 8))?);
    Date::new(y, m, d).ok_or_else(|| PyErr::value("day is out of range for month"))
}

// ---- Zips ----------------------------------------------------------------------------------------

/// A zip's tables, by their names without folders.
pub struct Zip {
    path: PathBuf,
    members: HashMap<String, crate::lakefill::zip::Member>,
}

/// An I/O error while a member is read, as the class Python's zipfile and zlib raise.
fn read_err(e: std::io::Error) -> PyErr {
    let msg = e.to_string();
    if msg.contains("CRC or size wrong") {
        PyErr::new(Kind::BadZip, msg)
    } else {
        match e.kind() {
            std::io::ErrorKind::UnexpectedEof => PyErr::new(Kind::Eof, msg),
            std::io::ErrorKind::InvalidData | std::io::ErrorKind::InvalidInput => PyErr::new(Kind::Zlib, msg),
            _ => PyErr::new(Kind::Os, msg),
        }
    }
}

impl Zip {
    /// `zipfile.ZipFile(path)`: OSError when it can't be opened, BadZipFile when it isn't a zip.
    pub fn open(path: &Path) -> R<Zip> {
        std::fs::File::open(path)
            .map_err(|e| PyErr::new(Kind::Os, format!("{e}: {}", py_repr(&path.to_string_lossy()))))?;
        let ms = crate::lakefill::zip::members(path)
            .map_err(|e| PyErr::new(Kind::BadZip, format!("File is not a zip file ({e:#})")))?;
        let mut members = HashMap::new();
        for m in ms {
            let base = m.name.rsplit('/').next().unwrap_or(&m.name).to_string();
            members.insert(base, m);
        }
        Ok(Zip { path: path.to_path_buf(), members })
    }

    /// Table `name`'s records (None when the zip hasn't it), decoded strictly (`lossy`: invalid
    /// UTF-8 replaced, as `errors="replace"`).
    pub fn csv(&self, name: &str, lossy: bool) -> R<Option<Csv>> {
        let Some(m) = self.members.get(name) else { return Ok(None) };
        let r = crate::lakefill::zip::open(&self.path, m).map_err(|e| PyErr::new(Kind::BadZip, format!("{e:#}")))?;
        Ok(Some(Csv::new(Box::new(std::io::BufReader::with_capacity(1 << 16, r)), lossy)))
    }

    /// `csv.DictReader` over table `name` (strict UTF-8): its rows; none when the zip hasn't it.
    pub fn dict_rows(&self, name: &str) -> R<DictRows> {
        DictRows::new(self.csv(name, false)?)
    }

    /// The rail counter's rows of table `name`: invalid UTF-8 replaced, the header's names and
    /// every cell stripped, a row's cells beyond the header dropped and missing ones absent.
    pub fn stripped_rows(&self, name: &str) -> R<StrippedRows> {
        StrippedRows::new(self.csv(name, true)?)
    }

    /// A member's bytes, whole.
    pub fn read(&self, name: &str) -> R<Option<Vec<u8>>> {
        let Some(m) = self.members.get(name) else { return Ok(None) };
        let mut r =
            crate::lakefill::zip::open(&self.path, m).map_err(|e| PyErr::new(Kind::BadZip, format!("{e:#}")))?;
        let mut v = Vec::new();
        r.read_to_end(&mut v).map_err(read_err)?;
        Ok(Some(v))
    }
}

// ---- CSV, as Python's csv module reads it ---------------------------------------------------------

/// The longest field Python's csv reads (`csv.field_size_limit()`).
const FIELD_LIMIT: usize = 131_072;

#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    StartRecord,
    StartField,
    InField,
    InQuoted,
    QuoteInQuoted,
    EatCrnl,
}

/// How the text a CSV reader is given splits into lines (Python's `newline` argument).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Lines {
    /// `newline=""`: lines end at `\r\n`, `\r` or `\n`, kept as they are.
    Raw,
    /// `newline=None` (`open`'s default): the same ends, each read as `\n`.
    Universal,
    /// `io.StringIO`'s: lines end at `\n` alone.
    LfOnly,
}

/// A CSV table's records as `csv.reader` gives them (the excel dialect, a file opened with
/// `newline=""` unless `lines` says otherwise): quoted fields keep their line ends; an empty line
/// is an empty record.
pub struct Csv {
    r: Box<dyn BufRead + Send>,
    lossy: bool,
    lines: Lines,
    first: bool,
    line: Vec<u8>,
    fields: Vec<String>,
    field: String,
    field_len: usize,
    state: State,
}

/// The csv module's text-decoding view: one decoder for a file, so a byte-order mark only counts
/// at its start.
fn decode(bytes: &[u8], lossy: bool) -> R<std::borrow::Cow<'_, str>> {
    if lossy {
        Ok(String::from_utf8_lossy(bytes))
    } else {
        std::str::from_utf8(bytes)
            .map(std::borrow::Cow::Borrowed)
            .map_err(|e| PyErr::value(format!("'utf-8' codec can't decode bytes: {e}")))
    }
}

impl Csv {
    pub fn new(r: Box<dyn BufRead + Send>, lossy: bool) -> Csv {
        Csv::with_lines(r, lossy, Lines::Raw)
    }

    pub fn with_lines(r: Box<dyn BufRead + Send>, lossy: bool, lines: Lines) -> Csv {
        Csv {
            r,
            lossy,
            lines,
            first: true,
            line: Vec::new(),
            fields: Vec::new(),
            field: String::new(),
            field_len: 0,
            state: State::StartRecord,
        }
    }

    /// The next line's bytes, its end included (`\r\n`, `\r` or `\n`); false at the end.
    fn next_line(&mut self) -> R<bool> {
        self.line.clear();
        loop {
            let buf = self.r.fill_buf().map_err(read_err)?;
            if buf.is_empty() {
                self.universal();
                return Ok(!self.line.is_empty());
            }
            if self.line.last() == Some(&b'\r') && self.lines != Lines::LfOnly {
                // (A `\r` ends a line; a `\n` right after it is the same end.)
                if buf[0] == b'\n' {
                    self.line.push(b'\n');
                    self.r.consume(1);
                }
                self.universal();
                return Ok(true);
            }
            let lf_only = self.lines == Lines::LfOnly;
            match buf.iter().position(|&b| b == b'\n' || (b == b'\r' && !lf_only)) {
                Some(i) => {
                    let end = buf[i];
                    self.line.extend_from_slice(&buf[..=i]);
                    self.r.consume(i + 1);
                    if end == b'\n' {
                        self.universal();
                        return Ok(true);
                    }
                }
                None => {
                    let n = buf.len();
                    self.line.extend_from_slice(buf);
                    self.r.consume(n);
                }
            }
        }
    }

    /// A line's end read as `\n` (`Lines::Universal`).
    fn universal(&mut self) {
        if self.lines != Lines::Universal {
            return;
        }
        if self.line.ends_with(b"\r\n") {
            self.line.truncate(self.line.len() - 2);
            self.line.push(b'\n');
        } else if self.line.last() == Some(&b'\r') {
            *self.line.last_mut().unwrap() = b'\n';
        }
    }

    fn save_field(&mut self) {
        self.fields.push(std::mem::take(&mut self.field));
        self.field_len = 0;
    }

    fn add_char(&mut self, c: char) -> R<()> {
        if self.field_len >= FIELD_LIMIT {
            return Err(PyErr::new(Kind::Csv, format!("field larger than field limit ({FIELD_LIMIT})")));
        }
        self.field.push(c);
        self.field_len += 1;
        Ok(())
    }

    /// One character, or the end of a line (None), as CPython's `parse_process_char`.
    fn process(&mut self, c: Option<char>) -> R<()> {
        let nl = |c: Option<char>| matches!(c, Some('\n' | '\r'));
        loop {
            match self.state {
                State::StartRecord => {
                    if c.is_none() {
                        return Ok(());
                    }
                    if nl(c) {
                        self.state = State::EatCrnl;
                        return Ok(());
                    }
                    self.state = State::StartField;
                    continue;
                }
                State::StartField => {
                    if c.is_none() || nl(c) {
                        self.save_field();
                        self.state = if c.is_none() { State::StartRecord } else { State::EatCrnl };
                    } else if c == Some('"') {
                        self.state = State::InQuoted;
                    } else if c == Some(',') {
                        self.save_field();
                    } else {
                        self.add_char(c.unwrap())?;
                        self.state = State::InField;
                    }
                }
                State::InField => {
                    if c.is_none() || nl(c) {
                        self.save_field();
                        self.state = if c.is_none() { State::StartRecord } else { State::EatCrnl };
                    } else if c == Some(',') {
                        self.save_field();
                        self.state = State::StartField;
                    } else {
                        self.add_char(c.unwrap())?;
                    }
                }
                State::InQuoted => match c {
                    None => {}
                    Some('"') => self.state = State::QuoteInQuoted,
                    Some(c) => self.add_char(c)?,
                },
                State::QuoteInQuoted => {
                    if c == Some('"') {
                        self.add_char('"')?;
                        self.state = State::InQuoted;
                    } else if c == Some(',') {
                        self.save_field();
                        self.state = State::StartField;
                    } else if c.is_none() || nl(c) {
                        self.save_field();
                        self.state = if c.is_none() { State::StartRecord } else { State::EatCrnl };
                    } else {
                        self.add_char(c.unwrap())?;
                        self.state = State::InField;
                    }
                }
                State::EatCrnl => {
                    if c.is_none() {
                        self.state = State::StartRecord;
                    } else if !nl(c) {
                        return Err(PyErr::new(
                            Kind::Csv,
                            "new-line character seen in unquoted field - do you need to open the file with newline=''?",
                        ));
                    }
                }
            }
            return Ok(());
        }
    }

    /// The next record; None at the end.
    pub fn record(&mut self) -> R<Option<Vec<String>>> {
        self.fields.clear();
        self.field.clear();
        self.field_len = 0;
        loop {
            if !self.next_line()? {
                // The end: a quoted field left open is the record's last.
                if self.field_len != 0 || self.state == State::InQuoted {
                    self.save_field();
                    self.state = State::StartRecord;
                    return Ok(Some(std::mem::take(&mut self.fields)));
                }
                return Ok(None);
            }
            let line = std::mem::take(&mut self.line);
            let mut text = decode(&line, self.lossy)?;
            if self.first {
                self.first = false;
                if let Some(t) = text.strip_prefix('\u{feff}') {
                    text = std::borrow::Cow::Owned(t.to_string());
                }
            }
            // (A line starting a record with no quote and no line end before its own: its fields
            // split at the commas, as the state machine would.)
            let body = text.trim_end_matches(['\r', '\n']);
            let end_len = text.len() - body.len();
            let plain_end = matches!(&text[body.len()..], "" | "\n" | "\r" | "\r\n") && end_len <= 2;
            if self.state == State::StartRecord && plain_end && !body.contains(['"', '\r', '\n']) {
                if !body.is_empty() || end_len == 0 {
                    for f in body.split(',') {
                        if f.len() > FIELD_LIMIT && f.chars().count() > FIELD_LIMIT {
                            return Err(PyErr::new(
                                Kind::Csv,
                                format!("field larger than field limit ({FIELD_LIMIT})"),
                            ));
                        }
                        self.fields.push(f.to_string());
                    }
                }
                drop(text);
                self.line = line;
                return Ok(Some(std::mem::take(&mut self.fields)));
            }
            for c in text.chars() {
                self.process(Some(c))?;
            }
            self.process(None)?;
            drop(text);
            self.line = line;
            if self.state == State::StartRecord {
                return Ok(Some(std::mem::take(&mut self.fields)));
            }
        }
    }
}

/// A row's cell by its column's name, as a dict of the row has it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cell<'a> {
    /// No such key.
    Absent,
    /// The key, its value None (a row shorter than the header, `csv.DictReader`'s `restval`).
    None,
    Str(&'a str),
}

impl<'a> Cell<'a> {
    /// `d.get(k)`: None for both.
    pub fn get(self) -> Option<&'a str> {
        match self {
            Cell::Str(s) => Some(s),
            _ => None,
        }
    }
    /// `d.get(k) or ""`.
    pub fn or_empty(self) -> &'a str {
        self.get().unwrap_or("")
    }
    /// `d[k]` used as text: KeyError when absent, TypeError when None.
    pub fn req(self, k: &str) -> R<&'a str> {
        match self {
            Cell::Str(s) => Ok(s),
            Cell::Absent => Err(PyErr::key(k)),
            Cell::None => Err(PyErr::none()),
        }
    }
    /// `d.get(k, "")` used as text: "" when absent, TypeError when None.
    pub fn get_or_empty(self) -> R<&'a str> {
        match self {
            Cell::Str(s) => Ok(s),
            Cell::Absent => Ok(""),
            Cell::None => Err(PyErr::none()),
        }
    }
}

/// A header's columns: each name's index (a repeated name: its last).
#[derive(Clone, Default)]
pub struct Head {
    idx: HashMap<String, usize>,
}

impl Head {
    fn new(names: &[String]) -> Head {
        let mut idx = HashMap::new();
        for (i, n) in names.iter().enumerate() {
            idx.insert(n.clone(), i);
        }
        Head { idx }
    }
}

/// A row, by its header's names.
pub struct Row<'h> {
    head: &'h Head,
    pub cells: Vec<String>,
    /// Whether cells missing are None (a DictReader's) rather than absent (the rail counter's).
    restval: bool,
}

impl Row<'_> {
    pub fn cell(&self, k: &str) -> Cell<'_> {
        self.at(self.head.col(k))
    }
    /// The cell of column `i` (`Head::col`): a hot loop's lookup without hashing the name.
    pub fn at(&self, i: Option<usize>) -> Cell<'_> {
        match i {
            None => Cell::Absent,
            Some(i) => match self.cells.get(i) {
                Some(s) => Cell::Str(s),
                None if self.restval => Cell::None,
                None => Cell::Absent,
            },
        }
    }
    /// An owned copy: each column name with its value (None: missing), in the header's order.
    pub fn to_map(&self) -> OwnedRow {
        let mut names: Vec<(&String, &usize)> = self.head.idx.iter().collect();
        names.sort_by_key(|(_, &i)| i);
        let mut m = Vec::new();
        for (n, &i) in names {
            match self.cells.get(i) {
                Some(s) => m.push((n.clone(), Some(s.clone()))),
                None if self.restval => m.push((n.clone(), None)),
                None => {}
            }
        }
        OwnedRow(m)
    }
}

/// A row kept: (name, value) in its header's order.
#[derive(Clone, Debug, Default)]
pub struct OwnedRow(pub Vec<(String, Option<String>)>);

impl OwnedRow {
    pub fn cell(&self, k: &str) -> Cell<'_> {
        match self.0.iter().find(|(n, _)| n == k) {
            None => Cell::Absent,
            Some((_, None)) => Cell::None,
            Some((_, Some(s))) => Cell::Str(s),
        }
    }
}

/// `csv.DictReader`'s rows.
pub struct DictRows {
    csv: Option<Csv>,
    pub head: Head,
}

impl DictRows {
    pub fn new(mut csv: Option<Csv>) -> R<DictRows> {
        let head = match csv.as_mut() {
            Some(c) => Head::new(&c.record()?.unwrap_or_default()),
            None => Head::default(),
        };
        Ok(DictRows { csv, head })
    }
    /// The next row's cells (empty records skipped); None at the end.
    pub fn next_cells(&mut self) -> R<Option<Vec<String>>> {
        let Some(c) = self.csv.as_mut() else { return Ok(None) };
        loop {
            match c.record()? {
                Some(r) if r.is_empty() => continue,
                r => return Ok(r),
            }
        }
    }
    pub fn row(&self, cells: Vec<String>) -> Row<'_> {
        Row { head: &self.head, cells, restval: true }
    }
    /// Every row, kept.
    pub fn all(mut self) -> R<Vec<OwnedRow>> {
        let mut v = Vec::new();
        while let Some(c) = self.next_cells()? {
            v.push(self.row(c).to_map());
        }
        Ok(v)
    }
}

/// The rail counter's rows (`Zip::stripped_rows`): empty records are empty rows.
pub struct StrippedRows {
    csv: Option<Csv>,
    pub head: Head,
}

impl StrippedRows {
    fn new(mut csv: Option<Csv>) -> R<StrippedRows> {
        let head = match csv.as_mut() {
            Some(c) => {
                let names: Vec<String> =
                    c.record()?.unwrap_or_default().iter().map(|h| crate::pytext::strip(h).to_string()).collect();
                Head::new(&names)
            }
            None => Head::default(),
        };
        Ok(StrippedRows { csv, head })
    }
    pub fn next_cells(&mut self) -> R<Option<Vec<String>>> {
        let Some(c) = self.csv.as_mut() else { return Ok(None) };
        Ok(c.record()?.map(|r| {
            let mut r: Vec<String> = r
                .into_iter()
                .map(|v| {
                    let t = crate::pytext::strip(&v);
                    if t.len() == v.len() {
                        v
                    } else {
                        t.to_string()
                    }
                })
                .collect();
            r.truncate(self.head.width());
            r
        }))
    }
    pub fn row(&self, cells: Vec<String>) -> Row<'_> {
        Row { head: &self.head, cells, restval: false }
    }
    pub fn all(mut self) -> R<Vec<OwnedRow>> {
        let mut v = Vec::new();
        while let Some(c) = self.next_cells()? {
            v.push(self.row(c).to_map());
        }
        Ok(v)
    }
}

impl Head {
    /// Column `k`'s index.
    pub fn col(&self, k: &str) -> Option<usize> {
        self.idx.get(k).copied()
    }
    /// The header's length (`zip(head, row)` stops there).
    fn width(&self) -> usize {
        self.idx.values().max().map_or(0, |m| m + 1)
    }
}

#[cfg(test)]
mod tests;
