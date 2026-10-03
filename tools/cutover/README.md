# Cutover recipes (docs/plan.md §10, phase 6)

Today's 34 regions as recipes: the Geofabrik outlines the legacy builds used
(`inputs/outlines/geofabrik/*.poly` on the NAS), and OSM relations, from the pass of 2026-09-28's
outlines, for the three without one: Gibraltar (1278736), Saint-Pierre-et-Miquelon (3406826),
Singapore (536780). Their coverage meets 482 z6 areas.

Installing them (`inputs/regions/` on the NAS) starts the agent building today's regions the new
way after the newest OSM pass. With `inputs/hold-catalog` present the agent writes the resulting
catalog to `catalog-held/` instead of publishing it, for the comparison before switching.
