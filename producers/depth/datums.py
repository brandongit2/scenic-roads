"""Vertical datum surfaces for the depth producers: an offset between two water levels (mean sea
level and a chart's datum, a survey's datum and MSL), from tide stations, over the water.

The surface is harmonic between the stations (each water cell the mean of its water neighbours,
the stations fixed; land a barrier, so a narrow bay's tide doesn't leak across a peninsula: the
Atlantic coast of Nova Scotia, about 1 m, stays out of the Minas Basin, 7.5 m), solved on a
lon/lat grid over the water bodies that hold a station (a station counts for the water nearest it,
within 25 km), and the nearest solved value beyond. Water with no station within 25 km of a solved
cell is extrapolated when it is open sea (a body touching the grid's edge or over 100 km²: the
nearest solved cell's value, or with no station within 25 km of any water the nearest station's,
however far; the farthest distance logged) and gets no depth otherwise (NaN: nodata, an enclosed
lagoon or lake), logged in one line; more than 5% of the water left without a value, or one body
over 2%, stops the run (a missing area of the table). No station near the water and no open sea
also stops. Extrapolated cells are not flagged in the output (one band); the log says how many. A
station table is a CSV with `lon`, `lat` and the value's column.
"""
from __future__ import annotations

import csv
from pathlib import Path

import numpy as np

from grid import Window, log, warp_onto


def stations(path: Path, column: str, skip=frozenset(), id_column: str | None = None) -> list[tuple[float, float, float]]:
    """(lon, lat, value) of each row with a value (rows whose `id_column` is in `skip` left out)."""
    out = []
    with open(path, newline="") as f:
        for r in csv.DictReader(f):
            if id_column and r.get(id_column) in skip:
                continue
            v = (r.get(column) or "").strip()
            if v:
                out.append((float(r["lon"]), float(r["lat"]), float(v)))
    return out


# A station farther than this (km) from its water, or water farther than this from every solved
# cell, isn't used: datum offsets change by decimetres over tens of km.
REACH_KM = 25.0
KM_DEG = 111.32


def nearest_water(water: np.ndarray, lon: float, lat: float, west: float, north: float, res: float):
    """The (row, col) of the water cell nearest the point, within REACH_KM, or None."""
    H, W = water.shape
    fr, fc = (north - lat) / res - 0.5, (lon - west) / res - 0.5
    ky = KM_DEG * res
    kx = ky * max(np.cos(np.radians(lat)), 0.05)
    ry, rx = int(np.ceil(REACH_KM / ky)) + 1, int(np.ceil(REACH_KM / kx)) + 1
    r0, r1 = max(0, int(np.floor(fr)) - ry), min(H, int(np.ceil(fr)) + ry + 1)
    c0, c1 = max(0, int(np.floor(fc)) - rx), min(W, int(np.ceil(fc)) + rx + 1)
    if r1 <= r0 or c1 <= c0:
        return None
    rr, cc = np.nonzero(water[r0:r1, c0:c1])
    if not len(rr):
        return None
    d = np.hypot((rr + r0 - fr) * ky, (cc + c0 - fc) * kx)
    k = int(np.argmin(d))
    return (int(rr[k]) + r0, int(cc[k]) + c0) if d[k] <= REACH_KM else None


def nearest_station(water: np.ndarray, pts, west: float, north: float, res: float):
    """(km, (lon, lat)) of the station nearest any water cell (for the message of a table without one)."""
    from scipy.spatial import cKDTree

    rr, cc = np.nonzero(water)
    lat0 = north - (rr.mean() + 0.5) * res
    kx = np.cos(np.radians(lat0))
    tree = cKDTree(np.column_stack([(west + (cc + 0.5) * res) * kx, north - (rr + 0.5) * res]))
    d, _ = tree.query([[p[0] * kx, p[1]] for p in pts])
    k = int(np.argmin(d))
    return float(d[k]) * KM_DEG, (pts[k][0], pts[k][1])


# Water beyond the reach that is open sea, not a lagoon: a body touching the grid's edge (the sea
# going on past the chunk; water_of pads the grid by 4 cells, so within 5) or larger than OPEN_KM2.
# NONNA and the other sea grids cover the sea only, so a body that big cut off from the solved
# water is sea split by a gap in the coverage (ice, an unsurveyed strait), not a lake; the sea's
# datum offsets change by decimetres over hundreds of km, while an enclosed lagoon's can differ.
OPEN_KM2 = 100.0
EDGE_CELLS = 5


def open_sea(lab: np.ndarray, bodies: np.ndarray, north: float, res: float) -> np.ndarray:
    """Which of `bodies` (labels in `lab`) are open sea (see OPEN_KM2)."""
    from scipy.ndimage import sum as nsum

    H, W = lab.shape
    lat = north - (np.arange(H) + 0.5) * res
    area = np.broadcast_to(((KM_DEG * res) ** 2 * np.cos(np.radians(lat)))[:, None], (H, W))
    km2 = np.atleast_1d(nsum(area, lab, bodies))
    e = EDGE_CELLS
    edge = np.unique(np.concatenate([lab[:e].ravel(), lab[H - e:].ravel(), lab[:, :e].ravel(), lab[:, W - e:].ravel()]))
    return (km2 > OPEN_KM2) | np.isin(bodies, edge)


def check_far(lab: np.ndarray, far: np.ndarray, dist, total: int | None = None) -> None:
    """Log the water left without a datum (`far`) and stop when it is over 5% of the water or one
    body over 2%: a missing area of the table."""
    if not far.any():
        return
    total = total or int((lab > 0).sum())
    ids, size = np.unique(lab[far], return_counts=True)
    how = f", the farthest {dist[far].max():.0f} km from a solved cell" if dist is not None else ""
    log(f"datum surface: {len(ids)} water bodies, {int(size.sum())} cells, with no station within {REACH_KM:g} km (the largest {int(size.max())} cells{how}) get no depth")
    if size.sum() > 0.05 * total or size.max() > 0.02 * total:
        raise SystemExit(f"{int(size.sum())} of {total} water cells (the largest body {int(size.max())}) have no station within {REACH_KM:g} km: "
                         "that is a missing area of the table, not a lake; the table needs rows for it")


class Surface:
    """An offset (m) on a lon/lat grid (`a`, its west, north and cell size)."""

    def __init__(self, a: np.ndarray, west: float, north: float, res: float):
        self.a, self.west, self.north, self.res = a.astype("float32"), west, north, res

    @classmethod
    def constant(cls, v: float) -> "Surface":
        return cls(np.full((2, 2), v, "float32"), -180.0, 90.0, 180.0)

    @classmethod
    def harmonic(cls, pts, water: np.ndarray, west: float, north: float, res: float) -> "Surface":
        """Solved over `water` (a bool lon/lat grid from `west`, `north`, cells of `res` degrees)."""
        from scipy.ndimage import distance_transform_edt, label
        from scipy.sparse import coo_matrix
        from scipy.sparse.linalg import spsolve

        H, W = water.shape
        if not water.any() or not pts:
            raise SystemExit("a datum surface needs water and stations")
        fixed: dict = {}
        for lon, lat, v in pts:
            hit = nearest_water(water, lon, lat, west, north, res)
            if hit:
                fixed.setdefault(hit, []).append(v)
        lab, _ = label(water)
        lat0 = north - H * res / 2
        if not fixed:
            # no station within reach of any water: open sea takes the nearest station's value
            bodies = np.unique(lab[water])
            opn = bodies[open_sea(lab, bodies, north, res)]
            if not len(opn):
                d, at = nearest_station(water, pts, west, north, res)
                raise SystemExit(
                    f"a datum surface needs a station within {REACH_KM:g} km of the water: none is, over "
                    f"{west:.2f}..{west + W * res:.2f} E, {north - H * res:.2f}..{north:.2f} N (the nearest is {d:.0f} km from its nearest water cell, "
                    f"at {at[0]:.2f}, {at[1]:.2f}); a far station's offset would be a wrong depth, so the table needs rows for this area")
            from scipy.spatial import cKDTree

            ext = np.isin(lab, opn)
            check_far(lab, water & ~ext, dist=None)
            kx = np.cos(np.radians(lat0))
            rr, cc = np.nonzero(ext)
            d, k = cKDTree([(p[0] * kx, p[1]) for p in pts]).query(np.column_stack([(west + (cc + 0.5) * res) * kx, north - (rr + 0.5) * res]))
            out = np.full((H, W), np.nan, "float32")
            out[rr, cc] = np.array([p[2] for p in pts])[k]
            log(f"datum surface: no station within {REACH_KM:g} km; {len(rr)} open-sea cells extrapolated from the nearest station (the farthest {d.max() * KM_DEG:.0f} km)")
            return cls(out, west, north, res)
        solve = water & np.isin(lab, list({lab[k] for k in fixed}))
        # a water body without a station takes the nearest solved value, if one is within reach
        _, (iy2, ix2) = distance_transform_edt(~solve, return_indices=True, sampling=(KM_DEG * res, KM_DEG * res * np.cos(np.radians(lat0))))
        dist = np.hypot((np.arange(H)[:, None] - iy2) * KM_DEG * res, (np.arange(W)[None, :] - ix2) * KM_DEG * res * np.cos(np.radians(lat0)))
        bodies = np.unique(lab[water & ~solve])
        far = np.zeros((H, W), bool)
        if len(bodies):
            from scipy.ndimage import minimum

            dmin = np.atleast_1d(minimum(dist, lab, bodies))
            gone = bodies[dmin > REACH_KM]
            if len(gone):
                opn = gone[open_sea(lab, gone, north, res)]
                ext = np.isin(lab, opn)
                if ext.any():
                    log(f"datum surface: {len(opn)} open-sea water bodies, {int(ext.sum())} cells, beyond {REACH_KM:g} km extrapolated from the nearest solved cell "
                        f"(the farthest {dist[ext].max():.0f} km)")
                far = np.isin(lab, gone) & ~ext
                check_far(lab, far, dist, total=int(water.sum()))
        idx = -np.ones((H, W), np.int64)
        cells = np.argwhere(solve)
        idx[solve] = np.arange(len(cells))
        rows, cols, vals = [], [], []
        b = np.zeros(len(cells))
        for k, (r, c) in enumerate(cells):
            if (r, c) in fixed:
                rows.append(k), cols.append(k), vals.append(1.0)
                b[k] = float(np.mean(fixed[(r, c)]))
                continue
            nb = [(r + dr, c + dc) for dr, dc in ((1, 0), (-1, 0), (0, 1), (0, -1)) if 0 <= r + dr < H and 0 <= c + dc < W and solve[r + dr, c + dc]]
            rows.append(k), cols.append(k), vals.append(float(len(nb) or 1))
            for rr, cc in nb:
                rows.append(k), cols.append(int(idx[rr, cc])), vals.append(-1.0)
        A = coo_matrix((vals, (rows, cols)), shape=(len(cells), len(cells))).tocsr()
        x = spsolve(A, b)
        out = np.full((H, W), np.nan, "float32")
        out[solve] = x
        log(f"datum surface: {len(pts)} stations on {len(fixed)} cells, {len(cells)} water cells, {x.min():.2f}–{x.max():.2f} m")
        out = out[iy2, ix2]
        out[far] = np.nan
        return cls(out, west, north, res)

    def on(self, win: Window) -> np.ndarray:
        """The surface on a depth grid's window (bilinear)."""
        from rasterio.transform import from_origin

        return warp_onto((self.a, from_origin(self.west, self.north, self.res, self.res), "EPSG:4326"), 1, win, "bilinear", src_nodata=np.nan)


def from_table(table, rasters, res: float = 0.005) -> Surface | None:
    """A station table's surface (`table`: path, column, ids left out, the id column) over the
    water the `rasters` (a source's files, any CRS) have data on: a constant when every station
    says the same; None without a table."""
    if table is None:
        return None
    path, column, skip, id_column = table
    pts = stations(Path(path), column, skip, id_column)
    if not pts:
        raise SystemExit(f"{path}: no station has {column}")
    if len({round(p[2], 3) for p in pts}) == 1:
        return Surface.constant(pts[0][2])
    water, w, n, r = water_of(rasters, res)
    return Surface.harmonic(pts, water, w, n, r)


def water_of(rasters, res: float) -> tuple[np.ndarray, float, float, float]:
    """A lon/lat grid (cells of `res`°) of where any of `rasters` has data, over their bounds."""
    import rasterio
    from rasterio.enums import Resampling
    from rasterio.transform import from_origin
    from rasterio.warp import reproject, transform_bounds

    boxes = []
    for p in rasters:
        with rasterio.open(p) as d:
            boxes.append(transform_bounds(d.crs, "EPSG:4326", *d.bounds))
    w, s = min(b[0] for b in boxes) - 4 * res, min(b[1] for b in boxes) - 4 * res
    e, n = max(b[2] for b in boxes) + 4 * res, max(b[3] for b in boxes) + 4 * res
    W, H = int(np.ceil((e - w) / res)), int(np.ceil((n - s) / res))
    out = np.zeros((H, W), bool)
    for p in rasters:
        with rasterio.open(p) as d:
            a = np.full((H, W), np.nan, "float32")
            reproject(rasterio.band(d, 1), a, dst_transform=from_origin(w, n, res, res), dst_crs="EPSG:4326", dst_nodata=np.nan, resampling=Resampling.max)
            out |= np.isfinite(a)
    return out, w, n, res


def water_from(win: Window, *depths: np.ndarray, res: float = 0.01) -> tuple[np.ndarray, float, float, float]:
    """A lon/lat water grid (cells of `res`°) over the window: where any of `depths` (on the window)
    has data."""
    from rasterio.transform import from_origin
    from rasterio.warp import Resampling, reproject

    w, s, e, n = win.bounds_lonlat()
    W, H = max(2, int(np.ceil((e - w) / res))), max(2, int(np.ceil((n - s) / res)))
    out = np.zeros((H, W), bool)
    for d in depths:
        a = np.full((H, W), np.nan, "float32")
        reproject(np.where(np.isfinite(d), 1.0, np.nan).astype("float32"), a, src_transform=win.transform(), src_crs="EPSG:3857", src_nodata=np.nan,
                  dst_transform=from_origin(w, n, res, res), dst_crs="EPSG:4326", dst_nodata=np.nan, resampling=Resampling.max)
        out |= np.isfinite(a)
    return out, w, n, res
