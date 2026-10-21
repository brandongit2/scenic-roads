# Names and descriptions

**One pipeline for every named thing:** places, states, seas and lakes, rivers, parks and protected
areas, natural features, sights, heritage sites and areas, Indigenous lands, stations, rail lines,
ferries, and road names (in the app's text).

**Display.** A **main** label and an optional smaller **sub** line (under main on the map, "main
(sub)" in text). Sub shows only when it truly differs from main: not the same but for accents, case,
punctuation or spacing, and not one of main's parts.

**A thing's English, in order** (`names::display`):
1. **Its own,** from a source about that thing, never copied to other things of the same name, and
   shown even where wrong (`names::own`): OSM's `name:en`; its romanised name (`name:ja-Latn`,
   `name:zh-Latn-pinyin`…); its kana reading romanised by modified Hepburn without macrons
   (`names::romaji`); a register's, UNESCO's or English Wikipedia's title where the heritage and
   landmark jobs put it in `en`. An own English equal to the name is none.
2. **Its name's translation:** the line for its name, kind and language, trying first the languages
   OSM tags the name in (each `name:<language>` equal to `name`), those spoken there first, then each
   language spoken where the thing is. A name tagged Chinese is read in Cantonese where Cantonese is
   spoken and Mandarin isn't.
3. **Else none:** the name alone, and the name goes on the to-do list.

**Translations are keyed by name, kind and language, in every script.**
- **Kind:** road, settlement (city to isolated dwelling) or other. A name is looked up in its own
  kind only. Labels: a place by its class, the rest other; the basemap: `place` by class,
  `transportation_name` road, the rest other; roads road; everything else other.
- **Language:** ISO 639, first subtag. A line holds for one language or several (a name that could
  be in any local language with the same English is filed under all). 中山 has a Chinese, a Japanese
  and a Cantonese line.
- The languages describe the name, not a region, so a line serves everywhere they're spoken.

**Languages spoken where a thing is** (`names::spoken`), in order:
- **Per territory:** the languages input's table (docs/inputs.md §6.2: `inputs/languages/
  territories.tsv`, from CLDR by `producers/languages/cldr-languages.py`): official or de facto
  official languages, most spoken first.
- **Refined** per subdivision (the producer's `REFINED`): Quebec French then English, New Brunswick
  English then French, Catalonia, Valencia, Galicia, the Basque Country, Wales, Scotland, Brittany,
  Hong Kong and Macau (Cantonese).
- **Where:** the pass's outlines of ISO 3166-1 territories and of the subdivisions with a line,
  simplified (1 km, 250 m), as a raster of 1/128° cells, run-length coded, the smallest outline
  winning. Beyond every territory no language is spoken.
  - **Made once per pass** by the `spoken` job (keyed by the outlines, `spoken::RULES` and the
    languages' accepted version): `global/spoken`, in the World download.
  - A catalog without it: the server makes the raster from the catalog's outlines and keeps it in
    its home (`names/spoken-…bin`).
  - Until it's there, names show with their own English alone (`/api/catalog`'s `names.waiting`).
- It's the only place location enters.

**Files:** `inputs/translations/**/*.jsonl` (not `todo/` nor `how/`), checked by the input check
(docs/inputs.md §6.3): `{"n": "Lac Bleu", "kind": "other", "langs": ["fr"], "main": "Lac Bleu",
"sub": "Blue Lake", "via": "agent:haiku"}`.
- `n`: the name as in OSM; `kind`: one or a list; `langs`: one or a list; `main`, `sub`: the display
  (`main` empty: the name; `sub` null: nothing); `via`: how it was made ("todo" and "skipped" lines
  are left out).
- Where lines share a name, kind and language, the later file (by path) wins, then the later line.
- The server copies the accepted version the lead's records name (crate::accepted,
  pipeline::inputs::served), checking every minute while in use.
- Compiled per file into one arena indexed by name (`names::table`), shared through one `Arc`
  (`names::Namer`).
- **Versions:** each language's is a hash of the accepted files holding lines in it. A tile's ETag
  includes the versions of the languages spoken within it (a tile wider than 40° takes all) and the
  raster's.

The tables converted from the earlier translation work are `0-converted/<language>.jsonl`
(converted once; the log is `inputs/translations/how/conversion-log.txt`); their
lines hold for their source area's languages.

**The build carries each thing's own English and language tags:** labels (`dem/labels.py`: `en`,
`kana`, `l` the languages OSM gives its name, `o` its OSM object); the basemap (`name:en`, `name:fr`,
and from the next pass `osmpass::BASEMAP_LANGUAGES`); roads (`global/roaden/<u>`, no language tags).

**To-do** (`pipeline::namestodo`, the `names-todo` job, after a catalog the map serves that changed
what it reads (`inputs_key`): at most daily while z6 tiles build, after any such once none's left):
- **`inputs/translations/todo/<language>.jsonl`:** each name something in the coverage has with no
  English of its own and no line in any language spoken there. Read from labels, roads and rail
  lines (base packs, each way placed once by the ways-here index), and landmarks. Names already in
  English, without letters, or settlements in Latin script aren't listed.
  - **Each entry:** the name and kind; candidate languages (spoken there, narrowed by OSM's tags and
    the script); how many things lack English and one of them; a priority (by class, population,
    fame or road class, plus log₁₀ of the count). Each goes on its first candidate's list.
  - **The brief:** `inputs/translations/todo/README.md` (`tools/names/translations-todo.md`), with
    `check.py` (`tools/names/check.py`) to check an answer file.
- **`inputs/descriptions/todo/`:** `landmarks.jsonl` and `areas.jsonl`, things with an English
  Wikipedia article or register entry and no description, by fame; `README.md`
  (`tools/names/descriptions-todo.md`) is the writers' brief.
- Running the translators and writers is done on request.

**Descriptions** are `inputs/descriptions/**/*.jsonl` (docs/inputs.md §6.4): `{"qid", "long",
"src"}`, or `{"id": "n123"|"w123"|"r123", "long"}`; `"drop": true` removes one. They're laid over
the popups of details, landmarks and area overlays when serving, matched by `qid`, else the first
`wikidata`, else `osm`. Later file names win; `src` is credited.
