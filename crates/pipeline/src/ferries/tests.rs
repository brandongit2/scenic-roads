use super::*;

/// The Python's answers (dem/ferries.py, CPython 3.14), for the inputs beside them.
#[test]
fn text_readings_as_the_python() {
    let f = |x: f64| Some(Num::Float(x));
    let i = |x: i64| Some(Num::Int(x));
    let minutes_in = [
        "", "90", "1h30", "1h", "1h30min", "10min", "48:00", "1:05:30", " 2 H ", "1.5 mins", "1.", "x", "١٢", "12:345",
        "1h30x",
    ];
    let minutes_out = [
        None,
        f(90.0),
        i(90),
        i(60),
        i(90),
        f(10.0),
        f(2880.0),
        f(65.5),
        i(120),
        f(1.5),
        None,
        None,
        f(12.0),
        None,
        None,
    ];
    for (v, want) in minutes_in.iter().zip(minutes_out) {
        assert_eq!(minutes(Some(v)), want, "{v:?}");
    }
    let spans = ["24/7", "Mo-Su 06:00-22:00", "06:00-02:00", "Mo-Su06:00-22:00", "7:00-19:00", "Mo-Fr 06:00-22:00"];
    let want = [Some(1440), Some(960), Some(1200), None, Some(720), None];
    for (v, w) in spans.iter().zip(want) {
        assert_eq!(daily_span(Some(v)), w, "{v:?}");
    }
    let t = |kv: &[(&str, &str)]| kv.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect::<Vec<_>>();
    let seasons = [
        (t(&[("seasonal", "yes")]), (3, "Seasonal")),
        (t(&[("seasonal", "summer;winter")]), (3, "Seasonal (summer, winter)")),
        (t(&[("opening_hours", "Apr-Oct 08:00-20:00")]), (3, "Seasonal: Apr-Oct 08:00-20:00")),
        (t(&[("opening_hours", "Mo-Su 06:00-22:00")]), (1, "Daily 06:00-22:00")),
        (t(&[("opening_hours", "24/7")]), (1, "Daily")),
        (t(&[("opening_hours", "Mo-Fr 07:00-19:00")]), (2, "Mo-Fr 07:00-19:00")),
        (t(&[("opening_hours", "Mo-Fr 07:00-19:00; Sa 08:00-12:00")]), (0, "")),
        (t(&[("seasonal", "no")]), (1, "Year-round")),
        (t(&[("seasonal", "no"), ("opening_hours", "x")]), (0, "Year-round")),
        (t(&[("opening_hours", "sunrise-sunset")]), (0, "")),
        (t(&[("opening_hours", "Su 10:00-12:00")]), (2, "Su 10:00-12:00")),
        (t(&[("opening_hours", "agu")]), (3, "Seasonal: agu")),
        (t(&[]), (0, "")),
    ];
    for (tags, (c, s)) in seasons {
        assert_eq!(season_of(&tags), (c, s.to_string()), "{tags:?}");
    }
    let months = [
        ("Apr-Oct", Some(7.0)),
        ("mid-Jun–15 Sep", Some(3.0)),
        ("late May-early Sep", Some(3.5)),
        ("4 Jul-30 Aug daily; 3 Apr-2 Jul & 5 Sep-1 Nov weekends", Some(7.0)),
        ("year-round", Some(12.0)),
        ("Nov–Mar", Some(5.0)),
        ("June", Some(1.0)),
        ("0 jan", Some(0.5)),
        ("15 May 2024 - 30 Sep 2024", Some(4.5)),
        ("Seasonal: May-Sep 09:00-18:00", Some(5.0)),
        ("jul and aug", Some(2.0)),
        ("Dec. 24", Some(0.5)),
        ("early june to late august", Some(2.5)),
        ("summer", None),
        ("", None),
        ("Seasonal (summer)", None),
        ("1 Jan-31 Dec", Some(12.0)),
        ("may 5-sep 3", Some(4.0)),
        ("Jan 12:00", Some(1.0)),
        ("2024 jun", Some(0.5)),
    ];
    for (text, want) in months {
        assert_eq!(months_of_text(text), want, "{text:?}");
    }
    let names = [
        ("Ferry X: A => B", "Ferry X"),
        ("A → B", "A → B"),
        ("Line 1 : Port A -> Port B", "Line 1"),
        ("Plain", "Plain"),
        ("  Spaced  ", "Spaced"),
        ("A: B: C => D", "A: B"),
        ("X:Y-->Z", "X"),
        ("a : b —> c => d", "a"),
    ];
    for (n, want) in names {
        assert_eq!(clean_name(n), want, "{n:?}");
    }
    for (kv, want) in [
        (t(&[("operator", "NYC Ferry")]), true),
        (t(&[("network", "tan")]), true),
        (t(&[("operator", "Titan Lines")]), false),
        (t(&[("operator", "Metro de Lisboa")]), true),
        (t(&[("brand", "Exo")]), true),
        (t(&[]), false),
    ] {
        assert_eq!(is_urban(&kv), want, "{kv:?}");
    }
    assert_eq!(unesc("a%20%b%zz%41%c%%"), "a b%zzAc%%");
}

#[test]
fn hypot_is_cpythons() {
    for ((x, y), want) in [
        ((3.0, 4.0), 5.0),
        ((0.1, 0.2), 0.223606797749979),
        ((1e-300, 1e-300), 1.414213562373095e-300),
        ((123.456, 0.000789), 123.45600000252124),
        ((0.0, 0.0), 0.0),
        ((-2.5, 7.25), 7.668930825088983),
        ((5e-324, 0.0), 5e-324),
    ] {
        assert_eq!(ferrycount::hypot(x, y).to_bits(), f64::to_bits(want), "{x} {y}");
    }
}

/// A line's ends in the order CPython's set gave them (its hashes and table).
#[test]
fn line_ends_in_cpythons_set_order() {
    let n = |v: f64| Num::Float(v);
    let ways = |w: Vec<Vec<[Num; 2]>>| line_ends(&w.iter().map(|c| c.as_slice()).collect::<Vec<_>>());
    let p = |x: f64, y: f64| [n(x), n(y)];
    assert_eq!(ways(vec![vec![p(1.0, 2.0), p(3.0, 4.0)]]), [p(1.0, 2.0), p(3.0, 4.0)]);
    assert_eq!(
        ways(vec![vec![p(10.123456789, 45.987654321), p(10.2, 46.0)], vec![p(10.2, 46.0), p(10.3, 46.1)]]),
        [p(10.3, 46.1), p(10.12346, 45.98765)]
    );
    let e = ways(vec![vec![p(-0.0, 0.0), [Num::Int(0), Num::Int(1)]], vec![p(0.0, 1.0), p(5.5, 5.5)]]);
    assert_eq!(e, [p(5.5, 5.5), p(-0.0, 0.0)]);
    assert!(e[1][0].f().is_sign_negative());
    let many: Vec<Vec<[Num; 2]>> =
        (1..9).map(|i| vec![p(i as f64 * 0.37, i as f64 * -1.13), p(i as f64 * 2.1, 3.3)]).collect();
    assert_eq!(ways(many), [p(2.96, -9.04), p(16.8, 3.3)]);
    // (Nineteen ways: the set grows its table past 8 and 32 slots.)
    let grown: Vec<Vec<[Num; 2]>> =
        (1..20).map(|i| vec![p(i as f64 * 0.011, 50.0 + i as f64 * 0.007), p(i as f64 * 0.019, 51.0)]).collect();
    assert_eq!(ways(grown), [p(0.361, 51.0), p(0.011, 50.007)]);
    assert_eq!(ways(vec![vec![p(120.5, -33.25), p(120.5, -33.25)]]), [p(120.5, -33.25), p(120.5, -33.25)]);
}

#[test]
fn hashes_are_cpythons() {
    // hash(1.5), hash(-0.1), hash(1e300), hash((1.0, 2.0)): CPython 3.14's, 64-bit.
    assert_eq!(hash_float(1.5), 1_152_921_504_606_846_977);
    assert_eq!(hash_float(-0.1), -230_584_300_921_369_408);
    assert_eq!(hash_float(1e300), 1_224_995_262_755_759_164);
    assert_eq!(hash_pair(Num::Float(1.0), Num::Float(2.0)), -3_550_055_125_485_641_917);
    assert_eq!(hash_int(-1), -2);
}
