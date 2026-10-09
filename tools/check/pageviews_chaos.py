"""Room-making mid-job for the Python steps (docs/plan.md §8, dem/cachefile.py): pageviews.py
looks up the months' indexes again and again (each copied here from the NAS when it isn't here),
while every cache file no job holds is deleted as fast as it can be in another process; every
lookup gives what the first did. (Run by crates/pipeline's cache chaos test, or alone:
`python3 -I tools/check/pageviews_chaos.py <scratch dir>`.)"""
from __future__ import annotations

import json
import multiprocessing
import os
import sys
from pathlib import Path

DEM = Path(__file__).resolve().parent.parent.parent / "dem"
sys.path.insert(0, str(DEM))


def offline() -> None:
    """No network for this process and its children, whatever path asks: the internet's sockets
    refuse here, and a program run (curl) is sent to a proxy that refuses (port 9, nothing there)."""
    import socket

    def refuse(*_a, **_k):
        raise OSError("no network here (pageviews_chaos.py)")

    real = socket.socket.connect

    def connect(self, address):
        if self.family in (socket.AF_INET, socket.AF_INET6):
            refuse()
        return real(self, address)

    socket.socket.connect = connect
    socket.getaddrinfo = refuse
    socket.create_connection = refuse
    for k in ("http_proxy", "https_proxy", "all_proxy", "HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY"):
        os.environ[k] = "http://127.0.0.1:9"
    os.environ.pop("no_proxy", None)
    os.environ.pop("NO_PROXY", None)
    os.environ["SCENIC_FETCH_OFFLINE"] = "1"


offline()


def deleter(months: str, stop) -> None:
    import cachefile

    gone = 0
    while not stop.is_set():
        try:
            names = os.listdir(months)
        except FileNotFoundError:
            continue
        for n in names:
            gone += cachefile.try_remove(Path(months) / n)
    print(json.dumps({"deleted": gone}), file=sys.stderr, flush=True)


def main() -> None:
    from compression import zstd

    work = Path(sys.argv[1])
    nas, out = work / "nas", work / "out"
    nas.mkdir(parents=True, exist_ok=True)
    os.environ["SCENIC_PAGEVIEWS_STORE"] = str(nas)
    import cachefile
    import pageviews

    pageviews.OUT = out

    # (Nothing from the internet here, `offline`; and a month whose index can't be had from the NAS
    # fails the check at once, said so.)
    def streams(*_a, **_k):
        raise AssertionError("pageviews.py would stream a month's dump: its index wasn't had from the NAS")

    pageviews._stream_index = streams
    months = ["2025-11", "2026-02", "2026-05"]
    langs = ",".join(sorted(pageviews.LANGS))
    for i, m in enumerate(months):
        with zstd.open(nas / f"{m}.tsv.zst", "wt", encoding="utf-8") as f:
            f.write(f"#langs\t{langs}\n")
            for k in range(20000):
                f.write(f"en|Page_{k}\t{(k * (i + 3)) % 997}\n")
    wanted = {f"en|Page_{k}" for k in range(0, 20000, 7)}

    def look() -> list[dict[str, int]]:
        got = [pageviews.month_views(m, wanted) for m in months]
        # (The job's holds let go, as a job's end would: the deleter may take them now.)
        for m in months:
            cachefile.release(out / "months" / f"{m}.tsv.zst")
        return got

    calm = look()
    stop = multiprocessing.Event()
    p = multiprocessing.Process(target=deleter, args=(str(out / "months"), stop))
    p.start()
    try:
        for _ in range(40):
            assert look() == calm, "a lookup with the deleter under way differs"
    finally:
        stop.set()
        p.join()
    print("pageviews chaos: 40 lookups alike", file=sys.stderr)


if __name__ == "__main__":
    main()
