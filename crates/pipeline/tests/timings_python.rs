//! dem/timings.py, the Python twin of pipeline::timings: a step program's phases, written where
//! the scenic-build phase that runs it says (SCENIC_PHASES_TO), come in as that phase's
//! sub-phases (tools/check/timings_py.py writes them).

use pipeline::timings::{self, Class};
use std::path::Path;

#[test]
fn a_python_programs_phases_come_in_under_the_phase_that_ran_it() {
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tools/check/timings_py.py");
    timings::start("test");
    {
        let _p = timings::phase("python step", Class::Mixed);
        let mut c = std::process::Command::new("python3");
        c.arg("-I").arg(&script);
        timings::child(&mut c);
        let st = c.status().expect("python3");
        assert!(st.success());
    }
    let r = timings::snapshot(true).unwrap();
    assert_eq!(r.phases.len(), 1);
    let p = &r.phases[0];
    let subs: Vec<(&str, Class, u64)> = p.sub.iter().map(|s| (s.name.as_str(), s.class, s.n)).collect();
    // Top-level phases alone (theirs folded into them), in the order they began.
    assert_eq!(subs, [("loop stage", Class::Compute, 3), ("parent", Class::Mixed, 1), ("main work", Class::Compute, 1), ("beside", Class::NasRead, 1)]);
    let l = &p.sub[0];
    assert_eq!((l.bytes, l.files), (30, 3));
    assert!(l.wall_s >= 0.03 && l.cpu_s.is_some());
    assert!(p.sub[2].overlapped && p.sub[3].overlapped);
    assert!(p.sub.iter().all(|s| s.sub.is_empty()));
    assert!(p.wall_s >= p.sub.iter().filter(|s| !s.background).map(|s| s.wall_s).sum::<f64>());
}
