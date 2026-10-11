#!/bin/zsh
# Upstream's render test cases and assets (FORK.md), left out of the repository (125 MB): fetched from
# the tag this fork starts from into test/integration/render/tests/upstream/ and
# test/integration/assets/ (both ignored). Our own cases stay in test/integration/render/tests/scenic/.
set -euo pipefail
cd ${0:A:h}/..
tag=v6.11.2
tmp=$(mktemp -d)
trap "rm -rf $tmp" EXIT
curl -sfL -o $tmp/src.tar.gz https://github.com/maplibre/maplibre-gl-js/archive/refs/tags/$tag.tar.gz
tar -xzf $tmp/src.tar.gz -C $tmp "maplibre-gl-js-${tag#v}/test/integration/render/tests" "maplibre-gl-js-${tag#v}/test/integration/assets"
rm -rf test/integration/render/tests/upstream test/integration/assets
mkdir -p test/integration/render/tests
mv $tmp/maplibre-gl-js-${tag#v}/test/integration/render/tests test/integration/render/tests/upstream
mv $tmp/maplibre-gl-js-${tag#v}/test/integration/assets test/integration/assets
# Where this fork draws differently on purpose, its own expected image (test/integration/render/expected/,
# committed; FORK.md says why each differs).
(cd test/integration/render/expected && find . -name 'expected*.png') | while read f; do
  rm -f test/integration/render/tests/upstream/${f:h}/expected*.png
  cp test/integration/render/expected/$f test/integration/render/tests/upstream/$f
done
echo "upstream's render tests ($tag) in test/integration/render/tests/upstream/"
