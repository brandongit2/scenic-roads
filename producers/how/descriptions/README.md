# Descriptions: how they're made

Short descriptions of landmarks, heritage sites and areas for the map's popups (docs/inputs.md §6.4,
docs/names.md). Each line credits its sources (`src`: title and URL).

## New descriptions (on request)

- The app writes `todo/landmarks.jsonl` and `todo/areas.jsonl` after each build, with the brief
  `todo/README.md` (`tools/names/descriptions-todo.md` in the repository: the voice, up to 55 words,
  every source credited).
- Claude agents on **Sonnet** (the owner's choice), a batch each, writing from the English Wikipedia
  article, else another language's or the register's entry; a pilot sampled by eye first.
- Answers go anywhere here but `todo/` and `how/`; later file names win.

## Those written so far (`heritage/`)

Sonnet agents in four rounds (September 2026), the file prefixes keeping their order: 1 the pilot
and first batches (`briefs-2026-09/WRITERS.md`), 2 fixes (`FIXERS.md`), 3 a second voice, 4
researched where the article was thin (`RESEARCH.md`). They wrote from the heritage chain's
Wikipedia extracts. The rule that chose those targets and the checks run on them weren't kept; the
set can be written again from `todo/` with today's brief.

