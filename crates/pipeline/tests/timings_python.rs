//! dem/timings.py, the Python twin of pipeline::timings: a step program's phases, written where
//! the scenic-build phase that runs it says (SCENIC_PHASES_TO), come in as that phase's
//! sub-phases (tools/check/timings_py.py writes them).
//!
//! scans: tools/check/timings_py.py dem/timings.py

use pipeline::timings;
use std::path::Path;

#[test]
fn a_python_programs_phases_come_in_under_the_phase_that_ran_it() {
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tools/check/timings_py.py");
    timings::start("test");
    {
        let _p = timings::phase("python step");
        let mut c = store::spawn::command("python3");
        c.arg("-I").arg(&script);
        timings::child(&mut c);
        let st = store::spawn::status(&mut c).expect("python3");
        assert!(st.success());
    }
    let r = timings::snapshot(true).unwrap();
    assert_eq!(r.phases.len(), 1);
    let p = &r.phases[0];
    let subs: Vec<(&str, u64)> = p.sub.iter().map(|s| (s.name.as_str(), s.n)).collect();
    // Top-level phases alone (theirs folded into them), in the order they began.
    assert_eq!(subs, [("loop stage", 3), ("parent", 1), ("waiting", 1), ("main work", 1), ("beside", 1)]);
    // No class: every phase's waits measured (task #870), by wall clock.
    assert!(p.sub.iter().all(|s| s.class.is_empty() && s.waits.is_some()), "{:?}", p.sub);
    let w = p.sub[2].waits.unwrap();
    assert!(w.nas_s >= 0.05, "two threads on the NAS for 50 ms: {w:?}");
    assert!(w.net_s >= 0.03 && w.waited() <= p.sub[2].wall_s + 1e-6, "{w:?}");
    assert_eq!(p.sub[0].waits.unwrap().waited(), 0.0);
    let l = &p.sub[0];
    assert_eq!((l.bytes, l.files), (30, 3));
    assert!(l.wall_s >= 0.03 && l.cpu_s.is_some());
    assert!(p.sub[3].overlapped && p.sub[4].overlapped);
    assert!(p.sub.iter().all(|s| s.sub.is_empty()));
    assert!(p.wall_s >= p.sub.iter().filter(|s| !s.background).map(|s| s.wall_s).sum::<f64>());
}
