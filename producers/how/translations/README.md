# Translations: how they're made

English for names on the map that have none of their own (docs/inputs.md §6.3, docs/names.md).

## New translations (on request)

- The app writes `todo/<language>.jsonl` after each build, with the brief `todo/README.md`
  (`tools/names/translations-todo.md` in the repository) and its checker `todo/check.py`
  (`tools/names/check.py`).
- Claude agents on **Haiku** (the owner's choice), a batch of at most 1,000 lines each, at most 20
  at a time, each told to translate every name itself (no scripts, word lists or find-and-replace),
  append 100 lines at a time and run `check.py`; a pilot of 3–4 batches across languages, sampled by
  eye, before the rest, and the checker run over every output after. `briefs-2026-09/agent-prompts.md`
  is the launch prompt used in September.
- Each answer file goes anywhere here but `todo/` and `how/`, its lines tagged `"via":
  "agent:haiku"` (or `rule:…`, `hand`).

## The converted tables (`0-converted/`)

Converted once from the earlier translation work's per-area tables (place and road names for jp,
tw, hk, sg, fr, ib, pt, na, gb), made in the separate place-translations repository: English from
OSM's and Wikipedia's own first (`native`), then word rules and romanisation (`rule:…`), then Claude
agents (the Haiku batches of 2026-09-30, with `briefs-2026-09/TRANSLATORS.md`; the lines tagged
`agent:sonnet`, from that repository's later session). `conversion-log.txt` is the conversion's
log; `analysis/` the scripts used to look at the lists. Neither the conversion script nor the later
session's rules and brief are here (only that repository's branch has them): those lines can't be
remade exactly, only translated again from `todo/`.
