# Names to translate

Written after each build (`names-todo`): names on the map with no English of their own and no
translation line in any language spoken where they are. One list per language, `<language>.jsonl`
(ISO 639: `fr`, `ja`, `zh` as Mandarin reads it, `yue` Cantonese, `es`, `pt`, `ca`, `gl`, `eu`,
`cy`, `gd`, `ga`, `br`, …), highest priority first; how far down to go is a choice. The lists are
remade every build: don't edit them; answers go elsewhere (below).

## An entry

`{"n", "kind", "langs", "things", "example": {"osm" | "qid" | "label", "at"}, "priority"}`

- `n`: the name exactly as in OpenStreetMap.
- `kind`: `road`, `settlement` (a city, town, village, hamlet, suburb, quarter, neighbourhood or
  isolated dwelling) or `other` (water, parks, peaks, sights, stations, lines, regions).
- `langs`: the languages the name may be in, this list's first (spoken where it is, narrowed by
  OSM's language tags and the script).
- `things`: how many things with the name lack English; `example`: one of them, by OSM object
  (`osm`: `n123` node, `w123` way, `r123` relation), else Wikidata item (`qid`), else label row
  (`label`), with `at` (lon, lat).
- `priority`: from fame, place class and population, or road class (a city ~70, a village 50, a
  hamlet 30; a landmark 30 + 8 × fame; a motorway 45, a residential street 20), plus log `things`.

Not listed: names with English of their own (`name:en`, a romanised name, a kana reading, a
register's English), names already English, bilingual names with an English part, names without
letters, and settlements in Latin script (they keep their own name).

## The answer

A new file per batch anywhere in `inputs/translations/` except `todo/` and `how/` (say
`inputs/translations/answers/fr-001.jsonl`), one line per name: the entry's `n`, `kind` and `langs`
as given, plus `main` and `sub`. The input check takes a batch in within a few minutes; a malformed
line holds the whole file, and lines `check.py` flags hold it until the owner accepts them (build
page or the map's build panel).

`{"n": "Lac Bleu", "kind": "other", "langs": ["fr"], "main": "Lac Bleu", "sub": "Blue Lake", "via": "agent:haiku"}`

- `main`: what the label shows (nearly always the name); `sub`: the English under it, or `null`
  when English leaves the name alone. A `null` line still answers the name.
- `kind` may be a list (`["settlement", "other"]`), and `langs` several languages, when the English
  is the same in each.
- `via`: how it was made (`agent:haiku`, `rule:…`, `hand`).
- A later file name wins for the same name, kind and language. The list loses the name after the
  next build.

## Conventions

- **Title case.** No explanations, notes, parentheses or alternatives: a name.
- **Settlements keep their own name** (Le Moulin, La Gare, Rivière-du-Loup) unless they have a
  well-known English one (Seville, The Hague, Quebec City): `sub` null.
- **An established English name** where there is one (Mount Fuji, Lake Biwa, Lisbon, Mont
  Saint-Michel); else a natural English name in English word order: translate the generic word
  and the descriptive words, drop articles English doesn't need, and keep person, saint and place
  names exactly as written, accents included (Lac Thérèse: Lake Thérèse; Rivière Saint-Pierre:
  Saint-Pierre River; Église Saint-Martin: Church of Saint-Martin).
- **Japanese:** modified Hepburn without macrons (Tokyo, Osaka, Otsubo River, Shinjuku Station);
  translate the generic ending (〜川 River, 〜山 Mount, 〜駅 Station, 〜神社 Shrine, 〜寺 Temple).
- **Chinese (`zh`), Taiwan's readings:** Hanyu Pinyin without tone marks, a name's syllables
  joined (Zhongshan, Xinyi), but the established spellings of cities and famous places (Taipei,
  Kaohsiung, Keelung, Tamsui, Sun Moon Lake); the proper part romanised, the generic ending
  translated (楓樹腳溪: Fengshujiao Creek).
- **Cantonese (`yue`), Hong Kong:** the Hong Kong Government's English names and romanisation
  (Mong Kok, Tsim Sha Tsui, 深灣村 Sham Wan Tsuen).
- **Singapore:** the English name in use.

### Right and wrong

| Name | Right | Wrong |
|---|---|---|
| Lac Long | Long Lake | Lake Long |
| Lac à la Truite | Trout Lake | Lake à la Truite |
| Lac Antoine | Lake Antoine | Anthony Lake |
| Rivière Sainte-Anne | Sainte-Anne River | Saint Anne River |
| Église Saint-Martin | Church of Saint-Martin | Church Saint-Martin |
| Place de l'Église | Church Square | Square de l'Église |
| Col de l'Espinas | Espinas Pass | Pass de l'Espinas |
| Castillo de San Jorge | Saint George's Castle | Castle de San Jorge |
| Río Guadalquivir | Guadalquivir River | River Guadalquivir |
| Serra da Estrela | Estrela Mountains | |
| Afon Taf | River Taff | |
| Le Moulin (settlement) | null | Mill |
| 多摩川 | Tama River | |
| 本町一丁目 | Honmachi 1-chome | |
| 鶴岡八幡宮 | Tsurugaoka Hachimangu Shrine | |

## Checking

`python3 check.py <answers.jsonl>` (in this folder, standard library only) checks a file against
the conventions and prints the problems, or "N lines, all good".
