# Scenic Roads — data pipeline and app.
#
#   make data   refresh everything: newer OSM extracts → roads → elevations → terrain → land
#               cover & tree canopy → viewsheds → designations → tiles → basemap
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
REGIONS := north-america/canada/quebec north-america/canada/ontario north-america/canada/new-brunswick \
           north-america/canada/nova-scotia north-america/canada/prince-edward-island \
           north-america/canada/newfoundland-and-labrador \
           north-america/us/new-york north-america/us/vermont north-america/us/new-hampshire north-america/us/maine \
           north-america/us/massachusetts north-america/us/connecticut north-america/us/rhode-island
PBFS := $(foreach r,$(REGIONS),$(OSM)/$(notdir $(r)).osm.pbf)
RUST_SRC := $(shell find crates -name "*.rs") Cargo.toml $(wildcard crates/*/Cargo.toml)
WEB_SRC  := $(shell find web/src -type f) web/index.html web/package.json web/vite.config.ts

HER   := $(DATA)/heritage
UV    := cd dem && uv run python

.PHONY: all data osm fonts web run dev clean-build heritage
all: data web

data: osm $(BUILD)/roads.tiles $(BUILD)/slope.tiles $(BUILD)/base.pmtiles fonts

# Conditional download: curl -z only fetches when the server copy is newer than ours.
osm:
	@mkdir -p $(OSM)
	@for r in $(REGIONS); do f=$(OSM)/$$(basename $$r).osm.pbf; \
	  if [ -f "$$f" ]; then z="-z $$f"; else z=""; fi; \
	  curl -sSL --fail $$z -o "$$f.part" "https://download.geofabrik.de/$$r-latest.osm.pbf" || exit 1; \
	  if [ -s "$$f.part" ]; then mv "$$f.part" "$$f"; touch "$$f"; echo "updated $$f"; else rm -f "$$f.part"; echo "up to date $$f"; fi; \
	done

$(OSM)/merged.osm.pbf: $(PBFS)
	osmium merge $^ -o $@.tmp.osm.pbf --overwrite --progress && mv $@.tmp.osm.pbf $@

# Binaries are order-only prerequisites of the data steps: editing code doesn't rebuild data
# (delete an output to redo its step).
target/release/extract target/release/tile target/release/server target/release/terrain target/release/scenic target/release/slope: $(RUST_SRC)
	cargo build --release && touch target/release/{extract,tile,server,terrain,scenic,slope}

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

# 3b. terrain slope pyramid (z12 slope, coarser levels = mean of the slopes beneath)
$(BUILD)/slope.tiles: $(BUILD)/terrain.tiles | target/release/slope
	./target/release/slope $(BUILD)

# 4. land cover (ESA WorldCover) on the analysis grid
$(BUILD)/grid.class.u8: $(BUILD)/terrain.tiles dem/landcover.py
	$(UV) landcover.py ../$(BUILD)

# 5. scenic analysis: 100 m road samples → tree canopy (Meta/WRI) near-field horizons →
#    15 km viewsheds and landscape metrics
$(BUILD)/samples.bin: $(BUILD)/elev.f32 $(BUILD)/terrain.tiles | target/release/scenic
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
$(BUILD)/scenic.u8: $(BUILD)/samples.metrics.u8 $(BUILD)/grid.areas.u8
	./target/release/scenic $(BUILD) flags
heritage:
	$(UV) heritage.py ../$(BUILD) && ./target/release/scenic $(BUILD) flags && ./target/release/tile $(BUILD) 4 14

# 7. elevation clean-up, climbs, tile pyramid (with drape heights and scenic channels)
$(BUILD)/roads.tiles: $(BUILD)/elev.f32 $(BUILD)/scenic.u8 | target/release/tile
	./target/release/tile $(BUILD) 4 14

# Context layers (water, boundaries, places)
tools/planetiler.jar:
	@mkdir -p tools
	curl -sSL -o $@ https://github.com/onthegomap/planetiler/releases/latest/download/planetiler.jar

$(BUILD)/base.pmtiles: $(OSM)/merged.osm.pbf tools/planetiler.jar
	java -Xmx5g -jar tools/planetiler.jar --osm-path=$< --download --storage=mmap --force \
	  --only-layers=water,waterway,boundary,place,water_name,park --languages=en,fr --maxzoom=14 \
	  --output=$(BUILD)/base.new.pmtiles && mv $(BUILD)/base.new.pmtiles $@

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
