// Weight presets for the scenic score (roads) and the ride score (rail). The built-in ones are
// data (data/weight-presets.json, data/rail-presets.json: weights keyed by component). Once a list
// is edited in the app (save, save as, rename, delete), the edited list is kept in localStorage
// (presets.roads, presets.rail) and used instead; "Built-ins" goes back to the file's. A stored
// list is read whole or not at all: one that isn't a list of presets, or where a preset weighs a
// factor the app doesn't have (an older name: the rest of its weights would mean something else)
// or holds a weight that isn't a number, gives the built-ins. A factor a preset doesn't list
// weighs 0 (a factor at 0 isn't stored).
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

type Stored = { id: string; name: string; weights: Record<string, unknown> };
type Comp = { key: string };

export const sameWeights = (a: number[], b: number[]) => a.length === b.length && a.every((v, i) => Math.abs(v - b[i]) < 1e-6);

export class PresetList {
  list: Preset[];
  /** True once edited in the app (the list then comes from localStorage). */
  custom: boolean;
  readonly builtin: Preset[];
  private ls = new Set<() => void>();

  constructor(private key: string, private comps: Comp[], data: unknown) {
    this.builtin = this.parse(data)!;
    const saved = this.parse(prefs.load<unknown>(key, null));
    this.custom = !!saved;
    this.list = saved ?? this.builtin.map((p) => ({ ...p, w: [...p.w] }));
  }

  /** A preset's weights in the factors' order (one not listed: 0), or null when it lists a factor
   * the app doesn't have or a weight that isn't a number. */
  private toVec(ws: Record<string, unknown>): number[] | null {
    const keys = new Set(this.comps.map((c) => c.key));
    if (Object.entries(ws).some(([k, v]) => !keys.has(k) || typeof v !== 'number' || !Number.isFinite(v))) return null;
    return this.comps.map((c) => (ws[c.key] as number | undefined) ?? 0);
  }
  private toMap(w: number[]) {
    return Object.fromEntries(this.comps.map((c, i) => [c.key, +w[i].toFixed(2)] as const).filter(([, v]) => v !== 0));
  }
  private parse(list: unknown): Preset[] | null {
    if (!Array.isArray(list)) return null;
    const out: Preset[] = [];
    for (const p of list as Stored[]) {
      if (!p || typeof p.id !== 'string' || typeof p.name !== 'string' || !p.weights || typeof p.weights !== 'object' || Array.isArray(p.weights)) return null;
      const w = this.toVec(p.weights);
      if (!w) return null;
      out.push({ id: p.id, name: p.name, w });
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
    this.ls.forEach((f) => f());
  }

  private commit() {
    this.custom = true;
    prefs.save(this.key, this.list.map((p): Stored => ({ id: p.id, name: p.name, weights: this.toMap(p.w) })));
    this.ls.forEach((f) => f());
  }
}

export const presets = new PresetList('presets.roads', COMPONENTS, builtin.presets);

export const railPresets = new PresetList('presets.rail', RAIL_COMPONENTS, railBuiltin.presets);
