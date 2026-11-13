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

`producers/extents/score.py <score.toml> --examples` prints what the score's weights do on a few
pairs of pieces; edit `score.toml` to tune them.

A new source of a kind the producer reads (an ArcGIS REST layer, a STAC or OGC API collection, a
WFS type, an index file, a listing of georeferenced names, a file's own header) is a new
declaration, no code.
