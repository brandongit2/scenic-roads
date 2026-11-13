# Source extents: how they're made

Where each data source has data and how good it is there (docs/inputs.md §6.11): a folder per
theme (`terrain/`), a declaration (`<source>.toml`) and its pieces (`<source>.geojson`) per source,
and the theme's `score.toml`.

A declaration is written by hand from the source's own pages (name, credit, licence, availability,
and for each piece block the service that says where its data is and which of its fields are the
metrics). Its pieces are made from it by `producers/extents/extents.py` in the repository, which
reads only the service it names:

    uv run --project producers python producers/extents/extents.py producers/extents/terrain/<source>.toml --out <folder> --cache <scratch>

It writes `<folder>/terrain/<source>.geojson` and a copy of the declaration, and says what it read
(requests, bytes, features), which each file also records (`read`). No more than `--max-gb` (5) is
read for one source. The declarations and their pieces are kept in the repository's
`producers/extents/terrain/`; add both, and `score.toml`, to `inputs/extents/terrain/`, where the
input check (the unit `extents`) takes them in. A declaration without its pieces yet is a warning
that holds it until accepted on the build page.

A source with a large index (3DEP's catalogue of 143,599 files, Quebec's 614,801 tiles, Ontario's
623,235, NRCan's 950 extent files, England's Environment Agency's, swissALTI3D's 88,775 items,
RGE ALTI's 290,932 source polygons) runs on the Mac mini, in a scratch folder of its own; its
`--cache` folder is kept between runs, so a run made again fetches nothing it has but an ArcGIS
layer's count of its features, asked afresh each run (13 Nov 2026: `~/scratch-t1350-cache` on the
mini).

`producers/extents/score.py <score.toml> --examples` prints what the score's weights do on a few
pairs of pieces; edit `score.toml` to tune them.

A new source of a kind the producer reads (an ArcGIS REST layer or ImageServer catalogue, a STAC or
OGC API collection, a WFS type answering GeoJSON or GML, an ATOM download feed, an index file:
GeoJSON, GML, KML, a GeoPackage or a zipped shapefile, a listing of georeferenced names: a bucket,
a page's links, a text file, a zip's members, a spreadsheet's rows, by coordinates, a lettered grid
reference or a nested sheet number, a file's own header or a coverage service's coarse answer) is a
new declaration, no code. A source that publishes no footprint is
a `claimed` piece: the administrative area the publisher states its data covers, read from Natural
Earth's 1:10m admin-1 file (public domain) with a `filter` (`{adm0_a3 = ["LCA"]}`, and `name` for
a part of a country). Where the publisher doesn't state what its data covers, or the smallest area
holding it would overstate it (a city's survey and its province), the piece is `none`: listed,
not drawn.
