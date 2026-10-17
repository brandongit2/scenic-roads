"""The declared sources (docs/inputs.md §6.9) as the build gives them to a Python step: the JSON file
SCENIC_SOURCES names, each accepted source by its role (`id`, `url`, `urls`, `version`, `params`),
written by the step from the accepted declarations (pipeline::inputs::sources::write_for_python).
No source's address is written in the Python steps: they ask for it here by role.
"""
from __future__ import annotations

import json
import os
from functools import cache
from pathlib import Path

ENV = "SCENIC_SOURCES"


@cache
def _all() -> dict:
    p = os.environ.get(ENV)
    if not p:
        raise SystemExit(f"{ENV} isn't set: the build step gives the declared sources (docs/inputs.md §6.9)")
    return json.loads(Path(p).read_text())


def source(role: str) -> dict:
    """The source declared for `role`."""
    s = _all().get(role)
    if s is None:
        raise SystemExit(f"no source is declared for the {role} role (inputs/sources/, docs/inputs.md §6.9)")
    return s


def url(role: str) -> str:
    """The URL (or template) of the source declared for `role`."""
    u = source(role).get("url")
    if not u:
        raise SystemExit(f"the {role} source names no url")
    return u
