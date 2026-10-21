# Sizes

**Measured** (the 2026-09-28 pass): planet 95.1 GB; filtered 60.6 GB; sets 10.9 GB; outlines 2.7 GB;
z6 tile extracts ~58 GB; basemap 28.6 GB (Planetiler needs ~6× its 16.3 GB input while it runs); water 1.38 GB.

**Estimated:**

| | today's coverage | whole world |
|---|---|---|
| road values | ~8 GB | ~8 GB |
| base packs | ~30 GB | ~300 GB |
| our layers | ~150 GB | ~1.2 TB, with buildings |
| sources kept per pass | ~200 GB | the same |
| roadside buildings (once per release) | ~40 GB | the same |
| 3D buildings (`docs/buildings3d.md` §2.5): sources, normalized files, packs | 62 + 17–22 + 4.6–6.9 GB | not planned |
| depth (docs/steps.md §3): its sources' grids; its packs | a sample's: 15″ grid 6 MB over 9.5° × 6°; 56 US coastal tiles 0.6 GB of grids → a z6 tile's packs 168 MB (4,173 tiles); the depth lab's estimate, quantised: ~25 GB seas, 5–20 GB surveyed lakes | — |
| an app Mac's mirror | the World ~10.5 GB; a region tens of MB to ~20 GB | as downloaded |

The NAS holds about two passes' sources at most (a retired pass's go 14 days after the next
completes).

**Time, measured:** docs/phase-times.md (per step and per phase, from `scenic timings`).

A tail task on the other Mac fetches 0.65–1 GB at 2–6 MB/s (the Macs reach each other over Wi-Fi at
about 4 MB/s; the NAS at 25–55 MB/s), so a tail is worth offering only to a Mac that would otherwise
idle. `elev`'s memory is the point arrays (30 M vertices: ~2 GB) and the files' decoded blocks
(docs/inputs.md §6.8: one 512 MB cache).
