# NAS access through one layer

The NAS project folder is a type, `store::nasfs::NasRoot`, and its files and folders are
`store::nasfs::NasPath`s (task #663). Neither is a `Path`: a raw `std::fs` call on a NAS file does
not compile. Every NAS read, write, listing and stat goes through a `NasPath` method, which counts
it (store::nasio), retries the reads, stats and listings the share fails for a moment
(store::retry), and makes it within a deadline (store::deadline, §6).
`crates/pipeline/tests/nas_access.rs` (task #530) lists every escape from the type.

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
  - A root carries its retry schedule, and its copies with another share its `made` set:
    the default is the build's ~3.5 minutes (`retry::WAITS`: jobs, the CLI, the threads the
    agent's loop starts and never waits on, which take `long_retry()`), its calls' deadline
    `deadline::LONG` (120 s); `short_retry()`, about 5 s (`retry::SHORT_WAITS`, decision D3), is
    the server's, and `no_retry()` the agent's loop's, their calls' deadline `deadline::SHORT`
    (20 s); `no_retry()`
    (`Agent::root`), and so the pool's tick, its `Deadlined` NAS, the coordinator's and the
    budgets' proxy's: their reads fail at once, as before #663, and the next beat reads again
    (25 Oct: a loop stalled on slow reads stops planning and the pool).
    Two page readers on threads of their own read often and are asked again soon, so neither takes
    the long schedule: the map's finished tiles (agent::builtview, a minute apart) read on the
    server's short one, a blip ridden out and an outage shown at once; the build page's view
    (agent::pageview, the pause each second) reads on the loop's root, under nasread's deadline.
- **`NasPath`** is a file or folder under a root: the root, and a `/`-joined relative path with no
  `..` and no absolute part (`join` panics on either). Two are equal when they name the same file.
  Path parts: `join`, `parent`, `file_name`, `extension`, `with_file_name`, `with_extension`,
  `rel`, `rel_under`, `tmp_name` (the same `<name>.<host>.<pid>.tmp` as `store::whole::tmp_name`).
- **Reads** are counted as file reads (one `Files`, the bytes, a note in the ring) and retried:
  `read` (store::whole::read: a busy file waited out), `read_to_string`, `read_replaced`,
  `read_fresh` (`F_NOCACHE`), `open` (a `NasFile`: `Read`, `Seek`, its reads counted, each filling
  a buffer of a MB in one call), `open_plain` (store::range::PlainFile, docs/pool.md §3),
  `copy_to_local` (whole), `copy_data_to_local` (store::sys::copy_data).
- **Stats and listings** are counted (`Stats`, `Listed`) and retried: `metadata`, `exists`,
  `try_exists`, `is_dir`, `is_file`, `read_dir` (entries with their names and types), `list`
  (sorted names, temporary files left out, none for a missing folder).
- **Writes** are counted (the bytes and one `Creates`, noted) and never retried: `write`
  (`std::fs::write`), `write_whole` (store::whole::write), `write_whole_made` (its folder made once a
  process), `create`, `open_with` (append, create-new), `create_new` (the pool protocol's), copies
  from local files, renames (`rename`, `rename_over`, `rename_no_replace`), removes (one `Deletes`),
  `create_dir_all`, `set_modified` (a reuse's touch, not counted).
- **`Loc` and `Place`** are a file on this Mac or on the NAS (`Loc` borrowed, `Place` owned), for the
  readers that take both: a local copy or the NAS's file (buildstore::localread), the catalogues'
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
  `retry_any` (`transient_or_missing`). A retry inside another's try tries once and leaves the waits
  to the outer one (`retry::attempt`; a store::deadline call made in a try, on its helper, counts
  as that try): nested, it would wait on the lane the outer try holds, for good on
  the same thread. Only the build's schedule looks at the share before its last try (and waits up
  to a minute for it); the server's 5 s never does. Writes, removes, renames, create-new and appends are never retried here: a lost
  answer can't tell made from not made. `nasfs::RETRY` turns the widening off without a revert.
- **The I/O pool** (store::iopool, the server's and the mirror's) takes `NasPath`s. Its reads are
  counted and retried on the root's schedule. Its operations are deadline calls (§6), its own
  timeout each, at most `calls` at once (8 for the map's data); its breaker is its own.
- **Counting.** Every method counts into store::nasio. Sites counted by hand before keep the same
  totals: `Out`'s uploads (store::naming, raw) count their bytes as each write goes
  (`nasio::written`) and the file as it lands (`NasPath::count_made`), as every upload does (a
  whole copy to the NAS, a NAS-to-NAS copy, a file written through `NasFile`: a job's beats see a
  long upload move, task #990; a file written in one call counts at its end),
  buildstore::localread's whole copies (store::blobs) count their bytes as they're read, as every
  whole copy does (`nasfs::copy_counter`: a job's beats see a long copy move, task #986), and the
  file once made (`count_opened`). `costin::meta` and
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

- `Out::path`'s run-time refusal of inputs-folder paths stays as it was (docs/inputs.md §7.1); it did
  not become a typed `inputs()` accessor.
- Errors carry the path through `display()`, as before; they don't carry `nas:<rel>` and the
  operation.
- `store::catalogue`'s writers (`write`, `write_delta_of`) still take a `Path`; the lead's catalogue
  publish passes the NAS folder through `unchecked_path()`. Its readers take a `Loc`.
- The step programs' NAS inputs (osmium's, extract's, the trees program's, GDAL's, basepart's,
  railfeeds', lakefill's, the names crate's tables) are paths on their command lines or in their
  readers: listed in `EXEMPT`, not wrapped.

## 6. One deadline (`crates/store/src/deadline.rs`, task #1141)

An SMB call can stay in the kernel for hours, answering nothing and failing nothing: on 11 Nov two
unit jobs sat in `pread` for 40 minutes and more (task #1139), `scenic gc`'s queue removal sat in
`unlink` for 63 minutes on the mini (a SIGTERM ended it at once), and the regions check waited
15 minutes in a `NasPath::read` that then returned. Every NAS call of a job, the CLI, the agent and
the server is made through `store::deadline`, one mechanism:

- **The call** runs on a helper thread its calling thread keeps; the caller waits for it its
  deadline at most, counting only the time the process ran (a look at it that comes back over four
  times later than asked counts as one: the guards' freeze, SIGSTOP; `Instant` doesn't count the
  Mac asleep). Past it the caller gets `TimedOut` (`deadline::past_deadline`), logged with the
  path and the call; the helper is left to the kernel, its result going nowhere when it comes, and
  the caller's next call gets a new helper. A call made inside another (on a helper) runs there,
  under the outer one's deadline, and the paths it changes are the outer one's too (held with it
  when it's left, or at once when it already was: a write inside a read-labelled call, a remove
  under `nasread::with`, a `Deadlined` write keyed its own way). A deadline's `TimedOut` is never retried (`retry::transient`).
- **Deadlines.** `LONG` (120 s) for the build's roots (jobs, the CLI, the agent's own threads),
  `SHORT` (20 s) for the server's and the agent loop's (crate agentcore's `nasread::DEADLINE`); a
  positioned read `LONG` and a second a MB (`deadline::for_bytes`); the server's I/O pool its own
  (8 s for the map's data). A call that says it moves (`Progress::add`: a whole read's, a listing's,
  a copy's, a read-back's, a base pack's ranges') has its deadline counted from its last move, so
  one that goes on is never ended for its length. One that moves at all within each deadline (a
  `read` giving bytes, an entry listed, a name removed) holds its caller until it ends, however
  slowly it goes: wanted, a slow share's copy isn't a stuck one, and the file's or folder's size
  bounds it.
- **A cap** on the calls a process holds past their deadline: at it, a call fails at once, never
  made (`deadline::refused`, `TimedOut` too), so stuck threads never pile up. Each Mac's setting,
  "NAS calls held" on the build page (1 to 1,000, default 32: `deadline::CAP_DEFAULT`), kept in
  the agent's folder (`nas-held.json`); the agent applies it each round and gives it to each job
  as it starts (`SCENIC_NAS_HELD`), the CLI reads it as it starts, the map server follows it
  (`deadline::follow_cap`, every 10 s). The NAS calls chart shows the
  pool's caps summed beside its calls in flight.
- **A job's end.** A process that left calls in the kernel may never finish exiting: the kernel
  ends it once each of its threads has left its call, and its group's head (the shell that writes
  its exit status) waits for that. A job (`scenic-build`, `scenic`) ending with calls held writes
  its status itself first (`jobrt::control::ending`, to the path its agent gives it,
  `SCENIC_JOB_STATUS`, store::spawn::JOB_STATUS: the job's own process alone, never inherited by
  a program it starts, so a tool's end, `scenic-build tool trees`' or extract's, never says the
  job's); the agent takes that as the job's end (`jobs::Running::poll`), records it,
  and stops what of the job still runs, a member that won't go killed again each round
  (`jobs::Outlived`).

**Why a call left to the kernel is harmless when it finishes late.** Leaving a call changes nothing
it does on the share: it lands, or doesn't, as it would have with its caller waiting. What changes
is what this process does next, so the danger is only a later call of this process racing the
late one. A call that changes the share names the paths it changes; while one is held, another
change of this process to any of them is refused at once (`deadline::writing`). By class:

- **Reads, stats and listings** change nothing; a late answer goes nowhere. A `PlainFile`'s
  descriptor held by a late read is let go, the next read opening the file again.
- **Whole writes** (`write_whole`, `copy_from_local`, `copy_to_local`, blobs' copies) write a
  temporary name of their own call and rename it into place: a late rename puts this content
  where a later write of this process can't go meanwhile (its path held), so it never lands over
  a newer write of this process; another Mac's write is as racy as it ever was. A temporary file
  left is a temporary name (`whole::is_tmp`), which GC sweeps.
- **In-place writes** (`write`, a `NasFile`'s writes, sync and close, `open_with`'s create or
  truncate): held by their path, so no later write of this process interleaves with a late one.
- **Content-named uploads** (store::naming: each stat, open, 4 MB piece, sync, close, read-back,
  rename and the temporary file's removal a call of its own): a late piece lands only in this
  process's temporary name, held meanwhile; a late exclusive rename lands only the same
  content-named bytes, which a name never changes from. A write the share loses for a moment (an
  OS error, ETIMEDOUT or EIO, at its open, a write, its sync or its close) is written again, as a
  whole write's temporary file is (`whole::lost_write`); one past its deadline is never: its
  `TimedOut` has no OS code, and its temporary name stays held.
- **Create-new** (the pool protocol's): past its deadline its caller can't tell made from not made,
  as with a lost answer, which the protocol already takes (`Created`); never retried here.
- **Renames** hold both names. **Removes** (`remove_file`, `remove_dir`, `remove_dir_all`, a
  temporary file's) are idempotent: a late one removes what was to be removed; a later write of
  this process to the name waits out the hold, so a late remove never takes a newer file of this
  process's. **Folders made** (`create_dir_all`) and **touches** (`set_modified`) are idempotent.

**Every NAS writer closes its file checked** (`store::sys::close`, task #1142): the Mac's SMB
client keeps a pageout's write error for the close alone (its fsync doesn't return it), and a
dropped `File` ignores the close's result. Content-named uploads, whole writes and copies, a
`NasFile` (`close`: a writer ends with it; a drop only logs the error), an in-place `write`, a
create-new, the I/O pool's writes, `sys::copy_data`, the NAS budget's probe and `bld-fetch`'s
appends (one whose close fails is removed, so its next attempt starts afresh instead of going on
from bytes the share may not hold).

**Still under no deadline**, each a library's or a program's own reading: a `File` taken out of
the type (`NasFile::into_file`, `Loc::open`: a library that wants a `File`, such as a TIFF's or a
land table's reader), and the paths listed in the guard's `EXEMPT` (§3: a step program's
arguments, GDAL's, osmium's and the other libraries' own opens, the catalogue writers, a mapped
file's pages). A call held there holds its job; the agent's stuck-job flag says so.

**Cost.** Measured on the M4 (load 16-28, release build, a 64 MB file in the page cache): a 16 KB
`pread` inline 1.4 µs, through the old kept helper (`range::bounded`) 6.3-7.1 µs, through
`deadline::call` 6.4-7.3 µs; at 8 threads at once 25.6 and 25.7 µs (inline 7.0), at 16, 51.1 and
50.3 µs; a local stat 1.0 µs inline, `NasPath::metadata` 7.1-7.7 µs; the server's pool's 16 KB read
9.0 µs before (its worker threads) and 6.4 µs now; 64 MB read through a `NasFile` in 8 KB reads
4.7-5.5 ms, the `File` itself 6.3-6.9 ms (its reads come a MB a call).
