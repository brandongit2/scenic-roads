# Idea, not adopted: inputs in the repository

**Status:** an idea of the owner's, written down on 14 November 2026 and shelved the same day as
premature. Nothing here is built or planned; docs/inputs.md describes how inputs work.

## The idea

Today every input is a folder on the NAS that inputs are dropped into, and the lead watches those
folders and checks each change (docs/inputs.md). The idea is to make the build replicable from the
repository instead:

- **Small things that are hard to make again are committed:** region definitions, source
  declarations, the results of research, scores, accepted warnings.
- **Big things that are easy to make again are "generated data"**, made by something that is
  committed: the place-name translations and heritage descriptions would come from a committed
  script that calls an LLM API, so the result is mostly determined by the commit (but for the
  model's own variation).
- **A change to an input is a commit.** Nothing watches folders for changes.
- **Committed things leave the NAS.** They are small, and the lead's own disk serves them better.

The owner's own observation while writing it: in this shape "input" may stop being a concept of
its own, since definitions and the scripts that generate data both become part of the codebase.

## Measured on 14 November 2026

| Inputs folder on the NAS | Size | Files |
|---|---|---|
| translations | 1.3 GB | 218 |
| heritage | 149 MB | 81 |
| descriptions | 74 MB | 62 |
| extents | 66 MB | 584 |
| sources | 18 MB | 42 |
| elevation | 14 MB | 24 |
| regions | 9 MB | 1,110 |
| timetables | 8 MB | 150 |
| languages, heritage-matches | under 50 KB | 4 |

All but the translations come to about 340 MB. The repository's history was 153 MB, 62 MB of it the
staged terrain declarations under `producers/extents/terrain/`, which are already copied to the NAS
by hand after each change.

## Wrinkles found when it was discussed

1. **The repository is public.** Data under non-commercial, personal-use or unclear licences is
   acceptable in a map that is never shared; committing it to a public repository would share it.
   Either the repository becomes private or such data stays out of git (see 2).
2. **"Easy to make again" is not "comes out the same".** LLM output differs from run to run, models
   are retired, and publishers' indexes change or refuse connections. Every step's key is a digest
   of its inputs' content, so a from-scratch run that produced different translations would rebuild
   everything after them. This suggests three kinds, not two:
   - *definitions*, committed;
   - *recipes*, committed (producers, prompts, model ids and parameters);
   - *pinned outputs*, not committed: whatever a recipe makes that is not bit-for-bit reproducible,
     and large fetched files, each listed by name and content hash in a committed lock file, with
     the bytes in a store keyed by hash. Making them again is then a deliberate act that ends in a
     new lock, and a commit still determines a build.
3. **The pinned outputs become the thing that must not be lost.** They are large but not cheap to
   replace exactly, so their store needs a backup; the lock makes a missing one detectable.
4. **Translations are a loop.** The place names change with every OSM pass. The build cannot write
   to the repository, so it would emit what is still wanted as an output, a recipe would fill only
   what is missing from a memo keyed by prompt, model and source string, and the result would be a
   new lock. The timetables' to-do list works the same way.
5. **The input check does not disappear.** Shape checks could run before a merge and accepted
   warnings become a committed file, but the heavy checks (the regions check reads every file and
   takes about an hour on the lead) cannot. The lead would still keep "the last commit whose
   definitions passed" and build from it while a newer commit is checked.
6. **A commit has to reach the lead.** Shipping definitions inside the app's release makes code
   and definitions change together (no ordering of "file first or release first"), but every edit
   then costs a full publish. A second, quick channel pinned to the same commit would avoid that.
   The lead pulling from GitHub by itself was not favoured.
7. **Git and files that are made again wholesale.** Hand-reviewable text that changes rarely suits
   git; generated files re-made whole (the extents' outlines) grow the history for good. A rule of
   thumb: reviewable text under about 1 MB a file in git, generated or large files behind the lock.
8. **The move must change no key.** Keys hash content, so a move should be free, but at least one
   version string includes the accepted index's name under `sources/inputs/`. Each input would need
   a check that its digests are the same after the move.
9. Secrets (`inputs/keys.env`) never go in git; a recipe that calls a paid API should say what a
   run will cost before it runs.

## Questions left open

- How quickly must an edited definition take effect: seconds, or a few minutes?
- Must a commit rebuild bit for bit, or is "equivalent" enough for LLM text? The lock file assumes
  bit for bit.
- Would the store of pinned outputs stay on the NAS, with only the definitions moving to the lead's
  disk?
- Whether the repository stays public.
