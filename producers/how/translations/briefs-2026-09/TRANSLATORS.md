<!-- Reference copy: the brief given to each Claude Haiku translator in the original project, as it
stood on 30 Sep 2026 (after the fixes described in the README). Paths are the original project's:
data/names/tr/ held the batches, and `python3 dem/names.py check` is the checker that
tools/check.py in this folder reproduces (same rules, plus four found later). -->

# Place-name translators (brief for each subagent)

You give English names for places on a personal, offline map app. The English is shown in smaller
text under the native name on the map, and in parentheses after it elsewhere ("浅草寺 (Senso-ji
Temple)", "Lac à la Truite (Trout Lake)"). Nothing is published.

## You translate every name yourself

Read each name and write its English as a person who knows the language would. **Do not write or
run any program, script, word list, dictionary or find-and-replace to produce translations**: a
word-for-word substitution gives "Lake Long", "Church Saint-Martin" and "Mill du Cosquer", and a
script can't read kanji at all. The only commands you run are: reading your batch (with an offset
and limit), appending your lines to the output, and the checker. Create no other file.

## Input

One batch file in `data/names/tr/`, named `<region>-NNN.jsonl`, one JSON object per line:
`{"n": "<name>", "k": "<kind>"}`, and in Japan sometimes `"rd": "<reading>"`. `k` is what the
place is: place (a town, village, hamlet, neighbourhood or island), water (a lake, river, stream,
canal, bay…), park (a national park, nature reserve, protected area or Indigenous reserve), peak,
waterfall, station, rail line, ferry line, terminal (a ferry terminal), stop (a viewpoint, rest
area, trailhead or lighthouse) or heritage site (a listed building, monument or historic area).
`rd` is the name's reading in kana: romanise from it (本町 with ほんまち is Honmachi, with
ほんちょう Honcho).

The region (the file name's prefix) says where the places are and how names are read: `jp`
Japan; `tw` Taiwan; `hk` Hong Kong; `sg` Singapore; `na` North America (mostly French names from
Quebec); `fr` France; `ib` Spain, Portugal and Andorra (Spanish, Portuguese, Catalan, Galician,
Basque); `pt` the Azores and Madeira; `gb` Britain and Ireland (Welsh, Irish, Scottish Gaelic).

## What to write

The English a good English-language map or guidebook would print:

1. **An established English name** if there is one: Mount Fuji, Lake Biwa, Kinkaku-ji, Taipei
   101, Sun Moon Lake, Victoria Harbour, Lisbon, Seville, Mont Saint-Michel, Quebec City.
2. **Otherwise a natural English name in English word order**: translate the generic word and the
   descriptive words, and drop articles and prepositions that English doesn't need. **Person,
   saint and place names stay exactly as written, accents included**: Lac Antoine is Lake
   Antoine (not Anthony Lake), Lac Thérèse is Lake Thérèse (not Therese), Rivière Saint-Pierre is
   Saint-Pierre River (not Saint Peter River). Only an established English name changes them
   (Fleuve Saint-Laurent: Saint Lawrence River). If you can't make it natural English, write
   `null` (Latin script only).
3. **`null` for Latin-script names English leaves alone**: names already English, and names of
   towns, villages, hamlets and localities (`place`), which keep their own name ("Le Moulin",
   "La Gare", "Rivière-du-Loup", "Château-Gaillard") unless they have a well-known English name
   (Seville, The Hague). Every non-Latin name gets an English name.

Write each line's final English the first time: never placeholder lines or nulls to fill in
later. A `null` is only for a name English leaves alone (rule 3), never for one that's hard or that
you haven't reached: a peak, pass, river, lake, spring, beach, waterfall or stop named with a
generic word in another language gets English (Río Urdiales: Urdiales River, Pico Bajero:
Bajero Peak). The checker rejects a batch that leaves many of them null. Keep it a name: no explanations, notes, parentheses or alternatives. Title case. Don't invent an
established English name you aren't sure of: translate and romanise instead.

### Latin script: right and wrong

| Name | Right | Wrong |
|---|---|---|
| Lac Long | Long Lake | Lake Long |
| Lac à la Truite | Trout Lake | Lake à la Truite |
| Lac des Îles | Lake of the Islands | Lake des Islands |
| Lac Castor | Beaver Lake | Lake Castor |
| Lac Simon | Lake Simon | |
| Lac Antoine | Lake Antoine | Anthony Lake |
| Lac Saint-Pierre | Lake Saint-Pierre | Saint Peter Lake |
| Rivière Sainte-Anne | Sainte-Anne River | Saint Anne River |
| Lac Thérèse | Lake Thérèse | Therese Lake |
| Rivière Jacques-Cartier | Jacques-Cartier River | River Jacques-Cartier |
| Rivière à l'Eau Claire | Clearwater River | River à l'Eau Clear |
| Ruisseau des Hauts Ponts | High Bridges Brook | Stream des Hauts Ponts |
| Étang Neuf | New Pond | Pond Neuf |
| Église Saint-Martin | Church of Saint-Martin | Church Saint-Martin |
| Église | Church | |
| Ancien château | Former Castle | |
| Place de l'Église | Church Square | Square de l'Église |
| Château de la Planche (heritage site) | Planche Castle | Castle de la Planche |
| Col de l'Espinas | Espinas Pass | Pass de l'Espinas |
| Parc national de la Vanoise | Vanoise National Park | |
| Canal de la Vallée des Baux | Baux Valley Canal | |
| Castillo de San Jorge | Saint George's Castle | Castle de San Jorge |
| Río Guadalquivir | Guadalquivir River | River Guadalquivir |
| Embalse de Alarcón | Alarcón Reservoir | |
| Igreja de São Roque | Church of São Roque | Church São Roque |
| Serra da Estrela | Estrela Mountains | |
| Afon Taf | River Taff | |
| Llyn Tegid | Bala Lake | |
| Le Moulin, La Gare, Les Marais (place) | null | Mill, Station, Marshes |
| Cascade River, Lake Superior | null | Waterfall River |

### Japan (modified Hepburn without macrons)

Every Japanese name gets English, places too (緑町 is Midoricho, 本町 Honmachi). Hepburn
spellings: shi, chi, tsu, fu, ji, sha, sho, cha (大坪川 is Otsubo River, not Otubo; 石川 Ishikawa,
not Isikawa); long vowels without macrons or doubled letters (大野 Ono, 東京 Tokyo, 大阪 Osaka).
Romanise the whole name, and translate the generic ending where English maps do: 〜川 river,
〜湖 lake, 〜池 pond, 〜山/〜岳 mount, 〜島 island, 〜駅 station, 〜線 line, 〜神社 shrine,
〜寺 temple, 〜城 castle, 〜公園 park, 〜滝 falls, 〜峠 pass, 〜岬 cape. Keep the reading whole
when dropping the ending would leave a single syllable. Neighbourhoods and chome stay romanised.

| Name | Right |
|---|---|
| 多摩川 | Tama River |
| 大川 | Okawa River |
| 黒川 | Kurokawa River |
| 荒川 | Arakawa River |
| 三国山 | Mount Mikuni |
| 浜名湖 | Lake Hamana |
| 新池 | Shinike Pond |
| 沖ノ島 | Okinoshima Island |
| 新宿駅 | Shinjuku Station |
| 銀座線 | Ginza Line |
| 鶴岡八幡宮 | Tsurugaoka Hachimangu Shrine |
| 清水寺 | Kiyomizu-dera Temple |
| 華厳滝 | Kegon Falls |
| 本町一丁目 | Honmachi 1-chome |
| 日本橋 | Nihonbashi |
| 緑町 | Midoricho |
| 大坪川 | Otsubo River |

### Taiwan, Hong Kong, Singapore

Every name gets English. Taiwan: Hanyu Pinyin without tone marks, syllables of one name joined (Zhongshan, Xinyi), but the
established spellings for cities and famous places (Taipei, Kaohsiung, Taichung, Hsinchu, Keelung,
Tamsui, Sun Moon Lake, Taroko Gorge). Translate the generic ending: 〜溪 river or creek, 〜河
river, 〜圳 canal, 〜排水 drain, 〜渠 channel, 〜山 mount, 〜區 district, 〜里 village, 〜站 station,
〜線 line, 〜宮/〜廟 temple, 觀景台 viewing platform. 嘉南大圳北幹線 is Jianan Canal North Main
Line; 滯洪池 is Detention Pond. Hong Kong: the Hong Kong Government's English names and Cantonese
romanisation (Mong Kok, Tsim Sha Tsui, Sha Tin, Lion Rock). Singapore: the English name in use.

In Chinese names the proper part is romanised, not translated word by word; only the generic
ending (and words like 北 north, 新 new, 大 big when they're separate from the name) are
translated. 楓樹腳溪 is Fengshujiao Creek (not Maple Tree Foot Creek), 溪墘大排 Xiqian Main Drain,
五源 Wuyuan; in Hong Kong 深灣村 is Sham Wan Tsuen, 大岩口 Tai Ngam Hau, 象門 Tseung Mun, 小石澳北山
Siu Shek O North Hill. Use a translation only when it is the place's established English name
(太平山 Victoria Peak, 獅子山 Lion Rock, 日月潭 Sun Moon Lake).

## Output

Write `<batch>.out.jsonl` next to the input (e.g. `jp-003.jsonl` → `jp-003.out.jsonl`), one line
per input line, in the same order: `{"n": "<the name, exactly as given>", "en": "<English>"}` or
`{"n": "…", "en": null}`. If the output file already exists (an earlier translator stopped
part way), count its lines first (`wc -l`) and carry on from the next input line; don't redo or
repeat the lines it has. Work through the input 100 lines at a time (read with an offset and
limit), and after each 100 append their lines to the output with a heredoc
(`cat >> data/names/tr/<batch>.out.jsonl <<'EOF'` … `EOF`), so no work is lost. The shell is
zsh: a line holding only a redirection (`> file`) waits forever for input, so never empty or
recreate the output file that way. At the end, run
`python3 dem/names.py check data/names/tr/<batch>.jsonl` from the project root and fix every line
it reports until it says "all good" (to fix a line, rewrite the output file with that line
corrected; keep every other line as it is).

Do not browse the web, search, spawn other agents or modify any other file. When finished, reply
with one line: the batch, the number of lines, and the checker's last line.
