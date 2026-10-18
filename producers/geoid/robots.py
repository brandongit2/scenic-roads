"""Whether a robots.txt lets the app fetch a path (RFC 9309), for producers/geoid/fetch.sh.

usage: python3 robots.py <robots.txt file> <path>   exit 0: allowed, 1: disallowed

- Lines: CR stripped, `#` comments dropped, fields case-insensitive.
- Groups: consecutive `User-agent` lines form one group; a `User-agent` after a rule starts a new one.
- The groups naming our product token (`scenic-roads`, case-insensitively, a `scenic-roads/…`
  version after it allowed) are ours, all merged; with none, the `*` groups, merged; with neither, everything is allowed.
- `Allow` and `Disallow` match a path's start, `*` any run of characters, a final `$` its end; the
  longest matching pattern wins, an `Allow` winning a tie; an empty `Disallow` matches nothing.
- Patterns and the path compared with their percent-encoding normalised (RFC 9309 §2.2.2): an
  escape of an unreserved character decoded, the others' hex upper-cased.
- Other fields (`Crawl-delay`, `Sitemap`, …) are ignored. An error running it (exit 1) stops the
  fetch too.
"""

import re
import sys

UNRESERVED = set("ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-._~")


def _norm(s):
    def one(m):
        c = chr(int(m.group(1), 16))
        return c if c in UNRESERVED else "%" + m.group(1).upper()

    return re.sub(r"%([0-9A-Fa-f]{2})", one, s)


def _ours(agent):
    return agent == TOKEN or agent.startswith(TOKEN + "/")

TOKEN = "scenic-roads"


def _groups(text):
    groups = []  # (agents, rules)
    agents, rules, in_rules = [], [], False
    for raw in text.replace("\r\n", "\n").replace("\r", "\n").split("\n"):
        line = raw.split("#", 1)[0].strip()
        if ":" not in line:
            continue
        field, value = line.split(":", 1)
        field, value = field.strip().lower(), value.strip()
        if field == "user-agent":
            if in_rules:
                groups.append((agents, rules))
                agents, rules, in_rules = [], [], False
            agents.append(value.lower())
        elif field in ("allow", "disallow"):
            in_rules = True
            if agents:
                rules.append((field == "allow", value))
    if agents:
        groups.append((agents, rules))
    return groups


def _matches(pattern, path):
    end = pattern.endswith("$")
    body = pattern[:-1] if end else pattern
    rx = "".join(".*" if c == "*" else re.escape(c) for c in body)
    return re.match(rx + ("$" if end else ""), path, re.S) is not None


def allowed(text, path):
    path = _norm(path)
    groups = _groups(text)
    mine = [r for a, r in groups if any(_ours(x) for x in a)]
    if not mine:
        mine = [r for a, r in groups if "*" in a]
    best = None  # (length, allow)
    for rules in mine:
        for allow, pattern in rules:
            if not pattern or not _matches(_norm(pattern), path):
                continue
            key = (len(pattern), allow)
            if best is None or key > best:
                best = key
    return best is None or best[1]


if __name__ == "__main__":
    with open(sys.argv[1], encoding="utf-8", errors="replace") as f:
        sys.exit(0 if allowed(f.read(), sys.argv[2]) else 1)
