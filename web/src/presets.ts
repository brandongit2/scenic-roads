// Weight presets for the scenic score (roads) and the ride score (rail). The built-in ones are
// data (data/weight-presets.json, data/rail-presets.json: weights keyed by component). Once a list
// is edited in the app (save, save as, rename, delete), the edited list is kept in localStorage
// (presets.roads, presets.rail) and used instead; "Built-ins" goes back to the file's. A list
// saved under the older keys (weightPresets, railWeightPresets) is read until the next edit.
import builtin from './data/weight-presets.json';
import railBuiltin from './data/rail-presets.json';
import * as prefs from './prefs';
import { RAIL_COMPONENTS } from './rail';
import { COMPONENTS } from './scenic';

export interface Preset {
  id: string;
  name: string;
  /** Weights in the components' order. */
  w: number[];
}

type Stored = { id: string; name: string; weights: Record<string, number> };
type Comp = { key: string; was?: string; def?: number };

export const sameWeights = (a: number[], b: number[]) => a.length === b.length && a.every((v, i) => Math.abs(v - b[i]) < 1e-6);

export class PresetList {
  list: Preset[];
  /** True once edited in the app (the list then comes from localStorage). */
  custom: boolean;
  readonly builtin: Preset[];
  private ls = new Set<() => void>();

  /** `old`: the localStorage key an older app kept the edited list under, and the factors whose
   * def it didn't have (it left them out at 0, as any factor without one). Every write here removes
   * that key, so when it is there it is the latest write (an older app's tab, still open, edited or
   * went back to the built-ins since) and is read over this key's. */
  constructor(private key: string, private comps: Comp[], data: unknown, private old: { key: string; noDef?: string[] }) {
    this.builtin = this.parse(data)!;
    const was = prefs.load<unknown>(old.key, undefined);
    const saved = was !== undefined ? this.parse(was, old.noDef) : this.parse(prefs.load<unknown>(key, null));
    this.custom = !!saved;
    this.list = saved ?? this.builtin.map((p) => ({ ...p, w: [...p.w] }));
  }

  // A factor added later (with a default) keeps its default in presets saved before it existed, so
  // it is stored even at 0. A factor's older key (`was`) is read as its own.
  private toVec(ws: Record<string, number>, noDef: string[] = []) {
    return this.comps.map((c) => {
      const v = Number(c.key in ws || c.was === undefined ? ws[c.key] : ws[c.was]);
      return Number.isFinite(v) ? v : noDef.includes(c.key) ? 0 : c.def ?? 0;
    });
  }
  private toMap(w: number[]) {
    return Object.fromEntries(this.comps.map((c, i) => [c.key, +w[i].toFixed(2), c.def !== undefined] as const).filter(([, v, keep]) => v !== 0 || keep).map(([k, v]) => [k, v]));
  }
  private parse(list: unknown, noDef?: string[]): Preset[] | null {
    if (!Array.isArray(list)) return null;
    const out: Preset[] = [];
    for (const p of list as Stored[]) {
      if (!p || typeof p.id !== 'string' || typeof p.name !== 'string' || !p.weights || typeof p.weights !== 'object') return null;
      out.push({ id: p.id, name: p.name, w: this.toVec(p.weights, noDef) });
    }
    return out;
  }

  on(f: () => void) {
    this.ls.add(f);
  }

  get(id: string): Preset | undefined {
    return this.list.find((p) => p.id === id);
  }

  /** Overwrite a preset's weights. */
  save(id: string, w: number[]) {
    const p = this.get(id);
    if (!p) return;
    p.w = [...w];
    this.commit();
  }

  /** New preset (after the selected one); returns its id. */
  add(name: string, w: number[], after?: string): string {
    const base = name.toLowerCase().normalize('NFKD').replace(/[^\w]+/g, '-').replace(/^-|-$/g, '') || 'preset';
    let id = base;
    for (let i = 2; this.get(id) || id === 'custom'; i++) id = `${base}-${i}`;
    const at = after ? this.list.findIndex((p) => p.id === after) + 1 : this.list.length;
    this.list.splice(at > 0 ? at : this.list.length, 0, { id, name, w: [...w] });
    this.commit();
    return id;
  }

  rename(id: string, name: string) {
    const p = this.get(id);
    if (!p) return;
    p.name = name;
    this.commit();
  }

  remove(id: string) {
    this.list = this.list.filter((p) => p.id !== id);
    this.commit();
  }

  restoreBuiltins() {
    this.list = this.builtin.map((p) => ({ ...p, w: [...p.w] }));
    this.custom = false;
    prefs.save(this.key, null);
    prefs.remove(this.old.key);
    this.ls.forEach((f) => f());
  }

  private commit() {
    this.custom = true;
    prefs.save(this.key, this.list.map((p): Stored => ({ id: p.id, name: p.name, weights: this.toMap(p.w) })));
    prefs.remove(this.old.key);
    this.ls.forEach((f) => f());
  }
}

/** A built-in list's preset the file calls its `default`, else its first (a link from before the
 * rail's preset was named is read as this preset when it carries its weights). */
const defaultOf = (list: Preset[], id: string | undefined) => list.find((p) => p.id === id) ?? list[0];

export const presets = new PresetList('presets.roads', COMPONENTS, builtin.presets, { key: 'weightPresets' });
export const BUILTIN: Preset[] = presets.builtin;

export const railPresets = new PresetList('presets.rail', RAIL_COMPONENTS, railBuiltin.presets, { key: 'railWeightPresets', noDef: ['freq'] });
export const RAIL_DEFAULT_PRESET = defaultOf(railPresets.builtin, railBuiltin.default).id;
