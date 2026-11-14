#!/usr/bin/env python3
"""Where the map's data lives and who builds it, two figures on one page.

To republish: `python3 page.py out.html`, then republish out.html to https://claude.ai/artifact/2LamxyqCeK6YLbkSpPPbrj.

usage: page.py OUT.html [--check] [--full]
  --check  data-max on texts: each text's room in px, to compare with its drawn width in a browser
  --full   a whole document (the published page is a fragment: the host adds doctype and metas)
"""
import sys

import diag
import storage
import workers

OUT = sys.argv[1]
CHECK = '--check' in sys.argv
FULL = '--full' in sys.argv

head = '<!doctype html>\n<html lang="en">\n<head>\n<meta charset="utf-8">\n<meta name="viewport" content="width=device-width, initial-scale=1">\n' if FULL else ''
page = head + f'''<title>Scenic Roads Storage and Pool</title>
<link rel="preconnect" href="https://fonts.googleapis.com">
<link rel="preconnect" href="https://fonts.gstatic.com" crossorigin>
<link rel="stylesheet" href="https://fonts.googleapis.com/css2?family=IBM+Plex+Mono:wght@400;500&family=IBM+Plex+Sans+Condensed:wght@400;600&display=swap">
<style>{diag.CSS}</style>
{'</head><body>' if FULL else ''}
<main>
<header>
<h1>Scenic Roads · where the data lives, who builds it</h1>
<p class="sub">Three Macs build the map as a pool. The Mac mini leads: it holds the pool’s state, the build’s records and the map on its own disk, and the others talk to it by HTTP. The NAS holds the bulk data. Every step of the build is in docs/pipeline.md.</p>
</header>
<figure>
<h2>Where the data lives</h2>
<div class="scroll">{storage.build(CHECK)}</div>
<figcaption>Heavy outline: the NAS. Every built file is named by its content and never rewritten, so a job’s upload changes nothing a reader holds; what the map serves changes only when the lead appends an update to the map’s log. The lead writes its records to its own disk first, then whole to the NAS. Each download is made once, by a job, and kept on the NAS.</figcaption>
</figure>
<figure>
<h2>Who builds it</h2>
<div class="scroll">{workers.build(CHECK)}</div>
<figcaption>The lead’s coordinator grants every job, the lead’s own too. A member’s work is on its own disk until a records snapshot the lead saved names it, so a lead that restarts, or a member that sleeps, loses nothing that was finished.</figcaption>
</figure>
</main>
{'</body></html>' if FULL else ''}
'''
open(OUT, 'w').write(page)
print(f'wrote {OUT}')
