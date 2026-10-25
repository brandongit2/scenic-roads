# NAS access through one layer (planned)

**Status: planned (task #663 design; #530 builds the guard on it). Nothing here is in the code
yet.** What is true today is in §1; §2 onward is the plan.

## 1. Today

- The NAS project folder is a plain `PathBuf` (`agent::find_root`, `cli::scenic::Root --root`),
  handed down and joined everywhere (~1,200 `root`-shaped uses: `.root`, `root.join`,
  `root: &Path`).
- `store::nas` finds and mounts the share; it does no I/O on it.
- `store::nasio` counts this process's NAS traffic, but only where a caller remembers to: 12
  files feed it (`costin` 9 calls, `store::range` 5, `localread` 2, `out` 1, `scenic-build` 1).
- `store::retry` (#656) retries transient SMB errors for the build jobs' reads that use it.
- `pool::nas::Share` already implements the pool protocol's six operations (create-new,
  write-whole, read, read-fresh, exists, list, remove) over a root. It is the closest thing to
  a layer, and the model for this one.
- `out::Out::path` refuses drop-box paths (docs/inputs.md §7.1), and
  `crates/pipeline/tests/inputs_accessor.rs` guards that by scanning the source. That is the
  pattern #530 follows.
- `agent::is_live_path` keeps tests off `/Volumes`, the NAS.

## 2. Census (heuristic, 25 Oct 2026)

The census was made with a line-level scan that tags each filesystem call by the identifiers in
it and in the 4 lines above it. NAS-shaped identifiers are `root`, `out.path`, `.path(`, `share`,
`state/`, `sources/`, `catalog`, `scenic-units`. Local-shaped ones are `home`, `cache`, `scratch`,
`tmp`, `local`, `blob`, `log`. Test code (`#[cfg(test)]` onward, `tests/`) is left out.

| Class | Call sites |
|---|---|
| All fs calls, pipeline + server + store, tests included (crude regex) | ~4,300 |
| Non-test call sites, line-level | 1,505 |
| Clearly NAS | 326 |
| Mixed (both kinds of names nearby) | 199 |
| Clearly local | 379 |
| Unclassified (a path passed in as an argument) | 601 |

The 326 clearly-NAS sites by operation are: stat 88, read 80, remove 48, mkdir 39, list 37,
write 27, rename 6, mmap 1. Most writes and renames are not in that count, because they go
through helpers taking a `&Path` (`whole::write`, `cachefile`, `Out`), so the helper is the one
NAS site.

By module, the clearly-NAS sites are:

| Module | Sites |
|---|---|
| agent | 35 |
| inputs | 31 |
| bin (scenic, scenic-build) | 24 |
| coord | 19 |
| answers, bld | 16 each |
| offload, osmpass, store::mirror | 14 each |
| rawpack, lakefill | 10 each |
| budget, cachefile | 7 each |
| pageviews, leaftype, iopool | 6 each |
| ~25 more | 1–5 each |

The server has about 10 files that take the root (`data`, `regions`, `downloads`, `accepted`,
`terrain`, `tiles`, `names_live`, `water`, `places`, `updater`).

**Read this as an estimate, not a list.** The 601 unclassified sites are helpers called with
paths from both sides. The exact census is the migration's compile errors (§4.1): once the root
is a `NasRoot`, every NAS site stops compiling until it goes through the layer, and every
local-only site keeps compiling.

**Hot paths.** These matter because SMB makes only about 20–55 small files a second
(docs/macs.md):

1. **`Out` saves** (`out.rs`, a step's outputs). These are content-named creates, one per tile
   or part: the creates-per-second limit.
2. **`store::range`** reads ranges and maps the base packs. This is the bulk of the read bytes,
   and the mmap/long-lived opens.
3. **`whole::write` / `pool::nas::Share::write_whole`**: temp file plus rename. These are the
   coordinator's state, the journal hand-offs, the heartbeats.
4. **Mirror and cache fills** (`store::mirror`, `cachefile`, `agent::jobs` copies). These stream
   big NAS files to the local disk.
5. **Listings** in GC, the take-up of a term, `answers` and `osmpass` sweeps. Each takes 3–33 s
   per folder under load.
6. **`inputs::open` and the gate.**

**tools/.** 25 non-seed scripts name the NAS. These are the `tools/nas/*.sh` tar-over-SSH and
move scripts, `publish.sh`, `install.sh`, `health.sh`, `remote-tests.sh`, `guards.sh`, the
taskboard's `backup.sh`, the status app (Swift), `launcher.c`, and `tools/check/*.mjs|sh`. Three
Python files in `dem/` and `producers/` take a root argument.

## 3. The type

### 3.1 `store::nasfs::NasRoot` and `NasPath`

The new module is `crates/store/src/nasfs.rs`, so the server and pipeline share it. The name
`Nas` is taken by `pool::nas::Nas`, the protocol trait, so the root type is `NasRoot`.

```rust
pub struct NasRoot { root: PathBuf, made: Arc<Mutex<BTreeSet<PathBuf>>> }  // Clone; no Deref, no AsRef<Path>
pub struct NasPath { root: NasRoot, rel: RelPath }                           // rel: '/'-joined, no '..', no absolute

impl NasRoot {
    pub fn at(&self, rel: &str) -> NasPath;          // panics on '..' or an absolute rel (track_caller)
}
impl NasPath {
    pub fn join(&self, part: &str) -> NasPath;
    pub fn rel(&self) -> &str;                        // for logs, manifests, nasio notes
    // reads: counted (ReadBytes, Files, Stats, Listed) and retried (store::retry::io)
    pub fn read(&self) -> io::Result<Option<Vec<u8>>>;
    pub fn read_fresh(&self) -> io::Result<Option<Vec<u8>>>;    // F_NOCACHE
    pub fn open(&self) -> io::Result<NasReader>;                 // streaming; impl Read + Seek; counts as read
    pub fn map(&self) -> io::Result<NasMap>;                     // mmap; open long-lived; counted once
    pub fn stat(&self) -> io::Result<Option<Metadata>>;
    pub fn list(&self) -> io::Result<Vec<String>>;               // sorted, temp files left out
    // writes: counted (WroteBytes, Creates, Deletes); NOT blindly retried (see 3.3)
    pub fn write_whole(&self, b: &[u8]) -> io::Result<()>;       // whole::write semantics: temp, fsync, len check, rename (busy-aside)
    pub fn create(&self) -> io::Result<NasWriter>;               // streaming whole write; commit() renames, drop removes the temp file
    pub fn create_new(&self, b: &[u8]) -> io::Result<Created>;   // pool protocol's create-new
    pub fn append(&self) -> io::Result<NasAppender>;             // logs/journals that append today
    pub fn rename_to(&self, to: &NasPath) -> io::Result<()>;
    pub fn remove(&self) -> io::Result<()>;                      // missing = Ok
    pub fn remove_tree(&self) -> io::Result<Tree>;
    pub fn make_dir(&self) -> io::Result<()>;                    // once per process (the `made` set)
    pub fn copy_to_local(&self, dst: &Path) -> io::Result<u64>;  // mirror/cache fills; temp + rename on the local side
    pub fn copy_from_local(&self, src: &Path) -> io::Result<u64>;
    // escape: the only way to a Path
    pub fn unchecked_path(&self) -> PathBuf;                     // greppable; on #530's exemption list per site
}
```

- **There is no `AsRef<Path>`, `Deref`, `Display` or `Into<PathBuf>`.** `Debug` prints
  `nas:<rel>`. Getting a `Path` takes the `unchecked_path` call, which the guard counts (§5).
- **Typed subtrees as the carrot.** Methods such as `NasRoot::state_build()`,
  `sources()`, `coord()`, `cache_units()` and `inputs()` return `NasPath`s. The drop-box refusal
  that `Out::path` does today moves to `inputs()`, which is crate-private to `pipeline::inputs`.
  Writing `root.coord().join("state.json").read()` is shorter than the raw-path version.
- **Errors carry `nas:<rel>` and the op.** The path is in every error with no `with_context` at
  the call site.

### 3.2 Counting

Every method counts through `nasio` itself. `opened` and `wrote` notes go to the ring, and `read`
and `add` go to the counters. That drops the 18 hand-fed calls: `costin`'s wrappers shrink to
deltas of `nasio::own()`, and `localread`/`out`/`scenic-build` stop counting by hand.
`store::range` keeps its local-disk `Local` count and moves its NAS reads onto `NasMap` and
`NasReader`. Counting must stay byte-identical in what it adds, or the measured costs that the
scheduler learned (`costin`, the Book's models) shift. §4.3 covers the check.

### 3.3 Retry

- **Reads, stat and list** go through `store::retry::io` with `transient`. They are idempotent,
  so this is safe everywhere. It is a behaviour change only for sites that don't retry today, and
  only when an error is transient. Decision D2 asks whether to apply it to all.
- **`write_whole`, `create` (commit) and `remove`** retry the whole operation on `transient`.
  Each retry writes a fresh temp file and renames again, which is safe because the rename is
  last-writer-wins on one name with one writer (docs/pool.md §1 rule 3).
- **`create_new` and `append`** are not retried inside the layer. A lost answer cannot tell
  "made" from "not made" (pool::nas's `Unwritten`), and an append may already have landed. The
  caller decides, as `pool::journal` does now.
- **`map` and long-lived `open`** retry only the open. A fault mid-read surfaces as a SIGBUS or
  an error, as it does today.

### 3.4 Who makes one

- **`NasRoot::find(mount: bool) -> Option<NasRoot>`** replaces `agent::find_root`. It is the one
  constructor from the mount.
- **`NasRoot::from_arg(path)`** is used for `--root DIR` (the CLI, `scenic-build`, the server's
  config, run-task specs). It is the one constructor from a string the user supplied. A child
  process gets the root as `--root <unchecked_path>` in its spec. That is one exempt site per
  spawner, in `agent::jobs` and `unit::run`.
- **`NasRoot::local_for_test(dir)`** is `#[cfg(any(test, feature = "testing"))]`. It asserts
  `!is_live_path`, which moves the check here from `Share::new` and the agent's tests.
- **The server** gets it from its config at startup and keeps it in its app state. Its NAS
  handlers take `&NasRoot`.
- **`pool::nas::Share`** becomes a thin `impl Nas for NasRoot`, so the `made` set and
  `write_whole` live in one place. The simulator keeps its own `impl Nas`.

### 3.5 Local stays plain

The agent's home, `agent/cache`, scratch and blobs stay `Path`s and use `cachefile`/`std::fs`.
Copying between the two worlds goes through `copy_to_local`/`copy_from_local`, so the boundary
is explicit.

## 4. Migration (all at once)

### 4.1 Size

- New `store::nasfs`: about 900 lines with tests, mostly moved code (`whole::write`/`rename`,
  `Share`'s helpers, `retry` wiring).
- Call sites: the 326 clearly-NAS sites, plus most of the 199 mixed ones, plus whatever the 601
  unclassified sites hide behind helpers. My estimate is **600–900 call sites in about 180
  files** (pipeline about 150, server about 10, store about 15), plus about 400 signature
  changes from `root: &Path` to `&NasRoot`.
- Roughly 3,000–5,000 changed lines in one branch. That is 2–3 agent-days on the M4 with
  compile-driven edits, plus an Opus review.
- Tests: about 240 test call sites move to `local_for_test`.

### 4.2 Order inside the one branch

The branch is merged once, as one publish.

1. Add `nasfs` with its own tests: byte-identical writes against `whole::write`, retry
   behaviour, counts.
2. Turn the root's type to `NasRoot` at the constructors (`find_root`, `--root`, server config).
   Then fix compile errors outward, crate by crate: store, pipeline (coord, pool, agent, then
   steps), server, bins.
3. Delete what the layer replaces (§6).
4. Run the full suite once, then the parity check (§4.3), then publish.

### 4.3 Not breaking the running rebuild

- **Same bytes, same names, same temp-name scheme.** The temp name stays
  `<name>.<host>.<pid>.tmp` (`whole::tmp_name`), so GC, `is_tmp` and other Macs still recognise
  one mid-flight. No folder layout changes, and no format version bumps.
- **A mixed pool is fine.** Old and new agents produce identical NAS effects, so publish.sh's
  rolling update of the three Macs needs no lockstep, no term change and no `DISKS_V`-style bump.
- **Parity check before publishing.** Run one small region's build steps (an `Out` step, a pack,
  a coord take-up on a copy of `state/coord`) with old and new binaries into two scratch roots
  on the M4. Diff the trees byte for byte and compare the `nasio` count totals. The counts may
  rise only where a site was uncounted before; list those sites in the commit.
- **Retry widening (D2) lands in the same branch but behind one constant.** A rollback of
  behaviour then doesn't need a revert of the types.
- **Publish when a forecast is whole and no long pack job is mid-flight.** The new agent takes
  up the term as usual.

## 5. The guard (#530) on top

The guard is `crates/pipeline/tests/nas_access.rs`, in the `inputs_accessor.rs` style.

- **It fails on** any non-test item outside `store::nasfs` that:
  - calls `unchecked_path()` and is not in `EXEMPT`;
  - names a NAS location literally (`/Volumes/`, `fishandchips`, `personal/projects/scenic-roads`,
    `smb://`) outside `store::nas`;
  - constructs a `NasRoot` other than through `find`, `from_arg` or `local_for_test`.
- **Raw `std::fs` on a NAS path cannot be scanned for** once the type exists, because a NAS path
  isn't a `Path`. The type is the guard there. The scan catches only the escapes.
- **`EXEMPT`** lists entries as `(file, item, why)`, and fails when an entry matches nothing. The
  expected entries are:
  - child-process spawns passing `--root`;
  - `store::range`'s mmap internals, if `NasMap` can't wrap them;
  - the server's `sendfile`/static-file serving of NAS tiles, if any;
  - `nas::unmount`/`mount`;
  - the GDAL/GeoTIFF opens that need a path string (counted by the caller with
    `nasio::add(Tiffs)`).
- **tools/** is checked by a second scan, a list of shell, JS and Swift files allowed to name the
  NAS, which also fails when stale. The allowed set:
  - `tools/nas/*.sh` (tar over SSH, the bulk path: a per-file layer would be slower);
  - `publish.sh`, `install.sh`, `health.sh`, `remote-tests.sh`, `guards.sh`;
  - the taskboard's `backup.sh`;
  - the status app;
  - `launcher.c`;
  - `tools/check/*` (two-Mac tests).

  These are operator tools that don't run in the build. They are listed, not migrated. The
  Python steps in `dem/` and `producers/` receive paths from their Rust runner and are out of
  scope. D4 asks whether that holds.

## 6. To delete along the way

- `nasio` hand calls at 12 files. Keep `nasio` itself: it is the counter store the layer writes
  to.
- `pipeline::whole::{write, rename}` folds into `nasfs`. `whole`'s file-is-whole checks (PNG,
  TIFF) stay.
- `pool::nas::Share`'s own `made`/`parent_made`/`write_whole` become the `NasRoot` ones.
- `agent::find_root` is replaced by `NasRoot::find`. The scattered `assert!(!is_live_path(root))`
  in tests become one, in `local_for_test`.
- `Out::path`'s drop-box panic becomes `inputs()` visibility. `inputs_accessor.rs`'s run-time
  half becomes a compile-time one. Its source scan stays for Python.
- Duplicate "list a folder, skip temp files" helpers in `answers`, `osmpass`, `offload`, `budget`
  and `pubtimes` become `NasPath::list`.

## 7. Risks

- **Size and conflicts.** A one-branch change across about 180 files conflicts with every
  in-flight task. It needs a quiet board window (D1), or a rebase plan where the migrator owns
  main for 2–3 days.
- **Hidden mixed helpers.** A function used for both local and NAS paths must split or become
  generic over a small `trait Fs`. Prefer splitting. A trait brings raw access back in.
- **Count drift.** Newly counted sites change `costin`'s measures and the scheduler's learned
  NAS weights. The parity check lists them, and the Book re-learns. This is the one visible
  behaviour change.
- **mmap over SMB.** `NasMap` must not add a copy or change when the mapping is taken.
- **The server's hot tile path.** One extra allocation of `NasPath` per request is fine. A
  retry on a tile read must not stall an HTTP worker for 3.5 min: the server's reads use a
  short retry schedule (D3).
- **WebAssembly.** `store` and `pipeline` compile to wasm (tools/app/wasm.sh). `nasfs` is
  `cfg(not(target_arch = "wasm32"))`, and the wasm build steps keep their in-memory roots.

## 8. Decisions for the PM

- **D1.** Is a merge window with a quiet board acceptable, given that all at once means about
  180 files in one branch? Or is staging allowed by crate (store+server first, then pipeline),
  each step still behaviour-identical?
- **D2.** Should every NAS read/stat/list get store::retry's 3.5 min retry, which widens #656 to
  all sites? Or only the ones retried today, with the rest unchanged for strict identity?
- **D3.** Should the server get a short retry (for example 5 s) or none?
- **D4.** Are the Python steps (`dem/`, `producers/`) and the tools/ scripts listed rather than
  migrated?
- **D5.** Is the name `NasRoot`/`NasPath` in `store::nasfs` acceptable, and should
  `pool::nas::Nas` stay the protocol trait?
