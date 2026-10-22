#!/bin/zsh
# Build the step programs for WebAssembly (wasm32-wasip1), the builds web workers run (docs/workers.md),
# into target/wasm32-wasip1/release/*.wasm. zstd's C is compiled to WebAssembly too (the real zstd:
# the same compressed bytes as natively), with the wasi-sdk's clang, llvm-ar and sysroot. The wasi-sdk
# is the version mise.toml pins, and the Rust the one rust-toolchain.toml does (with its wasm32-wasip1
# target); both installed by mise, once:
#   mise install
#
#   tools/app/wasm.sh
set -euo pipefail
case $#:${1:-} in
  0:) ;;
  1:-h|1:--help) print "usage: tools/app/wasm.sh   (no arguments)"; exit 0 ;;
  *) print -u2 "wasm.sh takes no arguments: $*"; exit 2 ;;
esac
cd ${0:A:h}/../..
eval "$(mise env -s zsh)"
wasi=$(mise where "github:WebAssembly/wasi-sdk" 2>/dev/null) || wasi=
[[ -x $wasi/bin/clang && -d $wasi/share/wasi-sysroot ]] || { echo "the wasi-sdk isn't installed: run 'mise install'" >&2; exit 2; }
export CC_wasm32_wasip1=$wasi/bin/clang AR_wasm32_wasip1=$wasi/bin/llvm-ar CFLAGS_wasm32_wasip1="--sysroot=$wasi/share/wasi-sysroot"
# The page gives each program its memory, capped at what its task may use: one that needs more
# fails cleanly (docs/workers.md §6).
export CARGO_TARGET_WASM32_WASIP1_RUSTFLAGS="-C link-arg=--import-memory"
progs=(extract tile scenic-metrics areaflags elev landcover trees bldtile terrainsub)
# (Its whole output kept, ~/Library/Logs/scenic/wasm-<time>.log; shown: the last line, or on
# standard error the errors.)
mkdir -p ~/Library/Logs/scenic
wasm_log=~/Library/Logs/scenic/wasm-$(date +%Y%m%d-%H%M%S).log
if ! cargo build --release --target wasm32-wasip1 -p pipeline ${progs/#/--bin=} > $wasm_log 2>&1; then
  grep -A12 '^error' $wasm_log | head -80 >&2
  echo "WebAssembly build failed; its whole output: $wasm_log" >&2
  exit 1
fi
tail -1 $wasm_log
for p in $progs; do ls -l target/wasm32-wasip1/release/$p.wasm; done
