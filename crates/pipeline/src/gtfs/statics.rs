//! Static GTFS, compiled by hand (docs/inputs.md §6.7.2): what the rail and ferry counters read of
//! the accepted zips of inputs/timetables/gtfs/, beyond what a feed has.
//!
//! - No expiry, but seasons: `calendar.txt`'s dates are read as month and day, the year ignored,
//!   over one reference year (`REF_YEAR`, not a leap year: a range ending on 29 February ends on
//!   the 28th); a start after the end wraps the new year. `calendar_dates.txt` is ignored.
//! - `counts.txt`: services known only by counts, so no departure time is invented. A row:
//!   `route_id`; for rail, `from_stop_id` and `to_stop_id` (trains each way between them) and
//!   `at_least` (1: a lower bound); then `per_day`, `per_week`, `per_day_low`, `headway`, `days`,
//!   `months`, `season`, `overnight` and `note`, each taken as given (an empty cell: not given).
//!   Numbers are read as written (`10` a whole number, `1.5` not; `null`: the source read gives
//!   none), `overnight` as 1 or 0, a `headway` that isn't a number as the text the source words it
//!   in.
//! - `routes.txt`'s extra columns: `checked` (when the source was read), `months`, and `osm` (the
//!   OSM line the route is pinned to: `w<id>` or `r<id>`).

use super::{Date, OwnedRow, PyErr, Zip, R};
use crate::pyjson::Py;
use std::collections::HashMap;

pub const REF_YEAR: i64 = 2001;
const WEEKDAYS: [&str; 7] = ["monday", "tuesday", "wednesday", "thursday", "friday", "saturday", "sunday"];
const COUNT_FIELDS: [&str; 9] =
    ["per_day", "per_week", "per_day_low", "headway", "days", "months", "season", "overnight", "note"];
const NUMBERS: [&str; 4] = ["per_day", "per_week", "per_day_low", "headway"];

/// Python's `s[a:b]` of characters, from the end (`s[-4:-2]`: `from_end(s, 4, 2)`).
fn tail_slice(s: &[char], from: usize, to: usize) -> String {
    let n = s.len();
    s[n.saturating_sub(from)..n.saturating_sub(to).max(n.saturating_sub(from))].iter().collect()
}

/// A calendar date read as month and day, in the reference year.
pub fn month_day(s: &str) -> R<Date> {
    let c: Vec<char> = crate::pytext::strip(s).chars().collect();
    let int = |t: String| {
        super::py_int(&t)
            .ok_or_else(|| PyErr::value(format!("invalid literal for int() with base 10: {}", super::py_repr(&t))))
    };
    let (m, mut d) = (int(tail_slice(&c, 4, 2))?, int(tail_slice(&c, 2, 0))?);
    if (m, d) == (2, 29) {
        d = 28;
    }
    Date::new(REF_YEAR, m, d).ok_or_else(|| PyErr::value("month must be in 1..12"))
}

/// Each service's days in the reference year, from `calendar.txt` read as months and days.
pub fn service_dates(z: &Zip) -> R<HashMap<String, Vec<Date>>> {
    let mut dates: HashMap<String, std::collections::BTreeSet<Date>> = HashMap::new();
    let (first, last) = (Date::new(REF_YEAR, 1, 1).unwrap(), Date::new(REF_YEAR, 12, 31).unwrap());
    let mut rows = z.dict_rows("calendar.txt")?;
    while let Some(cells) = rows.next_cells()? {
        let c = rows.row(cells);
        let span = (|| -> R<(Date, Date)> {
            Ok((month_day(c.cell("start_date").req("start_date")?)?, month_day(c.cell("end_date").req("end_date")?)?))
        })();
        let (a, b) = match span {
            Ok(x) => x,
            Err(e) if matches!(e.kind, super::Kind::Key | super::Kind::Value) => continue,
            Err(e) => return Err(e),
        };
        let mut wd = [false; 7];
        for (k, name) in WEEKDAYS.iter().enumerate() {
            wd[k] = crate::pytext::strip(c.cell(name).get_or_empty()?) == "1";
        }
        let spans = if a <= b { vec![(a, b)] } else { vec![(first, b), (a, last)] };
        for (lo, hi) in spans {
            let mut d = lo;
            while d <= hi {
                if wd[d.weekday() as usize] {
                    dates.entry(c.cell("service_id").req("service_id")?.to_string()).or_default().insert(d);
                }
                d = d.plus(1);
            }
        }
    }
    Ok(dates.into_iter().map(|(k, v)| (k, v.into_iter().collect())).collect())
}

/// A number as written: a whole number an int, else a float; None when it isn't one.
pub fn number(s: &str) -> Option<Py> {
    let digits = |t: &str| !t.is_empty() && t.chars().all(|c| crate::pytext::decimal(c).is_some());
    let body = s.strip_prefix('-').unwrap_or(s);
    if digits(body) {
        return Some(Py::Int(super::py_int(s)?.to_string()));
    }
    // -?(\d+\.\d*|\.\d+|\d+)([eE][-+]?\d+)?
    let (mant, exp) = match body.find(['e', 'E']) {
        Some(i) => (&body[..i], Some(&body[i + 1..])),
        None => (body, None),
    };
    let mant_ok = match mant.split_once('.') {
        Some((a, b)) => (digits(a) && (b.is_empty() || digits(b))) || (a.is_empty() && digits(b)),
        None => digits(mant),
    };
    let exp_ok = exp.is_none_or(|e| digits(e.strip_prefix(['-', '+']).unwrap_or(e)));
    (mant_ok && exp_ok).then(|| super::py_float(s).map(Py::Float))?
}

/// A count row's given fields, typed (`COUNT_FIELDS`; empty cells left out).
pub fn count_values(row: &OwnedRow) -> Vec<(String, Py)> {
    let mut out = Vec::new();
    for k in COUNT_FIELDS {
        let v = row.cell(k).or_empty();
        if v.is_empty() {
            continue;
        }
        let x = if NUMBERS.contains(&k) {
            if v == "null" {
                Py::Null
            } else {
                number(v).unwrap_or_else(|| Py::Str(v.to_string()))
            }
        } else if k == "overnight" {
            Py::Bool(v == "1")
        } else {
            Py::Str(v.to_string())
        };
        out.push((k.to_string(), x));
    }
    out
}

pub fn count_rows(z: &Zip) -> R<Vec<OwnedRow>> {
    z.dict_rows("counts.txt")?.all()
}

/// routes.txt by `route_id` (a repeated id: its last row, at its first's place).
pub fn routes(z: &Zip) -> R<Vec<(String, OwnedRow)>> {
    let mut out: Vec<(String, OwnedRow)> = Vec::new();
    let mut at: HashMap<String, usize> = HashMap::new();
    for r in z.dict_rows("routes.txt")?.all()? {
        let id = r.cell("route_id").req("route_id")?.to_string();
        match at.get(&id) {
            Some(&i) => out[i].1 = r,
            None => {
                at.insert(id.clone(), out.len());
                out.push((id, r));
            }
        }
    }
    Ok(out)
}

/// The ferry records of a zip's count rows, in their order: the record of the route's pinned OSM
/// line (`line`), its source as the route names it (`source`: route_long_name, `url`: route_url,
/// `checked`) and the row's fields as given.
pub fn ferry_counts(z: &Zip) -> R<Vec<Py>> {
    let rs: HashMap<String, OwnedRow> = routes(z)?.into_iter().collect();
    let mut out = Vec::new();
    for c in count_rows(z)? {
        // (`c.get("route_id", "")`: a missing cell, None, names no route.)
        let r = match c.cell("route_id") {
            super::Cell::Absent => rs.get(""),
            super::Cell::None => None,
            super::Cell::Str(id) => rs.get(id),
        };
        let Some(r) = r else { continue };
        let pin = crate::pytext::strip(r.cell("osm").or_empty());
        if !super::ferrycount::is_ferry(r.cell("route_type").get_or_empty()?) || pin.is_empty() {
            continue;
        }
        let mut rec =
            vec![("line".to_string(), Py::Str(pin.to_string())), ("kind".into(), Py::Str("timetable".into()))];
        rec.extend(count_values(&c));
        for (k, col) in [("source", "route_long_name"), ("url", "route_url"), ("checked", "checked")] {
            if let Some(v) = r.cell(col).get().filter(|v| !v.is_empty()) {
                rec.push((k.to_string(), Py::Str(v.to_string())));
            }
        }
        out.push(Py::Dict(rec));
    }
    Ok(out)
}
