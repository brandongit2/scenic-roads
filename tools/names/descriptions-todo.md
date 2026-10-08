# Descriptions to write

Written by the build Mac after each build (`names-todo`): the landmarks (`landmarks.jsonl`: the
eight kinds of points, heritage sites among them) and the parks and protected areas
(`areas.jsonl`) that have an English Wikipedia article or a heritage register entry and no
description yet, by fame, highest first: how far down to go is a choice. The lists are made again
after every build; don't edit them.

## An entry

- `qid` (the Wikidata item), `id` (its OSM object: `n123`, `w123`, `r123`), `name`, `en` (its
  English, if any), `kind`, `designation` (a heritage site's), `at` (lon, lat) or `bbox`.
- `enwiki`: the English Wikipedia article's title; else `wiki` (`{"lang", "title"}`), an article
  in another language; `register`: the register entry's page, with its `source`.
- `fame`: pageviews-based for landmarks (as the map ranks them), Wikidata's sitelinks or the
  area's size for areas.

## The answer

Files anywhere in `descriptions/` but `todo/` (say `descriptions/written/5-batch-000.jsonl`), one
line each: `{"qid": "Q243", "long": "…", "src": [{"t": "<title>", "u": "<url>"}]}`, or with `"id":
"w123"` for a thing without a Wikidata item. `{"qid": …, "drop": true}` removes an earlier one.
Later file names win. The map picks them up within about a minute and a half.

## The voice

Write like a good reference book or a museum label, not a tour guide.

- **Open with a noun phrase that says what the place is and why it is known**, the claim to fame
  first ("The first Cistercian abbey in Wales", "Former Royal Navy home for retired sailors").
  The name is shown just above: don't open with it.
- Then at most two supporting facts in one or two ordinary sentences: who built it and when, what
  happened there, what it became. Years, not full dates, unless the day is the point.
- **Up to 55 words**, one to three sentences, in English. Ordinary places get one plain sentence
  (10–20 words).
- **Don't inflate mundane places.** If the sources give no real claim to fame, say plainly what
  the place is. Never "iconic", "stunning", "must-see".
- No "It is…" openings, no scene-setting, nothing addressed to visitors, no questions or lists.
  Don't restate the place's own designation; a higher one (a UNESCO listing) is worth a mention.

## Sources, credited

- Write from the English article (`enwiki`) where there is one; else from the other-language
  article (`wiki`), translated, or the register entry (`register`).
- Where the article is about something else (a château's pumping station listed, the article
  about the château), research the place from other reliable sources, or leave it.
- Use only facts the sources state; paraphrase (no run of six or more words copied, names and
  titles aside).
- **Credit every source** in `src`: its title and address.
- Leave a line out rather than write one from nothing: an empty or disambiguation article, or a
  source that says no more than where the place is and that it is protected.
