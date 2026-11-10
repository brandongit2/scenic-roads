// The build page's elements, one way for every module (and the dev job dashboard's, which imports
// it: tools/devtools serves it at /lib/buildpage/).

/** An element: `h("div", "cls", ...kids)` or `h("div", {class, style, title, onclick, …}, ...kids)`.
 * An attribute null or false is left out, true is set empty; `style` an object (custom properties,
 * `--x`, too) or a string; `on…` a listener. Kids (arrays at any depth) null or false are left out,
 * the rest a node or text. */
export function h(tag, attrs, ...kids) {
  const e = document.createElement(tag);
  if (typeof attrs === "string") e.className = attrs;
  else if (attrs) for (const [k, v] of Object.entries(attrs)) {
    if (v == null || v === false) continue;
    if (k === "class") e.className = v;
    else if (k === "style" && typeof v === "object") for (const [p, x] of Object.entries(v)) p.startsWith("--") ? e.style.setProperty(p, x) : (e.style[p] = x);
    else if (k.startsWith("on")) e.addEventListener(k.slice(2), v);
    else e.setAttribute(k, v === true ? "" : v);
  }
  for (const k of kids.flat(Infinity)) if (k != null && k !== false) e.append(k instanceof Node ? k : String(k));
  return e;
}

/** An element with a class and its text: `el("span", "k", "load")`. */
export const el = (tag, cls, text) => { const e = document.createElement(tag); if (cls) e.className = cls; if (text != null) e.textContent = text; return e; };

/** A colour token's value (index.html's `--name`, as the theme has it now), read at `at` (the page's root). */
export const token = (k, at = document.documentElement) => getComputedStyle(at).getPropertyValue(k).trim();
