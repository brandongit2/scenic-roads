# Scenic Roads — data pipeline and app.
#
#   make data   build everything for the regions in regions.json: OSM extracts → roads →
#               elevations → terrain → land cover & tree canopy → viewsheds → designations →
#               tiles → basemap. Each step reuses what it did before (see README, Adding a
#               region); REFRESH=1 also fetches newer OSM extracts for the existing regions.
#   make heritage   re-run only the designations (official registers) and road flags
#   make run    build the app and serve it at http://localhost:8080
#   make dev    backend + Vite dev server (http://localhost:5173)
#
# Steps are skipped when their inputs are unchanged; OSM extracts are only re-downloaded when
# Geofabrik has a newer one, and elevations are reused for roads whose geometry is unchanged.
# Outputs are swapped in atomically: restart `make run` to serve freshly built data.

SHELL := /bin/bash
DATA  := data
OSM   := $(DATA)/osm
BUILD := $(DATA)/build
PORT  ?= 8080
SPACING_M ?= 8
# Regions: regions.json (Geofabrik extracts, or Overpass areas where Geofabrik has none).
IDS := $(shell python3 -c "import json;print(' '.join(r['id'] for r in json.load(open('regions.json'))['regions']))")
RUST_SRC := $(shell find crates -name "*.rs") Cargo.toml $(wildcard crates/*/Cargo.toml)
WEB_SRC  := $(shell find web/src -type f) web/index.html web/package.json web/vite.config.ts

HER   := $(DATA)/heritage
FER   := $(DATA)/ferries
UV    := cd dem && uv run python

.PHONY: all data osm fonts web run dev clean-build heritage ferries
all: data web

data: $(BUILD)/roads.tiles $(BUILD)/slope.tiles basemap-parts $(BUILD)/ferries.json $(BUILD)/rail-freq.bin $(BUILD)/trees-cover.tiles details fonts

# Conditional download: curl -z only fetches when the server copy is newer than ours.
# OSM: data/osm/merged.osm.pbf holds every region; a region added to regions.json is downloaded
# and merged in (dem/osmupdate.py), and extracts are dropped once merged and in the basemap.
# `make osm-refresh` downloads newer extracts of every region and rebuilds the merged file.
.PHONY: osm osm-refresh
osm: $(OSM)/merged.osm.pbf
$(OSM)/merged.osm.pbf: regions.json
	$(UV) osmupdate.py
osm-refresh:
	$(UV) osmupdate.py --refresh

# Region outlines (trees, leaf type): Geofabrik .poly files, fetched once.
$(DATA)/trees/poly/.done: regions.json dem/regionpolys.py
	$(UV) regionpolys.py && touch $(CURDIR)/$@

# Binaries are order-only prerequisites of the data steps: editing code doesn't rebuild data
# (delete an output to redo its step).
target/release/extract target/release/tile target/release/server target/release/terrain target/release/scenic target/release/slope target/release/railfreq target/release/peaks: $(RUST_SRC)
	cargo build --release && touch target/release/{extract,tile,server,terrain,scenic,slope,railfreq,peaks}

# 1. car-accessible roads + ferries, densified
$(BUILD)/ways.bin: $(OSM)/merged.osm.pbf | target/release/extract
	@mkdir -p $(BUILD)
	./target/release/extract $(BUILD) $(SPACING_M) $<

# 2. national DEMs (HRDEM lidar → USGS 3DEP → MRDEM), cached per vertex
$(BUILD)/elev.f32: $(BUILD)/ways.bin dem/sample.py
	cd dem && uv run python sample.py ../$(BUILD)

# 3. terrain tiles (3D mesh, hillshade, contours) and the z11 analysis grid
$(BUILD)/terrain.tiles: $(BUILD)/ways.bin | target/release/terrain
	./target/release/terrain $(BUILD)

# 3b. terrain slope pyramid (z12 slope; coarser levels sample the slopes beneath)
$(BUILD)/slope.tiles: $(BUILD)/terrain.tiles | target/release/slope
	./target/release/slope $(BUILD)

# 4. land cover (ESA WorldCover) on the analysis grid
$(BUILD)/grid.class.u8: $(BUILD)/terrain.tiles dem/landcover.py
	$(UV) landcover.py ../$(BUILD)

# 5. scenic analysis: 100 m road samples → tree canopy (Meta/WRI) near-field horizons →
#    15 km viewsheds and landscape metrics. The samples need the processed elevations (the tile
#    step's clean-up, run on its own first).
$(BUILD)/final.i16: $(BUILD)/elev.f32 | target/release/tile
	./target/release/tile $(BUILD) elev
$(BUILD)/samples.bin: $(BUILD)/final.i16 $(BUILD)/terrain.tiles | target/release/scenic
	./target/release/scenic $(BUILD) prep
$(BUILD)/roadside.u8: $(BUILD)/samples.bin
	./target/release/scenic $(BUILD) canopy
$(BUILD)/samples.metrics.u8: $(BUILD)/roadside.u8 $(BUILD)/grid.class.u8
	./target/release/scenic $(BUILD) view

# 6. designations from the official registers (see README), rasterised areas, road flags
$(DATA)/areas/areas.geojsonseq: $(OSM)/merged.osm.pbf
	@mkdir -p $(DATA)/areas
	osmium tags-filter $< wr/boundary=national_park,protected_area,aboriginal_lands wr/leisure=nature_reserve \
	  -o $(DATA)/areas/areas.osm.pbf --overwrite
	osmium export $(DATA)/areas/areas.osm.pbf -f geojsonseq --geometry-types=polygon -a type,id -o $@ --overwrite
$(HER)/osm/named.geojsonseq: $(OSM)/merged.osm.pbf
	@mkdir -p $(HER)/osm
	osmium tags-filter $< r/admin_level=4 -o $(HER)/osm/prov.osm.pbf --overwrite
	osmium export $(HER)/osm/prov.osm.pbf -f geojsonseq --geometry-types=polygon -o $(HER)/osm/prov.geojsonseq --overwrite
	osmium tags-filter $< nwr/historic nwr/heritage nwr/tourism=museum,attraction,viewpoint nwr/man_made=lighthouse \
	  nwr/railway=station nwr/building=train_station,church,cathedral nwr/amenity=place_of_worship \
	  nwr/boundary=protected_area,national_park nwr/leisure=park nwr/military -o $(HER)/osm/named.osm.pbf --overwrite
	osmium export $(HER)/osm/named.osm.pbf -f geojsonseq -o $@ --overwrite
$(HER)/federal.json: $(HER)/fhd.xlsx $(HER)/osm/named.geojsonseq dem/federal.py
	$(UV) federal.py
$(HER)/crhp.json: dem/crhp.py
	$(UV) crhp.py
$(BUILD)/grid.areas.u8: $(HER)/federal.json $(HER)/crhp.json $(DATA)/areas/areas.geojsonseq $(BUILD)/terrain.tiles dem/heritage.py \
                        $(wildcard $(HER)/*.json $(HER)/qc/* $(HER)/on/* $(HER)/ns/* $(HER)/nb/*)
	$(UV) heritage.py ../$(BUILD)
# Roadside buildings: Overture footprints' bounding boxes, streamed from its public release (~2.5 h).
# Only building files not yet fetched are streamed (a new bbox in regions.json "buildings").
$(DATA)/buildings/.done: regions.json
	$(UV) buildings.py && touch $(CURDIR)/$@
$(BUILD)/samples.bld.u8: $(BUILD)/samples.metrics.u8 $(DATA)/buildings/.done
	./target/release/scenic $(BUILD) buildings $(DATA)/buildings
$(BUILD)/scenic.u8: $(BUILD)/samples.metrics.u8 $(BUILD)/samples.bld.u8 $(BUILD)/grid.areas.u8
	./target/release/scenic $(BUILD) flags
heritage:
	$(UV) heritage.py ../$(BUILD) && ./target/release/scenic $(BUILD) flags && ./target/release/tile $(BUILD) 4 14

# 7. elevation clean-up, climbs, tile pyramid (with drape heights and scenic channels)
$(BUILD)/roads.tiles: $(BUILD)/elev.f32 $(BUILD)/scenic.u8 | target/release/tile
	./target/release/tile $(BUILD) 4 14

# Tree cover layer: leaf-type squares, then cover / height / leaf tiles (see README).
$(DATA)/trees/leaf/.done: $(DATA)/trees/poly/.done
	$(UV) leaftype.py && touch $(CURDIR)/$@
$(BUILD)/trees-cover.tiles: $(DATA)/trees/leaf/.done dem/trees.py regions.json
	$(UV) trees.py ../$(BUILD)

# Ferries: OSM routes and terminals, sailings from GTFS feeds and looked-up timetables (see README).
$(FER)/ways.geojsonseq: $(OSM)/merged.osm.pbf
	@mkdir -p $(FER)
	osmium tags-filter $< w/route=ferry r/route=ferry -o $(FER)/ferries.osm.pbf --overwrite
	osmium export $(FER)/ferries.osm.pbf -f geojsonseq --geometry-types=linestring -a type,id -o $@ --overwrite
	osmium cat $(FER)/ferries.osm.pbf -t relation -f opl -o $(FER)/relations.opl --overwrite
$(FER)/terminals.geojsonseq: $(OSM)/merged.osm.pbf
	@mkdir -p $(FER)
	osmium tags-filter $< nw/amenity=ferry_terminal -o $(FER)/terminals.osm.pbf --overwrite
	osmium export $(FER)/terminals.osm.pbf -f geojsonseq -a type,id -o $@ --overwrite
$(BUILD)/ferries.json: $(FER)/ways.geojsonseq $(FER)/terminals.geojsonseq dem/ferries.py $(wildcard $(FER)/freq/*.json)
	$(UV) ferries.py
# Re-derive the GTFS sailings (downloads feeds not yet cached), then rebuild the ferry layer.
ferries: $(FER)/ways.geojsonseq $(FER)/terminals.geojsonseq
	$(UV) ferries.py
	$(UV) gtfs.py
	$(UV) ferries.py

# Rail service frequency: trains a day from published timetables (GTFS feeds found in the Mobility
# Database catalogue, plus national operators) and hand-researched MTR lines, matched onto the rail
# ways (see README).
RAIL := $(DATA)/rail
$(RAIL)/feeds_v2.csv:
	@mkdir -p $(RAIL)
	curl -sSL -o $@ https://files.mobilitydatabase.org/feeds_v2.csv
$(RAIL)/feeds.json: $(RAIL)/feeds_v2.csv dem/railfeeds.py regions.json $(DATA)/trees/poly/.done
	$(UV) railfeeds.py
$(RAIL)/pairs.bin: $(RAIL)/feeds.json dem/railgtfs.py
	$(UV) railgtfs.py
$(RAIL)/hk-stations.geojsonseq: | $(OSM)/merged.osm.pbf
	osmium extract -b 113.8,22.1,114.5,22.6 $(OSM)/merged.osm.pbf -o $(RAIL)/hk.osm.pbf --overwrite
	osmium tags-filter $(RAIL)/hk.osm.pbf n/railway=station,halt,stop,tram_stop n/public_transport=station w/railway=station -o $(RAIL)/hk-stations.osm.pbf --overwrite
	osmium export $(RAIL)/hk-stations.osm.pbf -f geojsonseq -o $@ --overwrite
	rm -f $(RAIL)/hk.osm.pbf
$(RAIL)/pairs-mtr.bin: $(RAIL)/mtr.json $(RAIL)/hk-stations.geojsonseq dem/mtrpairs.py
	$(UV) mtrpairs.py
$(BUILD)/rail-freq.bin: $(BUILD)/ways.bin $(RAIL)/pairs.bin $(RAIL)/pairs-mtr.bin crates/pipeline/src/bin/railfreq.rs | target/release/railfreq
	./target/release/railfreq $(BUILD) $(RAIL)/pairs.bin $(RAIL)/pairs-mtr.bin

# Details for hover and popups (see README): POI tags and Wikidata facts, peak prominence and
# isolation, heritage sites' Wikidata facts and descriptions, area sizes and park details.
.PHONY: details
details: $(BUILD)/.layers
# The overlays as the map loads them: lean properties, draw order, simplified polygons (layers.py).
$(BUILD)/.layers: $(BUILD)/.interest $(wildcard $(BUILD)/heritage-areas.json $(BUILD)/indigenous.json $(BUILD)/special.json) dem/layers.py
	$(UV) layers.py && touch $(CURDIR)/$@
# How interesting each stop and heritage site is (fame from Wikipedia pageviews, rarity nearby),
# for thinning the map zoomed out and choosing what gets a written description (last: it rewrites
# pois.json and heritage.json after filterprops).
$(BUILD)/.interest: $(BUILD)/.filterprops $(DATA)/pageviews/items.json dem/interest.py
	$(UV) interest.py && touch $(CURDIR)/$@
# Pageviews from Wikimedia's monthly dumps (one month per season, ~5 GB each, streamed).
$(DATA)/pageviews/items.json: $(HER)/wd/items.jsonl $(BUILD)/details-poi.jsonl dem/pageviews.py
	$(UV) pageviews.py
# The numbers the Stops & sights filters use, stamped onto the map layers (last: it rewrites them).
$(BUILD)/.filterprops: $(BUILD)/details-poi.jsonl $(BUILD)/peaks.json $(BUILD)/details-heritage.jsonl $(BUILD)/details-park.jsonl dem/filterprops.py dem/heritagetiers.py
	$(UV) filterprops.py && touch $(CURDIR)/$@
$(DATA)/poi/pois.geojsonseq: $(OSM)/merged.osm.pbf
	@mkdir -p $(DATA)/poi
	osmium tags-filter $< nwr/natural=peak,volcano,saddle nwr/waterway=waterfall nwr/natural=waterfall nwr/man_made=lighthouse \
	  nwr/tourism=viewpoint,picnic_site nwr/highway=trailhead,rest_area,services nwr/leisure=picnic_table w/bridge=covered \
	  -o $(DATA)/poi/pois.osm.pbf --overwrite
	osmium export $(DATA)/poi/pois.osm.pbf -f geojsonseq -a type,id --geometry-types=point,linestring,polygon -o $@ --overwrite
# Rewrites pois.json (summits tagged as viewpoints become peaks; each POI gains its index), as do the
# steps after it, so it keys on ways.bin (written with pois.json by extract), not pois.json.
$(BUILD)/details-poi.jsonl: $(BUILD)/ways.bin $(DATA)/poi/pois.geojsonseq dem/poidetails.py $(wildcard $(HER)/desc/*.out.jsonl)
	$(UV) poidetails.py
$(BUILD)/peaks.json: $(BUILD)/details-poi.jsonl $(BUILD)/terrain.tiles | target/release/peaks
	./target/release/peaks $(BUILD)
$(HER)/wd/items.jsonl: $(BUILD)/grid.areas.u8 dem/heritagewd.py
	$(UV) heritagewd.py
# Long descriptions: `$(UV) heritagedetails.py extracts N`, then the writers (README), then this.
$(BUILD)/details-heritage.jsonl: $(HER)/wd/items.jsonl $(BUILD)/grid.areas.u8 dem/heritagedetails.py $(wildcard $(HER)/desc/*.out.jsonl)
	$(UV) heritagedetails.py
$(BUILD)/details-park.jsonl: $(DATA)/areas/areas.geojsonseq $(BUILD)/grid.areas.u8 dem/areadetails.py
	$(UV) areadetails.py

# Context layers (water, boundaries, places)
tools/planetiler.jar:
	@mkdir -p tools
	curl -sSL -o $@ https://github.com/onthegomap/planetiler/releases/latest/download/planetiler.jar

# The basemap is built once for the regions of the time (listed in base.regions); regions added
# later get a part each (base-parts/<id>.pmtiles, drawn with the same style), so adding one doesn't
# rebuild it all. `make basemap-full` rebuilds a single basemap from every region.
PLANETILER = java -Xmx5g -jar tools/planetiler.jar --download --storage=mmap --force \
	  --only-layers=water,waterway,boundary,place,water_name,park --languages=en,fr --maxzoom=14
$(BUILD)/base.pmtiles: | $(OSM)/merged.osm.pbf tools/planetiler.jar
	$(PLANETILER) --osm-path=$(OSM)/merged.osm.pbf --output=$(BUILD)/base.new.pmtiles && mv $(BUILD)/base.new.pmtiles $@
	for i in $(IDS); do echo $$i; done > $(BUILD)/base.regions
.PHONY: basemap-parts basemap-full
basemap-parts: $(BUILD)/base.pmtiles
	@mkdir -p $(BUILD)/base-parts
	@for n in $(IDS); do f=$(OSM)/$$n.osm.pbf; \
	  if grep -qx "$$n" $(BUILD)/base.regions 2>/dev/null; then continue; fi; \
	  out=$(BUILD)/base-parts/$$n.pmtiles; \
	  if [ ! -f "$$out" ] || [ "$$f" -nt "$$out" ]; then echo "basemap part: $$n"; \
	    $(PLANETILER) --osm-path=$$f --output=$$out.new.pmtiles && mv $$out.new.pmtiles $$out || exit 1; fi; \
	done
basemap-full: $(OSM)/merged.osm.pbf tools/planetiler.jar
	$(PLANETILER) --osm-path=$< --output=$(BUILD)/base.new.pmtiles && mv $(BUILD)/base.new.pmtiles $(BUILD)/base.pmtiles
	for i in $(IDS); do echo $$i; done > $(BUILD)/base.regions
	rm -rf $(BUILD)/base-parts

fonts:
	@./scripts/fonts.sh $(DATA)/fonts

web: web/dist/index.html
web/dist/index.html: $(WEB_SRC)
	cd web && ([ -d node_modules ] || npm ci) && npm run build

run: web target/release/server
	./target/release/server --data $(BUILD) --web web/dist --fonts $(DATA)/fonts --port $(PORT)

dev: target/release/server
	@trap "kill 0" EXIT; ./target/release/server --data $(BUILD) --web web/dist --fonts $(DATA)/fonts --port $(PORT) & \
	  (cd web && npm run dev); wait

clean-build:
	rm -rf $(BUILD)/*.tmp $(BUILD)/dem-progress.json
