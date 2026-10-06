#!/bin/zsh
# Build the step programs for WebAssembly (wasm32-wasip1), the builds web workers run (docs/workers.md),
# into target/wasm32-wasip1/release/*.wasm. zstd's C is compiled to WebAssembly too (the real zstd:
# the same compressed bytes as natively), with Homebrew's LLVM and WASI libc:
#   brew install llvm@22 wasi-libc; rustup target add wasm32-wasip1
#
#   tools/app/wasm.sh
set -euo pipefail
cd ${0:A:h}/../..
export PATH=/opt/homebrew/opt/rustup/bin:$PATH
llvm=/opt/homebrew/opt/llvm@22/bin
sysroot=/opt/homebrew/opt/wasi-libc/share/wasi-sysroot
[[ -x $llvm/clang && -d $sysroot ]] || { echo "needs Homebrew's llvm@22 and wasi-libc"; exit 2; }
export CC_wasm32_wasip1=$llvm/clang AR_wasm32_wasip1=$llvm/llvm-ar CFLAGS_wasm32_wasip1="--sysroot=$sysroot"
# The page gives each program its memory, capped at what its task may use: one that needs more
# fails cleanly (docs/workers.md §6).
export CARGO_TARGET_WASM32_WASIP1_RUSTFLAGS="-C link-arg=--import-memory"
progs=(extract tile scenic-metrics areaflags elev landcover trees)
cargo build --release --target wasm32-wasip1 -p pipeline ${progs/#/--bin=} 2>&1 | tail -1
for p in $progs; do ls -l target/wasm32-wasip1/release/$p.wasm; done
