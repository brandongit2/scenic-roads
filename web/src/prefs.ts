// Persistent settings in localStorage. Every access is guarded: storage can be unavailable
// (private windows, blocked site data) and the app must work without it.

const NS = 'scenic-roads:';

export function load<T>(key: string, fallback: T): T {
  try {
    const v = localStorage.getItem(NS + key);
    return v === null ? fallback : (JSON.parse(v) as T);
  } catch {
    return fallback;
  }
}

export function save(key: string, value: unknown) {
  try {
    localStorage.setItem(NS + key, JSON.stringify(value));
  } catch {
    /* storage unavailable or full */
  }
}

const timers = new Map<string, number>();

/** save(), coalesced over `ms` (for state that changes on every frame of a drag). */
export function saveSoon(key: string, value: () => unknown, ms = 300) {
  clearTimeout(timers.get(key));
  timers.set(key, window.setTimeout(() => save(key, value()), ms));
}
