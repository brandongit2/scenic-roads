"""The static timetables' counters (dem/staticgtfs.py, the static paths of dem/railgtfs.py and
dem/ferrygtfs.py, docs/inputs.md §6.7.2) on files made here, in a temporary folder.

    python3 tools/check/static_gtfs_py.py
"""
import json, os, shutil, struct, subprocess, sys, tempfile, unittest, zipfile
from datetime import date

here = os.path.dirname(os.path.abspath(__file__))
dem = os.path.join(here, "..", "..", "dem")
sys.path.insert(0, dem)
import ferrygtfs  # noqa: E402
import staticgtfs  # noqa: E402

CAL = "service_id,monday,tuesday,wednesday,thursday,friday,saturday,sunday,start_date,end_date\n"


def put(path, v):
    with open(path, "w") as f:
        json.dump(v, f)


def get(path):
    with open(path, "rb") as f:
        return f.read()


def make(path, tables):
    base = {
        "agency.txt": "agency_id,agency_name,agency_url,agency_timezone\na,A,,\n",
        "feed_info.txt": "feed_publisher_name\nus\n",
        "stops.txt": "stop_id,stop_name,stop_lat,stop_lon\n",
        "trips.txt": "route_id,service_id,trip_id\n",
        "stop_times.txt": "trip_id,arrival_time,departure_time,stop_id,stop_sequence\n",
        "calendar.txt": CAL,
    }
    base.update(tables)
    with zipfile.ZipFile(path, "w") as z:
        for n, t in base.items():
            z.writestr(n, t)


class Static(unittest.TestCase):
    def setUp(self):
        self.d = tempfile.mkdtemp()

    def tearDown(self):
        shutil.rmtree(self.d)

    def test_calendar_months_and_days(self):
        p = os.path.join(self.d, "c.zip")
        make(p, {"calendar.txt": CAL + "summer,1,1,1,1,1,1,1,20000501,20001031\nwinter,1,0,0,0,0,0,0,19991101,19990331\nleap,1,1,1,1,1,1,1,20240229,20240229\n"})
        with zipfile.ZipFile(p) as z:
            d = staticgtfs.service_dates(z)
        self.assertEqual(min(d["summer"]), date(staticgtfs.REF_YEAR, 5, 1))
        self.assertEqual(max(d["summer"]), date(staticgtfs.REF_YEAR, 10, 31))
        self.assertEqual({x.month for x in d["winter"]}, {11, 12, 1, 2, 3})
        self.assertTrue(all(x.weekday() == 0 for x in d["winter"]))
        self.assertEqual(d["leap"], {date(staticgtfs.REF_YEAR, 2, 28)})

    def test_count_values_as_written(self):
        v = staticgtfs.count_values({"per_day": "10", "per_week": "5.5", "per_day_low": "null", "headway": "about every 20 minutes", "overnight": "0", "note": " as given "})
        self.assertEqual(v, {"per_day": 10, "per_week": 5.5, "per_day_low": None, "headway": "about every 20 minutes", "overnight": False, "note": " as given "})
        self.assertIs(type(v["per_day"]), int)

    def test_rail_trains_and_counts(self):
        p = os.path.join(self.d, "r.zip")
        make(p, {
            "stops.txt": "stop_id,stop_name,stop_lat,stop_lon\na,A,46.5,7.5\nb,B,46.6,7.6\nc,C,46.7,7.7\n",
            "routes.txt": "route_id,agency_id,route_long_name,route_type,route_url\nv,a,Valley,2,https://e.org\nh,a,Hill,1,https://e.org\n",
            "trips.txt": "route_id,service_id,trip_id\nv,summer,t1\n",
            "stop_times.txt": "trip_id,arrival_time,departure_time,stop_id,stop_sequence\nt1,06:00:00,06:00:00,a,1\nt1,06:20:00,06:20:00,b,2\n",
            "frequencies.txt": "trip_id,start_time,end_time,headway_secs\nt1,06:00:00,23:00:00,600\n",
            "calendar.txt": CAL + "summer,1,1,1,1,1,1,1,20000501,20001031\n",
            "counts.txt": "route_id,from_stop_id,to_stop_id,at_least,per_day,per_week\nh,b,c,1,40,\nh,c,a,,,14\n",
        })
        lst, feeds = os.path.join(self.d, "s.json"), os.path.join(self.d, "f.json")
        put(lst, {"zips": [{"name": "r.zip", "path": p}]})
        put(feeds, {"feeds": []})
        out, counts, used = (os.path.join(self.d, n) for n in ("p.bin", "c.bin", "u.json"))
        subprocess.run([sys.executable, "railgtfs.py", "--feeds", feeds, "--out", out, "--used", used, "--static", lst, "--counts", counts], cwd=dem, check=True, capture_output=True)
        recs = lambda b: [struct.unpack("<ffffBf", b[i:i + 21]) for i in range(0, len(b), 21)]
        trains = recs(get(out))
        self.assertEqual(len(trains), 1)
        self.assertEqual((trains[0][4], trains[0][5]), (2, 102.0))  # 06:00 to 22:50 every 10 min
        c = recs(get(counts))
        f32 = lambda v: struct.unpack("<f", struct.pack("<f", v))[0]
        self.assertEqual(c[0], (f32(7.6), f32(46.6), f32(7.7), f32(46.7), 0x81, 40.0))
        self.assertEqual(c[1], (f32(7.7), f32(46.7), f32(7.6), f32(46.6), 0x81, 40.0))
        self.assertEqual((c[2][4], c[2][5]), (1, 2.0))
        self.assertEqual(len(c), 4)
        u = json.loads(get(used))
        self.assertEqual((u[0]["static"], u[0]["status"], u[0]["day"][:2] in ("05", "06", "07", "08", "09", "10"), u[0]["count_rows"]), ("r.zip", "ok", True, 2))

    def test_ferry_timed_seasonal_and_pinned(self):
        p = os.path.join(self.d, "f.zip")
        make(p, {
            "stops.txt": "stop_id,stop_name,stop_lat,stop_lon\np,P,60.0,10.0\nq,Q,60.1,10.2\nx,X,61.0,11.0\ny,Y,61.0,11.3\n",
            "routes.txt": "route_id,agency_id,route_long_name,route_type,route_url,checked,osm\nlake,a,Lake ferry,4,https://e.org/lake,2026-09,\nfar,a,Far ferry,4,https://e.org/far,2026-08,w9\n",
            "trips.txt": "route_id,service_id,trip_id\nlake,summer,out\nlake,summer,back\nfar,daily,f1\n",
            "stop_times.txt": "trip_id,arrival_time,departure_time,stop_id,stop_sequence\nout,08:00:00,08:00:00,p,1\nout,09:00:00,09:00:00,q,2\nback,10:00:00,10:00:00,q,1\nback,11:00:00,11:00:00,p,2\nf1,08:00:00,08:00:00,x,1\nf1,09:00:00,09:00:00,y,2\n",
            "frequencies.txt": "trip_id,start_time,end_time,headway_secs\nout,08:00:00,18:00:00,3600\n",
            "calendar.txt": CAL + "summer,1,1,1,1,1,1,1,20000501,20000930\ndaily,1,1,1,1,1,1,1,20000101,20001231\n",
        })
        # A line whose ends are the lake's ports; one far from the pinned route's stops.
        lines = {"w1": {"ends": [[10.0, 60.0], [10.2, 60.1]]}, "w9": {"ends": [[11.0, 61.05], [11.3, 61.05]]}, "w2": {"ends": [[11.0, 61.0], [11.3, 61.0]]}}
        recs = {r["line"]: r for r in ferrygtfs.process({"id": "f.zip", "url": ""}, p, lines, None, static=True)}
        lake = recs["w1"]
        self.assertEqual((lake["kind"], lake["per_day"], lake["months"], lake["season"], lake["url"], lake["checked"], lake["source"]), ("timetable", 5.5, "May–Sep", "seasonal", "https://e.org/lake", "2026-09", "Lake ferry"))
        self.assertEqual((recs["w9"]["per_day"], recs["w9"]["season"]), (1, "year"))
        self.assertNotIn("w2", recs, "a pinned route counts for its line alone")


if __name__ == "__main__":
    unittest.main()
