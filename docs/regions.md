# Coverage and regions

**The idea in one line:** OpenStreetMap comes from one worldwide download, cut by z6 tile, and a region
is only an outline saying which z6 tiles to build. Every step runs per z6 tile (or z3 tile), per pack near the
coverage, or once for the whole world, so region size, count and borders never show on the map.

**Regions** are files in `inputs/regions/` (docs/inputs.md §6.1): `osm:` relations, GeoJSON shapes
and other regions combined, each pinned to an OSM snapshot, checked by the input check before the
build uses them. A region may also have water (sea zones from Marine Regions' EEZ v12, lakes, the
high seas as the rest: docs/inputs.md §6.1.3). Overlaps are harmless to the build; nothing checks for
gaps unless `inputs/regions/_rules.toml` says the regions cover the Earth once (`cover = "earth"`),
when the check fills and checks them. Water is no step's target but the depth's: a z6 tile, map
tile, terrain, slope, tree cover or buildings' tile is built only near land (`Coverage::meets_rect`).
A region with no land (a sea zone, the high seas) is coverage for the depth alone
(`Coverage::depth_only`). A region of land and water is coverage like any other, its water held
(its stored shape marked `water`, `coverage::Shape::water`): a bridge's, a pier's or a ferry's ways
at sea are still the region's, but a tile that only its water meets is the depth's alone.

A region marked `disabled = true` (docs/inputs.md §6.1.3) is checked and stored, and other regions
may name it, but it is not part of the coverage. That way a list can cover the Earth, as the cover
check requires, while only some of it is built.

**Coverage** is the union of the enabled regions.
- **What gets built:** a feature that touches the coverage; for a way, any node inside. Ways are kept
  whole, so roads end a little past the edge.
- **Z6 tiles** are chosen by their reach, worked out once per pass from each z6 tile extract's roads, rail and
  ferries (`sources/osm/<date>/reach`, `pipeline::reach`): a z6 tile is built when the box of the ways
  it owns meets the coverage, or one of its own long ways touches it. The `unit` step then keeps
  exactly the ways touching the coverage.
- **Builds depend on coverage, never on regions.** Renaming, splitting or merging regions with the
  same outlines reruns nothing. A job is keyed on the coverage inside the box it reads
  (`Coverage::fingerprint`), so changing an outline reruns only what its changed part reaches.
- **Shrinking:** what only the removed part built leaves the manifest once the z6 tiles are built (a
  prune): z6 tiles no longer built, their candidates and peaks, map tiles no z6 tile's ways reach. A z6
  tile the coverage has left loses its terrain, slope and tree cover hi packs and mids ("none"
  pieces), a z3 tile its lo packs ("none" assemblies). The next catalogue drops them, and GC frees
  them.

**By location.** Rules that depend on where a thing is; the z6 tiles' rules are versioned by area in
`pipeline::rules`, so a changed rule reruns only the z6 tiles it applies to:
- elevation datasets and their blend (`pipeline::dem`, docs/inputs.md §6.8) and densification
  spacing (8 m where a declared dataset finer than 15 m has data, 15 m elsewhere:
  `pipeline::dem::spacing`, docs/inputs.md §6.8);
- heritage registers (`inputs/heritage/`, docs/inputs.md §6.5);
- timetables: the Mobility Database feeds of the countries the coverage is in, declared feeds and
  static timetables (docs/steps.md, Rail service; docs/inputs.md §6.7);
- road network codes (`extract`) and their colours (`web/src/mapschemes.ts`);
- leaf-type source: EEA in Europe, NALCMS in North America, none elsewhere (`pipeline::leaftype`);
- the languages spoken there (docs/names.md; docs/inputs.md §6.2);
- credits (`pipeline::rules::CREDITS`; a catalogue lists those meeting its coverage + 20 km or its
  z6 tiles' ways).

**No seams:** no step knows region borders; per-vertex values are computed once, by the owning z6 tile;
road values come from the planet; queries join road parts by
offset. Where coverage ends, roads end.

**IDs:** OSM type and id. Way ids fit u32 until the 2040s; points use u64.
- Each z6 tile's hidata has a "ways here" index (`here`, `ends`): every way whose box meets the tile,
  with its owner z6 tile and its index in that z6 tile's base pack.
- Details and profiles ask with the id plus the location, which picks the tile or a neighbour.
