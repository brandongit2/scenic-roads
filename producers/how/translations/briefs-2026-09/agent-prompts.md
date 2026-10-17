# The translator prompts, as they were used

For reference: how the batch translators were launched in the original project, and what came back.

## Setup

- Claude Code's Agent tool, `subagent_type: general-purpose`, `model: haiku` (Claude Haiku 4.5), run in the
  background, at most 20 at a time; a new one was started as each finished.
- One agent per batch of 1,000 names (`<region>-NNN.jsonl`, most frequent names first); each read
  `TRANSLATORS.md`, translated 100 lines at a time, appended them to `<batch>.out.jsonl` with a heredoc,
  ran the checker and fixed what it reported.
- The agents had the same tools as the main session (Bash, Read, …). They were told not to browse the web.

## The prompt for a batch

```text
You give English names for place names on a personal offline map. Project root: /Users/brandontsang/projets/road-elevations.

Read data/names/tr/TRANSLATORS.md first and follow it exactly. Your batch: data/names/tr/<batch>.jsonl → write data/names/tr/<batch>.out.jsonl.

Translate every name yourself, one by one, 100 lines at a time, until the whole batch is done. Keep each English on the same line as its own name, in the input's order. Never write or run a script, word list, dictionary or find-and-replace to produce translations; the only commands you run are reading the batch, appending lines with a heredoc, and the checker. When finished, reply with one line only: the batch, the number of lines, and the checker's last line.
```

## Variants

- A batch that had drifted twice (fr-008: each English two to four lines below its own name) was split into
  four batches of 250 and given this extra paragraph; all four came back clean:

  ```text
  This batch tripped up translators before: they lost their place and wrote each English 2–4 lines below its own name. Before appending each chunk, check that its first and last lines carry the same names as the input's lines of those numbers.
  ```

- Redoing a batch whose translator had left most names null (its output thrown away first):

  ```text
  An earlier translator left nearly every name in this batch null, and its output was thrown away. Every name that isn't a place (town, village, hamlet…) and isn't English already gets its English: Río Urdiales is Urdiales River, Pico Bajero is Bajero Peak.
  ```

## What came back (the run of 30 Sep 2026: 45 batches)

- Time: 3 to 17 minutes per 1,000-name batch, most 5–10 (250-name batches: 3–5).
- Tokens per agent (as Claude Code reports them): 65,000–75,000 for 250 names, 85,000–160,000 for 1,000
  (median about 105,000).
- Replies: the agents that had nulled 80–100 % of their batch reported "all good" (the checker allowed nulls
  then), as did the one that wrote its translations through a Python dictionary script against the brief.
  One that ran after the null rule was added reported its failure ("816 of the 836 names that aren't places
  are null"). The replies said little about quality; the checker and sampling did.
