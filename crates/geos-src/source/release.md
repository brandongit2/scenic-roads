2024-03-03

- Fixes/Improvements:
  - Fix ConcaveHullOfPolygons nested shell handling (GH-1169, Martin Davis)
  - Fix RelateNG for computing IM for empty-nonempty cases (Martin Davis)
  - Fix LineString->getPoint(n) for M geometries (GH-1191, @hsieyuan)
  - Fix TopologyPreservingSimplifier/TaggedLineString to avoid jumping components (JTS-1096, Martin Davis)
  - Fix WKTWriter for small precisions and with trim enabled (GH-1199, Mike Taves)
  - Fix BufferOp to increase length of segments removed by heuristic (GH-1200, Martin Davis)
  - Improve RelateNG performance for A/L cases in prepared predicates (GH-1201, Martin Davis)
  - Improve OffsetCurve to handle mitre joins for polygons (Martin Davis)
  - Fix inscribed circle initialization (GH-1225, Benoit Maurin)
  - Fix overlay heuristic for GeometryCollections with empty elements (GH-1229, Martin Davis)
  - Add ring buffer hole removal heuristic (GH-1233, Martin Davis)
  - Fix buffer element erosion for negative distance and remove overlay deps (GH-1239, Martin Davis)

