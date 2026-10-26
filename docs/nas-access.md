# NAS access through one layer

The NAS project folder is a type, `store::nasfs::NasRoot`, and its files and folders are
`store::nasfs::NasPath`s (task #663). Neither is a `Path`: a raw `std::fs` call on a NAS file does
not compile. Every NAS read, write, listing and stat goes through a `NasPath` method, which counts
it (store::nasio) and retries the reads, stats and listings the share fails for a moment
(store::retry). `crates/pipeline/tests/nas_access.rs` (task #530) lists every escape from the type.

## 1. The type (`crates/store/src/nasfs.rs`)

- **`NasRoot`** is the project folder: cheap to clone, with the set of folders this process has
  made under it. It has no `AsRef<Path>`, `Deref` or `Into<PathBuf>`. `Debug` prints
  `nas-root:<path>`; `display()` gives the full path for messages, so logs read as before.
  - `NasRoot::find(mount)` finds the share's mount (mounting it first when `mount` is set). The
    agent reaches it through `agent::find_root`, which in a test gives the test's own root.
  - `NasRoot::from_arg(dir)` makes a root from a string the user or a parent process gave:
    `--root`, `--out`, `--layers-root`, `--store`, the server's `--root`.
  - `NasRoot::local_for_test(dir)` exists only in test builds (`store`'s `testkit` feature). It
    refuses the real NAS and the app's home (`nasfs::is_live_path`).
  - `short_retry()` is the server's root: its reads are retried for about 5 s
    (`retry::SHORT_WAITS`, decision D3), never the build's 3.5 minutes.
- **`NasPath`** is a file or folder under a root: the root, and a `/`-joined relative path with no
  `..` and no absolute part (`join` panics on either). Two are equal when they name the same file.
  Path parts: `join`, `parent`, `file_name`, `extension`, `with_file_name`, `with_extension`,
  `rel`, `rel_under`, `tmp_name` (the same `<name>.<host>.<pid>.tmp` as `store::whole::tmp_name`).
- **Reads** are counted as file reads (one `Files`, the bytes, a note in the ring) and retried:
  `read` (store::whole::read: a busy file waited out), `read_to_string`, `read_replaced`,
  `read_fresh` (`F_NOCACHE`), `open` (a `NasFile`: `Read`, `Seek`, its reads counted), `open_plain`
  (store::range::PlainFile), `copy_to_local` (whole), `copy_data_to_local` (store::sys::copy_data).
- **Stats and listings** are counted (`Stats`, `Listed`) and retried: `metadata`, `exists`,
  `try_exists`, `is_dir`, `is_file`, `read_dir` (entries with their names and types), `list`
  (sorted names, temporary files left out, none for a missing folder).
- **Writes** are counted (the bytes and one `Creates`, noted) and never retried: `write`
  (`std::fs::write`), `write_whole` (store::whole::write), `write_whole_made` (its folder made once a
  process), `create`, `open_with` (append, create-new), `create_new` (the pool protocol's), copies
  from local files, renames (`rename`, `rename_over`, `rename_no_replace`), removes (one `Deletes`),
  `create_dir_all`, `set_modified` (a reuse's touch, not counted).
- **`Loc` and `Place`** are a file on this Mac or on the NAS (`Loc` borrowed, `Place` owned), for the
  readers that take both: a local copy or the NAS's file (pipeline::localread), the catalogs'
  folder (a Mac's copies and the NAS's), the packs a step reads. A caller passes a `&Path` or a
  `&NasPath`.
- **The escape** is `unchecked_path()` on a `NasRoot`, a `NasPath` or a `Loc`. It is for a program's
  argument, a library that opens a path itself, a statfs of the mount, and the store's own readers
  below the type (the I/O pool, content-named uploads). Each item making one is listed by the
  guard (§3).

`pool::nas::Nas` stays the pool protocol's trait (decision D5). `NasRoot` implements it, so the
`made` set and whole writes live in one place; the simulator keeps its own.

## 2. What it does on the share

- **The same bytes, names, temporary names and layout** as the raw calls each method replaces.
  `crates/store/src/nasfs/tests.rs` checks the writes against `store::whole::write`,
  `std::fs::write`, `store::whole::copy` and create-new, file for file and byte for byte.
- **Retries (decision D2).** Every read, stat and listing goes through `store::retry::io` with
  `retry::transient` on the root's schedule. A "not found" is never retried there; a caller that
  rides out SMB's moment of ENOENT on a file that is there says so with `retry_with` or
  `retry_any` (`transient_or_missing`). An inner retry that runs out is `Exhausted`, so an outer one
  never starts again. Writes, removes, renames, create-new and appends are never retried here: a lost
  answer can't tell made from not made. `nasfs::RETRY` turns the widening off without a revert.
- **The I/O pool** (store::iopool, the server's and the mirror's) takes `NasPath`s. Its reads are
  counted and retried on the root's schedule; its timeouts and its breaker stay its own.
- **Counting.** Every method counts into store::nasio. Sites counted by hand before keep the same
  totals: `Out`'s uploads (store::naming, raw) count once as they land (`NasPath::count_wrote`),
  pipeline::localread's whole copies once as they're made (`count_read`). `costin::meta` and
  `costin::wrote` keep a target's own measures and no longer add to nasio, which the layer does.
  What the layer counts that was never counted is new: the agent's and the coordinator's records,
  GC's and the archives' walks, the gate's listings, the heritage and depth readers' opens.

## 3. The guard (`crates/pipeline/tests/nas_access.rs`, task #530)

The scan covers every item outside `#[cfg(test)]` code and outside store::nasfs. It fails when:

- an item calls `unchecked_path()` and is not in `EXEMPT`, or makes another number of calls than its
  entry says (50 items, 62 calls when it was built);
- an item makes a root with `NasRoot::from_arg` and is not in `FROM_ARG` (8 items);
- an item names a NAS location literally (`/Volumes/`, the NAS's name, the project folder on the
  share, `smb://`) outside store::nas and `LITERALS` (none);
- a script in tools/, dem/ or producers/ names the NAS and is not in `TOOLS` (decision D4: listed,
  not migrated): tools/nas/, publish.sh, install.sh, remote-tests.sh, health.sh, the task board's
  backup.sh, the status app, the two-Mac tests in tools/check/, and the producers' scripts.

Each list fails when an entry matches nothing, so none can go stale. The test reads the repository's
files, so it carries a `//! scans:` line (tools/dev/testing.md).

## 4. Local stays plain

The agent's home, its caches, scratch folders and blobs stay `Path`s and use store::cachefile and
`std::fs`. Moving bytes between the two goes through `copy_to_local`, `copy_data_to_local` and
`copy_from_local`, so the boundary is explicit. A folder laid out like the project folder on this
Mac (a test's, a scratch run's `--out`) is a `NasRoot` too; the heritage chain's stand-in root, a
local folder of links, is a `Path`.

## 5. What is not done

- `Out::path`'s run-time refusal of drop-box paths stays as it was (docs/inputs.md §7.1); it did
  not become a typed `inputs()` accessor.
- Errors carry the path through `display()`, as before; they don't carry `nas:<rel>` and the
  operation.
- `store::catalog`'s writers (`write`, `write_delta_of`) still take a `Path`; the lead's catalog
  publish passes the NAS folder through `unchecked_path()`. Its readers take a `Loc`.
- The step programs' NAS inputs (osmium's, extract's, the trees program's, GDAL's, basepart's,
  railfeeds', lakefill's, the names crate's tables) are paths on their command lines or in their
  readers: listed in `EXEMPT`, not wrapped.
