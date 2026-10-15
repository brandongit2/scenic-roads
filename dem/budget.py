"""The pool's one budget for Wikimedia (docs/pool.md §7.7): a step's requests go through its agent's
proxy (HTTPS_PROXY, set for every job with the budgets on), which takes each from the budget; a 429
or a Retry-After the step sees is told to the proxy here, and the budget holds every process and
Mac until it ends. With the budgets off (no proxy named), nothing.

usage (in a step): budget.hold(retry_after_seconds_or_None)
"""
from __future__ import annotations

import os
import urllib.parse
import urllib.request

# Where the proxy is (crate::budget::PROXY_ENV): http://127.0.0.1:<port>.
ENV = "SCENIC_BUDGETS"
# The wait told when a 429 says none (crate::budget::HOLD_DEFAULT_S).
DEFAULT_S = 60


# The metered hosts (crate::budget::class_of's, which tests/budgets.rs checks this against): the
# query services, by name, and Wikimedia's sites, a domain and its subdomains.
QUERY = ("query.wikidata.org", "query-main.wikidata.org", "query-scholarly.wikidata.org")
QUERY_DOMAINS = ("qlever.dev", "qlever.cs.uni-freiburg.de")
WIKIMEDIA = ("wikipedia.org", "wikidata.org", "wikimedia.org", "wikimediafoundation.org", "mediawiki.org", "wiktionary.org", "wikibooks.org",
             "wikinews.org", "wikiquote.org", "wikisource.org", "wikiversity.org", "wikivoyage.org", "wmflabs.org")


def metered(url: str) -> bool:
    """Whether `url`'s host is one the budget meters."""
    h = (urllib.parse.urlsplit(url).hostname or "").rstrip(".").lower()
    under = lambda d: h == d or h.endswith("." + d)  # noqa: E731
    return h in QUERY or any(under(d) for d in QUERY_DOMAINS + WIKIMEDIA)


def seen(url: str, code: int | None, retry_after: float | str | None) -> bool:
    """A step's answer from `url`: a 429, or a Retry-After, from a metered host told to the proxy
    (`hold`). Whether it was."""
    if not metered(url) or (code != 429 and not retry_after):
        return False
    return hold(retry_after)


def hold(retry_after: float | str | None = None) -> bool:
    """Tells the agent's proxy a 429 or a Retry-After was seen: every process and Mac waits that long
    (DEFAULT_S when it says none). Whether a proxy was told (False: the budgets are off, or it
    didn't answer: the step's own wait is all)."""
    url = os.environ.get(ENV)
    if not url:
        return False
    try:
        s = float(retry_after) if retry_after not in (None, "") else DEFAULT_S
    except ValueError:
        s = DEFAULT_S
    # (Straight to the proxy, on this Mac: not through itself.)
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
    try:
        with opener.open(urllib.request.Request(f"{url}/hold?s={max(1, int(s + 0.999))}", data=b"", method="POST"), timeout=30) as r:
            r.read()
        return True
    except OSError:
        return False
