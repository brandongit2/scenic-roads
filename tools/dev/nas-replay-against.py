#!/usr/bin/env python3
"""The NAS pacing's replay and its bursts with the disks read low run on another revision's law (task
#1506), so the figures docs/pool.md §7.7 sets beside the pacing's (in brackets) can be made again.

  nas-replay-against.py <git ref> [--keep DIR]
      exports <ref>'s tree (`git archive`) into a new folder, puts this checkout's
      `the_readings_of_19_and_20_nov_let_work_go_and_cut_at_the_pools_load` (its replay and the
      helpers before it, from "A reading of the lead's, 19 Nov 12:22Z" to "The limits are the best
      of the last day's probes.") and `bursts_with_the_disks_read_low_or_not_at_all` into the ref's
      crates/jobrt/src/budget/nas.rs in place of its own replay, with `nas_readings.csv`, and runs
      both there on the test Mac (tools/app/remote-tests.sh, key `<this worktree>-against`),
      printing their lines whole. A ref whose `Sample` has no `moving` (judging against the weight,
      as main to 6e92eb8bf) is given the replay without it, its `Reading` without `outside`,
      `undecided` and `own` (taken as false); a burst without `disks` is given it; an `Answer`
      without `apart_s` (main before task #1552) is given the answers on the jobs'
      session alone, its cases with the answers apart then the same as without. The folder is
      deleted after (--keep: made there and kept).
      Exit: the run's.
"""
import os
import shutil
import subprocess
import sys
import tempfile

HERE = subprocess.run(["git", "rev-parse", "--show-toplevel"], capture_output=True, text=True, check=True).stdout.strip()
NAS = "crates/jobrt/src/budget/nas.rs"
REPLAY = ("    /// A reading of the lead's, 19 Nov 12:22Z", "    /// The limits are the best of the last day's probes.")
BURST = ("    /// Task #1506's re-review: the burst's cases with the disks read at half", REPLAY[0])
# What a ref's burst lacks for the disks read low or not at all (`Burst::disks`), as main to
# 6e92eb8bf.
DISKS = [
    ("        slip: f64,\n        k: Tuning,\n    }", "        slip: f64,\n        disks: Option<f64>,\n        k: Tuning,\n    }"),
    ("                slip: 0.0,\n                k: Tuning::default(),",
     "                slip: 0.0,\n                disks: Some(1.0),\n                k: Tuning::default(),"),
    ("let disk = [(rho * 100.0).min(100.0)];", "let disk = [(rho * 100.0 * self.disks.unwrap_or(0.0)).min(100.0)];"),
    ("busy: Some(&disk),", "busy: self.disks.map(|_| &disk[..]),"),
]
# Where a ref's own replay starts: this one's, or main's to 6e92eb8bf.
STARTS = (REPLAY[0], "    /// The lead's readings of 19 Nov, 17:00-21:50Z")


def cut(src, a, b):
    i = src.index(a)
    return src[i:src.index(b, i)]


def graft(theirs, ours):
    block, burst = cut(ours, *REPLAY), cut(ours, *BURST)
    if "pub moving:" not in theirs:
        for x, y in [("                moving: &moving,\n", ""), ("if !l.outside {", "if true {"),
                     ("p.last.as_ref().is_some_and(|l| l.undecided)", "false"),
                     ("|l| l.undecided", "|_| false"), ("|l| l.own.is_some_and(|o| o >= k.step)", "|_| false")]:
            block = block.replace(x, y)
    if "pub apart_s:" not in theirs:
        block = block.replace("apart_s: if apart { APART_S } else { 0 }, ", "")
    if "disks: Option<f64>" not in theirs:
        for x, y in DISKS:
            assert theirs.count(x) == 1, x
            theirs = theirs.replace(x, y)
    start = next(s for s in STARTS if s in theirs)
    i, j = theirs.index(start), theirs.index(REPLAY[1])
    if "fn bursts_with_the_disks_read_low_or_not_at_all" not in theirs:
        block = burst + block
    return theirs[:i] + block + theirs[j:]


def main():
    args = sys.argv[1:]
    if not args or args[0].startswith("-"):
        sys.exit(__doc__)
    ref, keep = args[0], (args[args.index("--keep") + 1] if "--keep" in args else None)
    root = keep or tempfile.mkdtemp(prefix="nas-replay-")
    os.makedirs(root, exist_ok=True)
    try:
        tar = os.path.join(root, ".tree.tar")
        subprocess.run(["git", "-C", HERE, "archive", "--format=tar", "-o", tar, ref], check=True)
        tree = os.path.join(root, "tree")
        os.makedirs(tree, exist_ok=True)
        subprocess.run(["tar", "-xf", tar, "-C", tree], check=True)
        os.remove(tar)
        theirs = open(os.path.join(tree, NAS)).read()
        ours = open(os.path.join(HERE, NAS)).read()
        open(os.path.join(tree, NAS), "w").write(graft(theirs, ours))
        shutil.copy(os.path.join(HERE, "crates/jobrt/src/budget/nas_readings.csv"),
                    os.path.join(tree, "crates/jobrt/src/budget/nas_readings.csv"))
        subprocess.run(["git", "init", "-q"], cwd=tree, check=True)
        subprocess.run(["git", "add", "-A"], cwd=tree, check=True)
        key = os.path.basename(HERE) + "-against"
        filt = "budget::nas::tests::the_readings budget::nas::tests::bursts_with_the_disks_read"
        run = subprocess.run([os.path.join(HERE, "tools/app/remote-tests.sh"), key, tree,
                              f"cargo nextest run -p jobrt --no-capture --no-fail-fast {filt}"])
        subprocess.run([os.path.join(HERE, "tools/app/remote-tests.sh"), "--drop", key])
        return run.returncode
    finally:
        if not keep:
            shutil.rmtree(root, ignore_errors=True)


if __name__ == "__main__":
    sys.exit(main())
