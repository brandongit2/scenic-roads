// Passenger rail: colour schemes, metrics and the scenic factors of a ride. Channels are the same
// per-vertex scenic channels as roads (roadcore::scenic::ch), measured from a carriage window
// (2.8 m); engineering factors come from the track itself (elevation, grade, bridges, tunnels).
// Component normalisation must match the rail branch of metric() in roads/layer.ts.

import { u8Area } from './scenic';

export type RailColour = 'line' | 'group' | 'metric' | 'single';
export type RailMetric = 'rscore' | 'freq' | 'elev' | 'grade' | 'viaduct' | 'view' | 'water' | 'vista' | 'drama' | 'ridge' | 'curvy';

/** Trains a day each way, as a byte per line (log scale, 0.1 to 1000 a day; 0 = unknown). */
export const freqCode = (perDay: number) => (perDay > 0 ? 1 + Math.round(Math.max(0, Math.min(1, (Math.log10(perDay) + 1) / 4)) * 254) : 0);
export const freqOfCode = (c: number) => (c > 0 ? 10 ** (((c - 1) / 254) * 4 - 1) : -1);
export const fmtTrains = (v: number) => (v >= 10 ? `${Math.round(v)}` : v >= 1 ? `${+v.toFixed(1)}` : `${+(v * 7).toFixed(1)}/wk`);

export interface RailMetricDef {
  key: RailMetric;
  label: string;
  /** Shader mode id (roads/layer.ts). */
  id: number;
  unit: string;
  range: [number, number];
  domain: [number, number];
  step: number;
  diverging?: boolean;
  help: string;
  fmt: (v: number) => string;
}

const n0 = (v: number) => Math.round(v).toLocaleString('en-CA');
const km2 = (v: number) => (v < 1 ? v.toFixed(2) : v < 10 ? v.toFixed(1) : n0(v)) + ' km²';

export const RAIL_METRICS: RailMetricDef[] = [
  { key: 'rscore', label: 'Scenic score', id: 30, unit: '', range: [0, 100], domain: [0, 100], step: 1, help: 'Weighted blend of the ride factors below.', fmt: (v) => v.toFixed(0) },
  { key: 'freq', label: 'Service frequency', id: 32, unit: '/day', range: [0, 2.5], domain: [-1, 3], step: 0.05, help: 'Trains a day each way on a typical weekday (log scale), all services on the track added up, from operators\u2019 published timetables. Grey: no timetable found.', fmt: (v) => `${fmtTrains(10 ** v)} a day` },
  { key: 'elev', label: 'Track elevation', id: 0, unit: 'm', range: [0, 1500], domain: [-50, 3500], step: 10, help: 'Metres above sea level.', fmt: (v) => `${n0(v)} m` },
  { key: 'grade', label: 'Gradient', id: 1, unit: '%', range: [0, 6], domain: [0, 30], step: 0.5, help: 'Track gradient over ~50 m (adhesion railways stay under ~4 %; rack railways go far steeper).', fmt: (v) => `${v.toFixed(1)} %` },
  { key: 'viaduct', label: 'Viaduct height', id: 31, unit: 'm', range: [0, 60], domain: [0, 150], step: 1, help: 'Height of bridges and viaducts above the ground beneath.', fmt: (v) => `${n0(v)} m` },
  { key: 'view', label: 'Views', id: 4, unit: 'km²', range: [0, 1], domain: [0, 1], step: 0.01, help: 'Area visible within 15 km from a carriage window.', fmt: (v) => km2(u8Area(v * 255)) },
  { key: 'water', label: 'Water views', id: 5, unit: 'km²', range: [0, 1], domain: [0, 1], step: 0.01, help: 'Lake, river and sea area visible within 15 km.', fmt: (v) => km2(u8Area(v * 255)) },
  { key: 'vista', label: 'Vista distance', id: 6, unit: 'km', range: [0, 12], domain: [0, 15], step: 0.1, help: 'Average farthest visible distance over all directions.', fmt: (v) => `${v.toFixed(1)} km` },
  { key: 'drama', label: 'Mountains', id: 7, unit: 'm', range: [0, 500], domain: [0, 765], step: 5, help: 'Relief of the terrain within 3 km.', fmt: (v) => `${n0(v)} m` },
  { key: 'ridge', label: 'Ledge ↔ gorge', id: 8, unit: 'm', range: [-60, 60], domain: [-256, 254], step: 2, diverging: true, help: 'Track above (+) or below (−) the surrounding terrain within 1.5 km.', fmt: (v) => `${v > 0 ? '+' : ''}${n0(v)} m` },
  { key: 'curvy', label: 'Curvature', id: 9, unit: '°/km', range: [0, 300], domain: [0, 1020], step: 5, help: 'Turning per kilometre over ±250 m.', fmt: (v) => `${n0(v)} °/km` },
];
export const railMetricDef = (k: RailMetric) => RAIL_METRICS.find((m) => m.key === k) ?? RAIL_METRICS[0];

export interface RailComponent {
  key: string;
  label: string;
  help: string;
  short: string;
  unit: string;
  text: (c: RailSample) => string;
}

/** What the hover readouts and the CPU metric need from one track vertex. */
export interface RailSample {
  elev: number;
  grade: number;
  /** Ground beneath (m): the drape height; under bridges, the ground below the deck. */
  ground: number;
  bridge: boolean;
  tunnel: boolean;
  ch: ArrayLike<number>;
  /** Trains a day each way (-1: no timetable). */
  freq: number;
}

const signed = (v: number) => `${v > 0 ? '+' : v < 0 ? '−' : ''}${n0(Math.abs(v))}`;
const area = (v: number) => (v < 1 ? v.toFixed(2) : v < 10 ? v.toFixed(1) : n0(v));
const viaduct = (s: RailSample) => (s.bridge && !s.tunnel ? Math.max(0, s.elev - s.ground) : 0);

export const RAIL_COMPONENTS: RailComponent[] = [
  { key: 'views', label: 'Views', short: 'Views', unit: 'km²', help: 'Visible area from the window (trees & terrain block the view)', text: (s) => area(u8Area(s.ch[0])) },
  { key: 'water', label: 'Water in view', short: 'Water', unit: 'km²', help: 'Visible lake / river / sea area', text: (s) => area(u8Area(s.ch[1])) },
  { key: 'vista', label: 'Long vistas', short: 'Vista', unit: 'km', help: 'Farthest visible distance (full at 15 km)', text: (s) => (s.ch[8] / 17).toFixed(1) },
  { key: 'relief', label: 'Mountains', short: 'Mountains', unit: 'm', help: 'Terrain relief within 3 km (full at 600 m)', text: (s) => n0(s.ch[2] * 3) },
  { key: 'ledge', label: 'Ledges & gorges', short: 'Ledge', unit: 'm', help: 'Running high on a slope or deep in a gorge (full at ±60 m from the surroundings)', text: (s) => signed((s.ch[3] - 128) * 2) },
  { key: 'altitude', label: 'Altitude', short: 'Altitude', unit: 'm', help: 'Track elevation (full at 1,500 m)', text: (s) => n0(s.elev) },
  { key: 'viaduct', label: 'Viaducts', short: 'Viaduct', unit: 'm', help: 'Height above the ground on bridges and viaducts (full at 40 m)', text: (s) => n0(viaduct(s)) },
  { key: 'tunnel', label: 'Tunnels', short: 'Tunnel', unit: '', help: 'Underground running (negative: you see nothing)', text: (s) => (s.tunnel ? 'yes' : 'no') },
  { key: 'gradient', label: 'Gradient', short: 'Gradient', unit: '%', help: 'Climbing and descending (full at 4 %; rack railways max out)', text: (s) => s.grade.toFixed(1) },
  { key: 'curvy', label: 'Curves', short: 'Curves', unit: '°/km', help: 'Curvature (full at 400 °/km)', text: (s) => n0(s.ch[4] * 4) },
  { key: 'freq', label: 'Service frequency', short: 'Trains', unit: '/day', help: 'Trains a day each way (full at 100 a day, log scale); tracks without a timetable leave it out of the score', text: (s) => (s.freq >= 0 ? fmtTrains(s.freq) : '–') },
];
export const RNCOMP = RAIL_COMPONENTS.length;
/** Index of the service-frequency factor (left out of the score where unknown). */
export const RAIL_FREQ = 10;

/** Components (0..1, RAIL_COMPONENTS order). */
export function railComponents(s: RailSample): number[] {
  const c = s.ch;
  return [
    c[0] / 255,
    c[1] / 255,
    c[8] / 255,
    Math.min(1, (c[2] * 3) / 600),
    Math.min(1, (Math.abs(c[3] - 128) * 2) / 60),
    Math.max(0, Math.min(1, s.elev / 1500)),
    Math.min(1, viaduct(s) / 40),
    s.tunnel ? 1 : 0,
    Math.min(1, s.grade / 4),
    Math.min(1, (c[4] * 4) / 400),
    s.freq > 0 ? Math.max(0, Math.min(1, Math.log10(Math.max(s.freq, 1)) / 2)) : 0,
  ];
}

export function railScore(s: RailSample, w: number[]): number {
  const comp = railComponents(s);
  const pos = w.reduce((a, v, i) => a + (i === RAIL_FREQ && s.freq < 0 ? 0 : Math.max(v, 0)), 0) || 1e-6;
  return Math.max(0, Math.min(1, comp.reduce((a, v, i) => a + v * w[i], 0) / pos)) * 100;
}

export function railMetricOf(m: RailMetric, s: RailSample, w: number[]): number {
  switch (m) {
    case 'rscore': return railScore(s, w);
    case 'freq': return s.freq > 0 ? Math.log10(s.freq) : NaN;
    case 'elev': return s.elev;
    case 'grade': return s.grade;
    case 'viaduct': return viaduct(s);
    case 'view': return s.ch[0] / 255;
    case 'water': return s.ch[1] / 255;
    case 'vista': return s.ch[8] / 17;
    case 'drama': return s.ch[2] * 3;
    case 'ridge': return (s.ch[3] - 128) * 2;
    case 'curvy': return s.ch[4] * 4;
  }
}

/** Colours per rail group (tram, metro, commuter, intercity, heritage) for "by service". */
export const RAIL_GROUP_COLOURS = ['#e0b43c', '#3fa9f5', '#8fd14f', '#f2555a', '#c77dff'];
