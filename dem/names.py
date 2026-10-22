"""Whether a name's English truly differs from it, not just in accents, case, punctuation or spacing
(Montréal, Montreal): labels.py's test. The heritage layers' English is crate::heritage_layers'.
"""
from __future__ import annotations

import re
import unicodedata


def norm(s: str) -> str:
    s = unicodedata.normalize("NFKD", s.lower())
    return re.sub(r"[\W_]+", "", "".join(c for c in s if not unicodedata.combining(c)))


def differs(name: str, en: str | None) -> bool:
    return bool(en) and norm(en) != norm(name)
