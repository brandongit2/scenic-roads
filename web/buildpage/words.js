// The build page's words for numbers, one way for every module (and the dev job dashboard's, which
// imports it: tools/devtools serves it at /lib/buildpage/).

/** A count as the page writes it: "12,480". */
export const n = (v) => Number(v).toLocaleString("en-US");

/** A count and its noun: "1 job", "12,480 z6 tiles"; `many` where it isn't `one` + "s". */
export const plural = (k, one, many = `${one}s`) => `${n(k)} ${k === 1 ? one : many}`;

/** Two digits: "07". */
export const pad = (v) => String(v).padStart(2, "0");

/** A Mac's speed (the lead's `speeds.macs[host]`: jobrt::timings::MacSpeed, task #1418) in words:
 * "8.1 G instructions/s a performance core, 2.9 an efficiency core · over 4 steps every Mac ran
 * (63 clean runs)", the steps and runs its performance cores' (else its efficiency cores'); null
 * with neither measured. Pure (web/test/words.test.mjs). */
export function speedWords(s) {
  const kinds = [["p", "a performance core"], ["e", "an efficiency core"]].filter(([k]) => s?.[k]);
  if (!kinds.length) return null;
  const rate = kinds.map(([k, core], i) => `${s[k].ginsn_s.toFixed(1)}${i ? "" : " G instructions/s"} ${core}`).join(", ");
  const o = s[kinds[0][0]];
  return `${rate} · over ${plural(o.steps.length, "step")}${o.shared ? " every Mac ran" : " of its own"} (${plural(o.runs, "clean run")})`;
}
