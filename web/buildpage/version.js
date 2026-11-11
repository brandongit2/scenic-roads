// An open page reloading into a newly published version of itself (docs/coordinator.md §7, The
// page's version). The coordinator puts its page's version (the hash of the page's files) in the
// page it serves (`served`) and on every answer (`HEADER`); a page open for days is told by its next
// answer.
export const HEADER = "x-page-version";

// A function for the headers of each answer (`Headers`) that calls `reload` when the answer names
// a version other than `served`: not when it names none (the lead away, a proxy's 502, an answer
// from elsewhere), nor when the page was served without one. `tried` ({get, set}, kept across a
// reload) holds the version reloaded for: a reload that still isn't the version (a cached page)
// isn't repeated for it.
export function versionWatch(served, reload, tried) {
  return (headers) => {
    const v = headers.get(HEADER);
    if (!v || !served || v === served || tried.get() === v) return;
    tried.set(v);
    reload();
  };
}
