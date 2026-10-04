# Landmarks golden test (docs/phase5.md "Exactness")

Today's landmarks worker (web/src/landmarks.worker.ts), run under Node on the layer files a server
serves, against the same server's `/api/marks/*` answers. Run against a server whose catalog has
both today's files (`global/legacy/*`) and the converted points (`markdata/`, `marks-*` packs).

    node tools/golden-marks/build.mjs                 # bundles the worker into worker.mjs
    node tools/golden-marks/run.mjs http://localhost:8092      # In view answers and counts: exact
    node tools/golden-marks/blocks.mjs http://localhost:8092   # z6 blocks and popups by id
    node tools/golden-marks/log10vals.mjs http://localhost:8092 /tmp/log10.txt
    cargo run --release -p pipeline --example log10_check < /tmp/log10.txt   # the server's log10

The server's scores use `marks::log10_js`, V8's fdlibm log10 with clang's fused multiply-adds on
arm64: Node's `Math.log10` and the platform's can differ in the last bit.
