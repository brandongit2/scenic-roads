#!/usr/bin/env python3
"""No testkit in a production build (task #1135; tools/dev/testing.md, "prod: the production build").

  python3 tools/check/testkit_deps.py            scan the repository's manifests (tools/app/guards.sh runs it)
  python3 tools/check/testkit_deps.py PATH ...   scan these Cargo.toml files only

A crate's `testkit` feature holds test-only items (`cfg(any(test, feature = "testkit"))`). Turned on by
a dev-dependency it reaches test targets alone; turned on by anything else, every build of the crate
that dependency is in has it, production too, and an item only the testkit has can be used in
production code unseen: check.sh's prod part builds the whole workspace while publish builds
`-p server -p pipeline`, so a member publish doesn't build enabling a crate's testkit would turn it on
for the prod part alone (#1097's StopCarry, caught by publish's release build).

So a feature naming testkit (`testkit`, `store/testkit`, `store?/testkit`, any feature whose name holds
"testkit") is allowed only in a `[dev-dependencies]` table (`[target.<cfg>.dev-dependencies]` too) and
in the lists of the testkit features themselves (`[features] testkit = ["store/testkit"]`). Refused in
`[dependencies]`, `[build-dependencies]` (and their `[target.<cfg>.…]` forms), `[workspace.dependencies]`
(a member inheriting it with `workspace = true` gets its features in any table) and any other feature's
list (`default = ["testkit"]`, `bench = ["store/testkit"]`).
"""
import subprocess
import sys
import tomllib

DEP_TABLES = ("dependencies", "build-dependencies")


def is_testkit(feature):
    return "testkit" in feature.rsplit("/", 1)[-1]


def dep_features(table):
    for name, spec in (table or {}).items():
        if isinstance(spec, dict):
            for f in spec.get("features", []):
                if is_testkit(f):
                    yield name, f


def scan_manifest(text):
    """Each (table, entry, feature) turning a testkit on outside a dev-dependency or a testkit feature."""
    m = tomllib.loads(text)
    found = []
    for t in DEP_TABLES:
        found += [(f"[{t}]", n, f) for n, f in dep_features(m.get(t))]
    for cfg, tables in (m.get("target") or {}).items():
        for t in DEP_TABLES:
            found += [(f"[target.{cfg}.{t}]", n, f) for n, f in dep_features(tables.get(t))]
    found += [("[workspace.dependencies]", n, f)
              for n, f in dep_features((m.get("workspace") or {}).get("dependencies"))]
    for name, enables in (m.get("features") or {}).items():
        if not is_testkit(name):
            found += [("[features]", name, f) for f in enables if is_testkit(f)]
    return found


def manifests():
    out = subprocess.run(["git", "ls-files", "--cached", "--others", "--exclude-standard", "*Cargo.toml"],
                         capture_output=True, text=True, check=True).stdout
    return [p for p in out.split("\n") if p.endswith("Cargo.toml")]


def main(paths):
    bad = 0
    for p in paths or manifests():
        with open(p, "rb") as f:
            text = f.read().decode()
        for table, entry, feature in scan_manifest(text):
            print(f"{p}: {table} {entry} turns on {feature!r}: a testkit feature only in [dev-dependencies] "
                  "or a testkit feature's own list (tools/check/testkit_deps.py says why)")
            bad += 1
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
