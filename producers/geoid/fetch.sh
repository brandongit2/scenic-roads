#!/bin/sh
# The geoid grids the declarations in declarations/ name (docs/inputs.md §6.8, step 3), each
# downloaded whole into the store: <root>/sources/fetched/<id>/<the URL's last part>, where the
# elevation step reads a grid declared `store = true` (pipeline::dem::geoid). A grid already there
# at the server's size is skipped; a partial one is resumed. Then drop the declarations into
# <root>/inputs/sources/.
#
# usage: producers/geoid/fetch.sh <root, the NAS project folder>
set -eu
root=${1:?usage: fetch.sh <root>}
here=$(cd "$(dirname "$0")" && pwd)
ua="scenic-roads/0.1 (personal offline map)"
# First, alone: each host's robots.txt, judged by robots.py (RFC 9309: groups, our token's group
# over `*`'s, Allow/Disallow longest match, `*` and `$`). A grid's path it disallows, or an error running it,
# stops here; a
# robots.txt that can't be read (404, 401, 403, unreachable) counts as allow.
robots=$(mktemp)
trap 'rm -f "$robots"' EXIT
for decl in "$here"/declarations/*.toml; do
	grep -q '^format = "geoid-grid"' "$decl" || continue
	url=$(sed -n 's/^url = "\(.*\)"$/\1/p' "$decl" | head -n 1)
	host=$(echo "$url" | sed 's|^\(https*://[^/]*\).*|\1|')
	# (The path robots.txt judges: "/" for a URL with none.)
	path=${url#"$host"}
	path=${path:-/}
	if ! curl -sS -f -L --max-redirs 5 -m 30 -A "$ua" -o "$robots" "$host/robots.txt" 2>/dev/null; then
		echo "$host/robots.txt: unreadable, counted as allow"
		continue
	fi
	if ! python3 "$here/robots.py" "$robots" "$path"; then
		echo "$host/robots.txt disallows $path: stopped" >&2
		exit 1
	fi
	echo "$host/robots.txt allows $path"
done
for decl in "$here"/declarations/*.toml; do
	grep -q '^format = "geoid-grid"' "$decl" || continue
	id=$(basename "$decl" .toml)
	url=$(sed -n 's/^url = "\(.*\)"$/\1/p' "$decl" | head -n 1)
	name=${url##*/}
	dir="$root/sources/fetched/$id"
	mkdir -p "$dir"
	size=$(curl -sSIL -A "$ua" "$url" | tr -d '\r' | awk 'tolower($1)=="content-length:"{n=$2} END{print n}')
	have=$(stat -f %z "$dir/$name" 2>/dev/null || stat -c %s "$dir/$name" 2>/dev/null || echo 0)
	if [ "$have" = "$size" ]; then
		echo "$id: $name kept ($size bytes)"
		continue
	fi
	echo "$id: $name ($size bytes)"
	curl -fSL -A "$ua" -C - -o "$dir/$name.part" "$url"
	got=$(stat -f %z "$dir/$name.part" 2>/dev/null || stat -c %s "$dir/$name.part")
	[ "$got" = "$size" ] || { echo "$id: $name is $got bytes, the server says $size" >&2; exit 1; }
	mv "$dir/$name.part" "$dir/$name"
done
