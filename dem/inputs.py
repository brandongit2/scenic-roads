"""The drop boxes' accessor for the Python steps (docs/inputs.md §7.1).

A step never builds a drop-box path: it's given its inputs' paths on its command line, by the agent,
which takes them from the accepted versions (the Rust side's `pipeline::inputs::open`). Every step
opens an input argument through `arg`, which refuses a path in a drop box (under `inputs/`,
`translations/` or `descriptions/`, but the checked copies under `sources/inputs/`, what's fetched
under `sources/fetched/` and the job's own scratch folder), so a step handed a drop box fails
instead of bypassing the gate.
"""

from pathlib import Path

# The folders that hold the drop boxes (pipeline::inputs::DROP_ROOTS).
DROP_ROOTS = ("inputs", "translations", "descriptions")


class DropBox(ValueError):
    """A path in a drop box, given to a step."""


def arg(path) -> Path:
    """`path` (an input argument) as a Path; DropBox when it lies in a drop box."""
    p = Path(path)
    parts = p.parts
    # (A job's own scratch folder is its own, whatever it names inside.)
    if "scratch" in parts:
        return p
    for i, part in enumerate(parts):
        if part in DROP_ROOTS and not (part == "inputs" and i > 0 and parts[i - 1] == "sources") and i < len(parts) - 1:
            raise DropBox(f"{path}: a drop box; a step reads an input's accepted version, given on its command line (docs/inputs.md §7.1)")
    return p
