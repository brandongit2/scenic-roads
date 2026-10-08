# Names to translate

Written by the build Mac after each build (`names-todo`): the names on the map that something has
with no English of its own and no translation line in any language spoken where it is. Each list
is one language's, `<language>.jsonl` (ISO 639: `fr` French, `ja` Japanese, `zh` Chinese as
Mandarin reads it, `yue` Cantonese, `es`, `pt`, `ca`, `gl`, `eu`, `cy`, `gd`, `ga`, `br`, …),
sorted by priority, highest first: how far down to go is a choice. The lists are made again after
every build, so don't edit them; answers go elsewhere (below).

## An entry

`{"n", "kind", "langs", "things", "example": {"osm", "at"}, "priority"}`

- `n`: the name exactly as in OpenStreetMap.
- `kind`: `road`, `settlement` (a city, town, village, hamlet, suburb, quarter, neighbourhood or
  isolated dwelling) or `other` (water, parks, peaks, sights, stations, lines, regions).
- `langs`: the languages the name may be in, the list's own first: those spoken where the things
  lacking English are, narrowed by OpenStreetMap's language tags and the name's script.
- `things`: how many things with the name lack English; `example`: one of them, its OSM object
  (`n123` a node, `w123` a way, `r123` a relation: openstreetmap.org/node/123) and where it is
  (lon, lat).
- `priority`: fame, place class and population, road class (a city about 70, a village 50, a
  hamlet 30, a motorway 45, a residential street 20), plus the log of `things`.

Names with English of their own (OSM's `name:en`, a romanised name, a kana reading, a register's
English) aren't listed: that English belongs to its thing, and shows. Nor are names already in
English where English is spoken.

## The answer

Put answer files anywhere in `translations/` but `todo/` (say `translations/answers/fr-001.jsonl`),
one line per name: the entry's `n`, `kind` and `langs` as given, and the display.

`{"n": "Lac Bleu", "kind": "other", "langs": ["fr"], "main": "Lac Bleu", "sub": "Blue Lake", "via": "agent:haiku"}`

- `main`: what the label shows (nearly always the name itself); `sub`: the English under it, or
  `null` when English leaves the name alone (a settlement keeping its name, a name already
  English). A line with `sub` null still answers the name: it leaves the list.
- `kind` may be a list (`["settlement", "other"]`) and `langs` several languages when the English
  is the same in each.
- `via`: how it was made, for the record (`agent:haiku`, `rule:…`, `hand`).
- A later file name wins where lines share a name, kind and language. The map picks a file up
  about a minute and a half after it lands (once it has stood still for 10 s); the list loses the
  name after the next build.

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

`python3 check.py <answers.jsonl>` (in this folder, standard library only) checks an answer file
against the conventions: one valid line per name, no script left to romanise, no half
translations or the original's word order, Hepburn without macrons in Japanese, accents and
saints' names kept. It prints the problems, or "N lines, all good".
