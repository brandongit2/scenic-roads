# Coverage and regions

**The idea in one line:** OpenStreetMap comes from one worldwide download, cut by area, and a region
is only an outline saying which areas to build. Every step runs per area, per pack near the
coverage, or once for the whole world, so region size, count and borders never show on the map.

**Regions** are files in `inputs/regions/` (docs/inputs.md §6.1): `osm:` relations, GeoJSON shapes
and other regions combined, each pinned to an OSM snapshot, checked by the input check before the
build uses them. A region may also have water (sea zones from Marine Regions' EEZ v12, lakes, the
high seas as the rest: docs/inputs.md §6.1.3). Overlaps are harmless to the build; nothing checks for
gaps unless `inputs/regions/rules.toml` says the regions cover the Earth once (`cover = "earth"`),
when the check fills and checks them. A region with no land (a sea zone, the high seas) is coverage
for the depth alone (`Coverage::depth_only`, its stored geometry's `water_only`): no unit, map tile,
terrain, slope, tree cover or buildings' tile is built for it. A region of land and water is coverage
like any other, its water included.

**Coverage** is the union of the regions.
- **What gets built:** a feature that touches the coverage; for a way, any node inside. Ways are kept
  whole, so roads end a little past the edge.
- **Units** are chosen by their reach, worked out once per pass from each piece's roads, rail and
  ferries (`sources/osm/<date>/reach`, `pipeline::reach`): a unit is built when the box of the ways
  it owns meets the coverage, or one of its own long ways touches it. The unit step then keeps
  exactly the ways touching the coverage.
- **Builds depend on coverage, never on regions.** Renaming, splitting or merging regions with the
  same outlines reruns nothing. A job is keyed on the coverage inside the box it reads
  (`Coverage::fingerprint`), so changing an outline reruns only what its changed part reaches.
- **Shrinking:** what only the removed part built leaves the manifest once the units are built (a
  prune): units no longer built, their candidates and peaks, map tiles no unit's ways reach. A z6
  tile the coverage has left loses its terrain, slope and tree cover hi packs and mids ("none"
  pieces), a z3 tile its lo packs ("none" assemblies). The next catalog drops them, and GC frees
  them.

**By location.** Rules that depend on where a thing is; the units' rules are versioned by area in
`pipeline::rules`, so a changed rule reruns only the units it applies to:
- elevation datasets and their blend (`pipeline::dem`, docs/inputs.md §6.8) and densification
  spacing (8 m where a declared dataset finer than 15 m has data, 15 m elsewhere:
  `pipeline::dem::spacing`, docs/inputs.md §6.8);
- heritage registers (`inputs/heritage/`, docs/inputs.md §6.5);
- timetables: the Mobility Database feeds of the countries the coverage is in, declared feeds and
  static timetables (docs/steps.md, Rail service; docs/inputs.md §6.7);
- road network codes (`extract`) and their colours (`web/src/mapschemes.ts`);
- leaf-type source: EEA in Europe, NALCMS in North America, none elsewhere (`dem/leaftype.py`);
- the languages spoken there (docs/names.md; docs/inputs.md §6.2);
- credits (`pipeline::rules::CREDITS`; a catalog lists those meeting its coverage + 20 km or its
  units' ways).

**No seams:** no step knows region borders; per-vertex values are computed once, by the owning unit;
road values come from the planet; queries join road parts by
offset. Where coverage ends, roads end.

**IDs:** OSM type and id. Way ids fit u32 until the 2040s; points use u64.
- Each z6 tile's hidata has a "ways here" index (`here`, `ends`): every way whose box meets the tile,
  with its owner unit and its index in that unit's base pack.
- Details and profiles ask with the id plus the location, which picks the tile or a neighbour.
