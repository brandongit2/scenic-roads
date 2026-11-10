// The build page's words for numbers, one way for every module (and the dev job dashboard's, which
// imports it: tools/devtools serves it at /lib/buildpage/).

/** A count as the page writes it: "12,480". */
export const n = (v) => Number(v).toLocaleString("en-US");

/** A count and its noun: "1 job", "12,480 z6 tiles"; `many` where it isn't `one` + "s". */
export const plural = (k, one, many = `${one}s`) => `${n(k)} ${k === 1 ? one : many}`;

/** Two digits: "07". */
export const pad = (v) => String(v).padStart(2, "0");
