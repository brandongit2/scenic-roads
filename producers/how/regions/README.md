# Regions: how they're made

Written by hand: each region is the owner's choice of what to build, so there is nothing to remake
it from; the files are the definition (docs/inputs.md §6.1, docs/regions.md). A region names OSM
boundary relations or a shape (`<name>.geojson`), grouped by folder. The daily backup keeps them.

`old-2026-10/outlines/` holds the outlines the earlier regions were made from (Geofabrik's `.poly`
extracts and a few hand-made ones), kept for reference; no step reads them.
