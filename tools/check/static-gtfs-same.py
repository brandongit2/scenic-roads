"""The proof of #141 (docs/inputs.md §6.7.2, Migration): the static GTFS zips the producers made
(producers/timetables/) give what the old paths gave.

    python3 tools/check/static-gtfs-same.py --zips DIR --mtr-pairs FILE --timetables DIR [--osm DIR --old-dem DIR]

1. The MTR: dem/railgtfs.py's count rows of the zips (no feeds), byte for byte the old pairs
   (`sources/rail/mtr-pairs`), which railfreq read as its last input as the counts are now.
2. The ferries' looked-up timetables: every record of the `timetables-*.json` files in --timetables,
   in their order, field for field the record the zips' count rows give (dem/staticgtfs.py), each
   value of the same JSON type; and the records the ferries read for each
   line (dem/ferries.py's choice, its headway and per_week readings) the same.
3. With --osm (the ferries job's exports of a pass's ferries set: ways.geojsonseq, relations.opl,
   terminals.geojsonseq) and --old-dem (dem/ as it was, with ferries.py reading freq/): ferries.py
   run both ways, without feeds, writes the same ferries.json and ferry-lines.json, byte for byte.

Exits 1 if anything differs.
"""
import glob, json, os, shutil, subprocess, sys, tempfile, zipfile

here = os.path.dirname(os.path.abspath(__file__))
dem = os.path.join(here, "..", "..", "dem")
sys.path.insert(0, dem)
import ferrygtfs  # noqa: E402
import staticgtfs  # noqa: E402


def arg(name, default=None):
    return sys.argv[sys.argv.index(name) + 1] if name in sys.argv else default


zips = sorted(glob.glob(os.path.join(arg("--zips"), "*.zip")))
work = tempfile.mkdtemp(prefix="static-gtfs-same-")
bad = 0

# 1. The MTR's pairs.
lst = os.path.join(work, "static.json")
with open(lst, "w") as f:
    json.dump({"zips": [{"name": os.path.basename(z), "path": os.path.abspath(z)} for z in zips]}, f)
with open(os.path.join(work, "feeds.json"), "w") as f:
    json.dump({"feeds": []}, f)
subprocess.run([sys.executable, "railgtfs.py", "--feeds", os.path.join(work, "feeds.json"), "--out", os.path.join(work, "pairs.bin"), "--used", os.path.join(work, "used.json"), "--static", lst, "--counts", os.path.join(work, "counts.bin")], cwd=dem, check=True, capture_output=True)
got, want = open(os.path.join(work, "counts.bin"), "rb").read(), open(arg("--mtr-pairs"), "rb").read()
trip_pairs = os.path.getsize(os.path.join(work, "pairs.bin"))
print(f"MTR: {len(got) // 21} pairs from the count rows, {len(want) // 21} before: {'byte for byte the same' if got == want else 'DIFFERENT'}; {trip_pairs // 21} from timed trains")
bad += got != want or trip_pairs != 0

# 2. The looked-up ferry records.
old = []
for p in sorted(glob.glob(os.path.join(arg("--timetables"), "timetables-*.json"))):
    old += json.load(open(p))
new = []
for z in zips:
    with zipfile.ZipFile(z) as zf:
        new += staticgtfs.ferry_counts(zf, ferrygtfs.is_ferry)
diff = [(a, b) for a, b in zip(old, new) if a != b]
same_types = all(a.keys() == b.keys() and all(type(a[k]) is type(b[k]) for k in a) for a, b in zip(old, new))
print(f"ferries: {len(old)} looked-up records, {len(new)} from the zips; {len(diff)} differ field for field; every value's type the same: {same_types}")
for a, b in diff[:5]:
    print("  was", a, "\n  now", b)
bad += len(old) != len(new) or bool(diff) or not same_types


def chosen(recs):
    """dem/ferries.py's reading of the records, every line taken as on the map."""
    import copy, re
    freq = {}
    for rec in copy.deepcopy(recs):
        if isinstance(rec.get("headway"), str):
            m = re.match(r"\s*(\d+(?:\.\d+)?)\s*(h|hour|hours|hr)?", rec["headway"])
            rec["headway"] = (float(m[1]) * (60 if m[2] else 1)) if m else None
        if rec.get("per_day") is None and rec.get("per_week") is not None:
            rec["per_day"] = round(rec["per_week"] / 7, 3)
        if rec.get("per_day") is None and rec.get("headway") is None:
            continue
        freq.setdefault(rec["line"], {k: rec[k] for k in ("per_day", "per_day_low", "headway", "days", "months", "overnight", "source", "url", "checked", "season") if k in rec})
    return freq


a, b = chosen(old), chosen(new)
print(f"ferries: {len(a)} lines' records as the ferries read them: {'the same' if a == b else 'DIFFERENT'}")
bad += a != b

# 3. ferries.py both ways.
if arg("--osm"):
    src_old, src_new = os.path.join(work, "src-old"), os.path.join(work, "src-new")
    for d in (src_old, src_new):
        os.makedirs(d)
        for f in ("ways.geojsonseq", "relations.opl", "terminals.geojsonseq"):
            shutil.copy(os.path.join(arg("--osm"), f), d)
    shutil.copytree(arg("--timetables"), os.path.join(src_old, "freq"), ignore=lambda d, fs: [f for f in fs if not f.startswith("timetables-")])
    runs = {"old": (arg("--old-dem"), src_old, []), "new": (dem, src_new, ["--static", lst])}
    outs = {}
    for k, (d, src, extra) in runs.items():
        o = os.path.join(work, f"out-{k}")
        subprocess.run([sys.executable, "ferries.py", "--src", src, "--out", o, *extra], cwd=d, check=True, capture_output=True)
        outs[k] = [open(os.path.join(o, f), "rb").read() for f in ("ferries.json", "ferry-lines.json")]
    same = outs["old"] == outs["new"]
    lines = json.loads(outs["new"][1])
    print(f"ferries.py: {len(lines)} lines, {sum('freq' in i for i in lines.values())} with a frequency: ferries.json and ferry-lines.json {'byte for byte the same' if same else 'DIFFERENT'}")
    bad += not same

shutil.rmtree(work)
sys.exit(1 if bad else 0)
