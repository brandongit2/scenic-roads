// Bottom strip. Row 1: the hovered road — name, elevation, grade, then the scenic score and each
// measured score factor as a bar (full = the factor at its cap), then roadside tree height. A rail
// line shows its services and the ride factors in the same slots; a marker or highlighted area
// (with no road under the cursor) gets one descriptive line.
// Row 2: yes/no flags as chips, road tags and elevation sources on the left; loading status,
// cursor position, zoom, scale, links and the credits dialog on the right. Every slot has a fixed
// width and its elements persist, so nothing moves as the values change.
import type { Map as MLMap } from 'maplibre-gl';
import { CLASS_LABELS, RAIL0, RAIL_GROUPS, ST_BRIDGE, ST_LINK, ST_TUNNEL, ST_UNPAVED } from '../config';
import type { FeatureSummary } from '../overlays';
import { RAIL_COMPONENTS, fmtTrains, railComponents, railScore, type RailSample } from '../rail';
import type { HoverInfo } from '../roads/layer';
import type { WayInfo } from '../api';
import { COMPONENTS, FLAG_LABELS, components, scoreOf } from '../scenic';
import { cap, fmt, h, toast } from './dom';

const VALUE_FONT = '600 10px -apple-system, system-ui, sans-serif';
let measureCtx: CanvasRenderingContext2D | null = null;
const textWidth = (t: string) => {
  measureCtx ??= document.createElement('canvas').getContext('2d')!;
  measureCtx.font = VALUE_FONT;
  return measureCtx.measureText(t).width;
};

/** A labelled bar whose value sits at the tip of the fill: inside it if it fits, else just past it. */
class Bar {
  readonly el: HTMLDivElement;
  private track: HTMLDivElement;
  private fill: HTMLElement;
  private val: HTMLSpanElement;

  private lbl: HTMLSpanElement;

  constructor(label: string, private unit: string, private title: string, cls = '') {
    this.fill = h('i');
    this.val = h('span', { class: 'bv' });
    this.track = h('div', { class: 'track' }, this.fill, this.val);
    this.lbl = h('span', { class: 'lbl' });
    this.el = h('div', { class: `bar ${cls}`, title }, this.lbl, this.track);
    this.label(label, unit, title);
  }

  /** Relabel (the same slots show road or rail factors). */
  label(label: string, unit: string, title: string) {
    this.unit = unit;
    this.title = title;
    this.lbl.replaceChildren(label, unit ? h('span', { class: 'u' }, ` ${unit}`) : '');
  }

  set(frac: number, text: string, trackW: number, colour: string) {
    const f = Math.max(0, Math.min(1, frac));
    const fillPx = f * trackW;
    const tw = textWidth(text);
    this.fill.style.width = `${f * 100}%`;
    this.fill.style.background = colour;
    this.val.textContent = text;
    this.el.title = `${text}${this.unit ? ` ${this.unit}` : ''} · ${this.title}`;
    // Inside the fill, ending at its tip; else just past the tip; else (a narrow bar) not at all.
    const inside = fillPx >= tw + 6;
    const outside = !inside && fillPx + 3 + tw <= trackW - 2;
    this.val.classList.toggle('in', inside);
    this.val.classList.toggle('none', !inside && !outside);
    this.val.style.left = `${inside ? fillPx - tw - 3 : fillPx + 3}px`;
  }

  /** Width of the track (all bars share it). */
  get width() {
    return this.track.clientWidth;
  }
}

export class Strip {
  private r1: HTMLDivElement;
  private road: HTMLSpanElement;
  private elev: HTMLElement;
  private sw: HTMLSpanElement;
  private grade: HTMLElement;
  private trees: HTMLElement;
  private score: Bar;
  private bars: { i: number; bar: Bar }[];
  private info: HTMLSpanElement;
  private coord: HTMLSpanElement;
  private zoom: HTMLSpanElement;
  private scale: HTMLSpanElement;
  private loading: HTMLSpanElement;
  private credits: HTMLDialogElement;
  private last: { hov: HoverInfo; info: WayInfo | null | 'loading'; colour: string; areas: FeatureSummary[] } | null = null;
  private featLine: HTMLSpanElement;
  /** Which factors the bars currently show. */
  private barsFor: 'road' | 'rail' = 'road';
  /** Rail: the ride factors shown in the bar slots (tunnels go to row 2). */
  private readonly railBars = RAIL_COMPONENTS.map((_, i) => i).filter((i) => RAIL_COMPONENTS[i].key !== 'tunnel');
  railWeights: () => number[] = () => [];

  constructor(root: HTMLElement, private map: MLMap, private weights: () => number[]) {
    const idle = h('span', { class: 'hint' },
      'Hover a road for elevation & scenic metrics · click for its profile · two-finger drag pans, pinch zooms, ⌥ + two-finger drag or right-drag tilts & rotates around the cursor · G: Street View, M: Google Maps, O: OpenStreetMap at the cursor');
    const cell = (label: string, title: string) => {
      const b = h('b');
      return { el: h('div', { class: 'cell', title }, h('span', { class: 'lbl' }, label), b), b };
    };
    this.road = h('span', { class: 'road' });
    const e = cell('Elevation', 'Road elevation at the cursor');
    this.sw = h('span', { class: 'sw' });
    e.b.before(this.sw);
    this.elev = e.b;
    const g = cell('Grade', 'Road grade at the cursor');
    this.grade = g.b;
    const t = cell('Trees', 'Roadside tree height (p95 within 30 m)');
    this.trees = t.b;
    this.score = new Bar('Scenic', '', 'Scenic score with the current weights (0–100)', 'score');
    this.bars = COMPONENTS.flatMap((c, i) => (c.bar ? [{ i, bar: new Bar(c.bar.short, c.bar.unit, `${c.label}: ${c.help}`) }] : []));
    this.featLine = h('span', { class: 'featline' });
    this.r1 = h('div', { class: 'r1 idle' },
      idle, this.featLine, this.road, e.el, g.el, this.score.el, ...this.bars.map((b) => b.bar.el), t.el);

    this.info = h('span', { class: 'info' });
    this.coord = h('span', { class: 'coord num' });
    this.zoom = h('span', { class: 'zoom num' });
    this.scale = h('span', { class: 'scale' });
    this.loading = h('span', { class: 'status' });
    const copy = h('button', {
      title: 'Copy a link to this view',
      onclick: async () => {
        await navigator.clipboard.writeText(location.href);
        toast('Link copied');
      },
    }, 'Copy link');
    this.credits = creditsDialog();
    const credits = h('button', { title: 'Data sources, credits and licences', onclick: () => this.credits.showModal() }, '© Credits');
    root.append(
      this.r1,
      h('div', { class: 'r2' },
        this.info,
        h('span', { class: 'right' }, this.loading, this.coord, this.zoom, this.scale, copy, credits),
      ),
      this.credits,
    );
    map.on('move', () => this.view());
    map.on('mousemove', (ev) => (this.coord.textContent = fmt.coord(ev.lngLat.lat, ev.lngLat.lng)));
    // Bar widths change with the window: re-place the values.
    new ResizeObserver(() => this.last && this.show(this.last.hov, this.last.info, this.last.colour, this.last.areas)).observe(this.r1);
    this.view();
  }

  private view() {
    const z = this.map.getZoom();
    const c = this.map.getCenter();
    this.zoom.textContent = `z ${z.toFixed(1)}`;
    // Metric scale bar (~100 px).
    const mpp = (40075016.686 * Math.cos((c.lat * Math.PI) / 180)) / (512 * 2 ** z);
    const target = 100 * mpp;
    const p = 10 ** Math.floor(Math.log10(target));
    const nice = [1, 2, 5, 10].map((k) => k * p).filter((v) => v <= target).pop() ?? p;
    const bar = h('i');
    bar.style.width = `${nice / mpp}px`;
    this.scale.replaceChildren(bar, fmt.dist(nice));
  }

  setLoading(text: string) {
    this.loading.textContent = text;
  }

  /** A marker or highlighted area (no road under the cursor), with the other areas it lies in. */
  showFeature(f: FeatureSummary, areas: FeatureSummary[]) {
    this.last = null;
    this.r1.classList.remove('idle');
    this.r1.classList.add('feat');
    const dot = h('i', { class: f.area ? 'fa' : 'fp' });
    dot.style.background = f.colour;
    this.featLine.replaceChildren(dot, h('b', {}, cap(f.title)), h('span', { class: 'kind' }, f.kind), ...f.facts.map((x) => h('span', { class: 'fact' }, x)));
    this.featLine.title = [f.title, f.kind, ...f.facts].join(' · ');
    const others = areas.filter((a) => a.title !== f.title);
    if (f.also?.length) {
      const c = h('span', { class: 'chip', title: `Also here: ${f.also.join(' · ')} (click for all)` }, `+${f.also.length} here: ${f.also[0]}${f.also.length > 1 ? ' …' : ''}`);
      this.featLine.append(c);
    }
    const second = f.desc ? h('span', { class: 'src desc', title: cap(f.desc) }, cap(f.desc)) : f.source ? h('span', { class: 'src' }, `Source: ${f.source}`) : null;
    this.info.replaceChildren(...this.areaChips(others), ...(second ? [second] : []));
  }

  private areaChips(areas: FeatureSummary[]) {
    return areas.slice(0, 3).map((a) => {
      const c = h('span', { class: 'chip area', title: `${a.kind}${a.facts.length ? ' · ' + a.facts.join(' · ') : ''}` }, `in ${a.title}`);
      c.style.setProperty('--c', a.colour);
      return c;
    });
  }

  private relabel(to: 'road' | 'rail') {
    if (this.barsFor === to) return;
    this.barsFor = to;
    this.bars.forEach(({ i, bar }, k) => {
      if (to === 'road') {
        const c = COMPONENTS[i];
        bar.label(c.bar!.short, c.bar!.unit, `${c.label}: ${c.help}`);
        bar.el.style.visibility = '';
      } else {
        const c = RAIL_COMPONENTS[this.railBars[k]];
        if (c) bar.label(c.short, c.unit, `${c.label}: ${c.help}`);
        bar.el.style.visibility = c ? '' : 'hidden'; // rail has fewer factors than roads
      }
    });
    this.score.label('Scenic', '', to === 'road' ? 'Scenic score with the current weights (0–100)' : 'Ride score with the rail weights (0–100)');
  }

  show(hov: HoverInfo | null, info: WayInfo | null | 'loading', colour: string, areas: FeatureSummary[] = []) {
    this.r1.classList.remove('feat');
    if (!hov) {
      this.last = null;
      this.r1.classList.add('idle');
      this.info.replaceChildren();
      return;
    }
    this.last = { hov, info, colour, areas };
    this.r1.classList.remove('idle');
    const st = hov.style;
    if ((st & 15) >= RAIL0) return this.showRail(hov, info, colour, areas);
    this.relabel('road');

    // Row 1.
    this.road.replaceChildren();
    if (info === 'loading') this.road.append(h('span', { class: 'spin' }));
    else if (info) {
      if (info.ref) this.road.append(h('span', { class: 'ref' }, info.ref));
      this.road.append(cap(info.name) || (info.ref ? '' : 'Unnamed road'));
    }
    this.road.title = this.road.textContent ?? '';
    this.sw.style.background = colour;
    this.elev.textContent = fmt.m1(hov.elev);
    this.grade.textContent = fmt.pct(hov.grade);
    const c = hov.ch;
    const none = c.every((x) => x === 0); // no scenic analysis here
    const w = this.weights();
    const k = components(c);
    const tw = this.bars[0].bar.width, ts = this.score.width; // factor bars share one width
    const score = scoreOf(c, w);
    this.score.set(none ? 0 : score / 100, none ? '–' : score.toFixed(0), ts, '#f3c77a');
    for (const { i, bar } of this.bars) {
      bar.set(none ? 0 : k[i], none ? '–' : COMPONENTS[i].bar!.text(c), tw, w[i] < 0 ? '#ff8a7a' : 'var(--accent)');
    }
    this.trees.textContent = none ? '–' : `${Math.round(c[11] / 8)} m`;

    // Row 2: flags, tags, sources.
    const chips = FLAG_LABELS.filter(([m]) => c[7] & m).map(([m, l]) =>
      h('span', { class: 'chip' }, m === 1 && info && info !== 'loading' && info.route ? `★ ${info.route}` : l));
    const tags = [CLASS_LABELS[st & 15] + (st & ST_LINK ? ' link' : '')];
    if (st & ST_UNPAVED) tags.push('unpaved');
    if (st & ST_BRIDGE) tags.push('bridge');
    if (st & ST_TUNNEL) tags.push('tunnel');
    if (info && info !== 'loading') {
      if (info.surface && !(st & ST_UNPAVED)) tags.push(info.surface);
      if (info.maxspeed) tags.push(`${info.maxspeed} km/h`);
      if (info.lanes) tags.push(`${info.lanes} lanes`);
      if (info.oneway) tags.push('one-way');
      if (info.toll) tags.push('toll');
      if (info.covered) tags.push('covered bridge');
      if (info.route && !(c[7] & 1)) tags.push(`★ ${info.route}`);
    }
    const src = info && info !== 'loading' && info.sources.length ? info.sources.map(([s]) => s).join(' + ') : '';
    this.info.replaceChildren(...chips, ...this.areaChips(areas), h('span', { class: 'tags' }, tags.join(' · ')), ...(src ? [h('span', { class: 'src' }, src)] : []));
  }

  private showRail(hov: HoverInfo, info: WayInfo | null | 'loading', colour: string, areas: FeatureSummary[]) {
    this.relabel('rail');
    const st = hov.style;
    this.road.replaceChildren();
    if (info === 'loading') this.road.append(h('span', { class: 'spin' }));
    else if (info) {
      if (info.colour) {
        const sw = h('i', { class: 'line-sw' });
        sw.style.background = info.colour;
        this.road.append(sw);
      }
      this.road.append(cap(info.name) || info.route || CLASS_LABELS[st & 15]);
    }
    this.road.title = [info && info !== 'loading' ? info.route : '', this.road.textContent].filter(Boolean).join(' · ');
    this.sw.style.background = colour;
    this.elev.textContent = fmt.m1(hov.elev);
    this.grade.textContent = fmt.pct(hov.grade);
    const smp: RailSample = { elev: hov.elev, grade: hov.grade, ground: hov.ground ?? hov.elev, bridge: !!(st & ST_BRIDGE), tunnel: !!(st & ST_TUNNEL), ch: hov.ch, freq: hov.fq ?? -1 };
    const none = hov.ch.every((x) => x === 0);
    const w = this.railWeights();
    const k = railComponents(smp);
    const tw = this.bars[0].bar.width, ts = this.score.width;
    const score = railScore(smp, w);
    this.score.set(none ? 0 : score / 100, none ? '–' : score.toFixed(0), ts, '#f3c77a');
    this.bars.forEach(({ bar }, j) => {
      const i = this.railBars[j];
      const c = RAIL_COMPONENTS[i];
      if (!c) return;
      const measured = !none || ['altitude', 'viaduct', 'gradient'].includes(c.key);
      bar.set(measured ? k[i] : 0, measured ? c.text(smp) : '–', tw, (w[i] ?? 0) < 0 ? '#ff8a7a' : 'var(--accent)');
    });
    this.trees.textContent = none ? '–' : `${Math.round(hov.ch[11] / 8)} m`;
    const tags = [CLASS_LABELS[st & 15]];
    if (st & ST_BRIDGE) tags.push('bridge');
    if (st & ST_TUNNEL) tags.push('tunnel');
    tags.push(smp.freq > 0 ? `${hov.fqMin ? 'at least ' : ''}${fmtTrains(smp.freq)} trains a day each way` : 'no timetable');
    const chips: HTMLElement[] = [];
    if (info && info !== 'loading') {
      for (const g of info.rail ?? []) {
        const gi = RAIL_GROUPS.findIndex((x) => x.key === g);
        if (gi >= 0 && RAIL0 + gi !== (st & 15)) tags.push(RAIL_GROUPS[gi].label.toLowerCase());
      }
      if (info.route) chips.push(h('span', { class: 'chip rail', title: 'Services using this track (OSM route relations)' }, info.route));
      if (info.maxspeed) tags.push(`${info.maxspeed} km/h`);
    }
    const src = info && info !== 'loading' && info.sources.length ? info.sources.map(([s]) => s).join(' + ') : '';
    this.info.replaceChildren(...chips, ...this.areaChips(areas), h('span', { class: 'tags' }, tags.join(' · ')), ...(src ? [h('span', { class: 'src' }, src)] : []));
  }
}

/** Data sources and licences (from the README), in a modal dialog. */
function creditsDialog(): HTMLDialogElement {
  const rows: [string, string, string][] = [
    ['Roads, water, boundaries, places, parks, points of interest, Indigenous land boundaries', 'OpenStreetMap (Geofabrik extracts); basemap schema by OpenMapTiles', '© OpenStreetMap contributors, ODbL'],
    ['Road elevation, North America', 'NRCan HRDEM lidar → USGS 3DEP 10 m → NRCan MRDEM 30 m', 'OGL–Canada / public domain'],
    ['Road elevation, Europe & Hong Kong', 'FABDEM v1-2 30 m (University of Bristol / Fathom; Hawker et al. 2022). FABDEM is produced using Copernicus WorldDEM-30 © DLR e.V. 2010-2014 and © Airbus Defence and Space GmbH 2014-2018 provided under COPERNICUS by the European Union and ESA; all rights reserved.', 'CC BY-NC-SA 4.0 (non-commercial)'],
    ['3D terrain, hill-shading, contours, slope', 'Terrain Tiles (Terrarium) on AWS Open Data', 'Mapzen / various open sources'],
    ['Tree canopy height & cover (scenic factors, tree cover layer)', 'Meta & WRI global canopy height', 'CC BY 4.0'],
    ['Forest leaf type, Europe', '© European Union, Copernicus Land Monitoring Service 2018, European Environment Agency (EEA): High Resolution Layer Dominant Leaf Type', 'Copernicus free and open data policy (attribution)'],
    ['Forest leaf type, North America', '2020 Land Cover of North America (NALCMS): Commission for Environmental Cooperation; NRCan/CCRS, USGS, INEGI, CONAFOR', 'CEC terms of use (attribution)'],
    ['Land cover', 'ESA WorldCover 2021', 'CC BY 4.0'],
    ['UNESCO World Heritage', 'UNESCO World Heritage Centre, World Heritage List (data.unesco.org)', '© UNESCO World Heritage Centre, CC BY-SA 4.0'],
    ['France heritage', 'Ministère de la Culture, base Mérimée (POP); sites patrimoniaux remarquables via the Géoportail de l\'Urbanisme', 'Licence Ouverte 2.0'],
    ['Andorra heritage', 'Govern d\'Andorra, Inventari general del patrimoni cultural (IDE Andorra)', 'Private, personal use only'],
    ['Canadian federal designations', 'Parks Canada Directory of Federal Heritage Designations', 'OGL–Canada'],
    ['US designations', 'NPS National Register of Historic Places', 'Public domain'],
    ['Québec heritage', 'MCC Répertoire du patrimoine culturel', 'CC BY 4.0'],
    ['Ontario heritage', 'Ontario Heritage Act Register (Ontario Heritage Trust)', 'Personal non-commercial use only'],
    ['Nova Scotia heritage', 'Registered Heritage Properties; Halifax (HRM) municipal heritage', 'NS Open Government Licence; HRM open data'],
    ['NB, PEI, NL, western & northern Canada heritage', 'Canadian Register of Historic Places; Moncton open data', 'Non-commercial reproduction with credit'],
    ['Biosphere reserves, geoparks, dark-sky places', 'UNESCO MAB & Global Geoparks, DarkSky International, RASC', 'Facts from the official registries'],
    ['England heritage', '© Historic England, National Heritage List for England; contains Ordnance Survey data © Crown copyright and database right', 'OGL v3'],
    ['Scotland heritage', 'Contains Historic Environment Scotland and Ordnance Survey data © Historic Environment Scotland – Scottish Charity No. SC045925 © Crown copyright and database right', 'OGL v3'],
    ['Wales heritage', 'Designated Historic Asset GIS Data, The Welsh Historic Environment Service (Cadw), via DataMapWales', 'OGL v3'],
    ['Northern Ireland heritage', 'Department for Communities, Historic Environment Division', 'OGL v3'],
    ['Guernsey heritage', 'States of Guernsey Development & Planning Authority', 'gov.gg terms: research and private use'],
    ['Ireland heritage', 'National Inventory of Architectural Heritage; National Monuments Service (Department of Housing, Local Government and Heritage)', 'CC BY 4.0'],
    ['Spain heritage', 'Generalitat de Catalunya; Junta de Castilla y León; Gobierno de Aragón; Generalitat Valenciana; Xunta de Galicia; Gobierno de Navarra (IDENA); Junta de Extremadura; IAPH (Junta de Andalucía)', 'Per region: CC BY / CC BY-SA / free use with credit'],
    ['Portugal heritage', 'Património Cultural, I.P., Atlas do Património Classificado e em Vias de Classificação', 'CC BY-NC 4.0'],
    ['Hong Kong heritage', 'Antiquities and Monuments Office, via the Common Spatial Data Infrastructure (CSDI) Portal', 'DATA.GOV.HK terms'],
    ['Local-language names (some UNESCO sites, biosphere reserves, geoparks, Québec federal sites)', 'Wikidata', 'CC0'],
    ['Passenger rail lines and services', 'OpenStreetMap route relations and tracks', '© OpenStreetMap contributors, ODbL'],
    ['Stops & sights details', 'OpenStreetMap tags (heights, lights, hill lists, facilities); Wikidata facts (heights, flow, prominence, isolation, inception, descriptions) via QLever (University of Freiburg)', 'ODbL; Wikidata CC0'],
    ['Peak prominence & isolation', 'Computed from the terrain tiles (key col by priority flood, nearest higher ground); tagged values (OSM, Wikidata) preferred', 'Derived'],
    ['Heritage descriptions', 'Wikidata items matched by register ID; English Wikipedia short descriptions; for the most notable sites, 2–3 sentence summaries of their Wikipedia articles written by Claude', 'Wikidata CC0; Wikipedia CC BY-SA 4.0'],
    ['Park & area details', 'Areas computed from the boundaries; OpenStreetMap protected-area tags; Wikidata (inception, visitors, operator)', 'ODbL; Wikidata CC0'],
    ['Colour ramps', 'matplotlib (viridis, magma, plasma, inferno, cividis, cubehelix), seaborn (mako, rocket), Google (turbo), ColorBrewer (Cynthia Brewer), Fabio Crameri\u2019s Scientific colour maps (batlow, hawaii, La Jolla, Oslo, Bamako, Tokyo, Vik, Berlin), cmocean (ice), colorcet (fire, Peter Kovesi)', 'ColorBrewer: Apache 2.0; Crameri, cmocean: MIT; colorcet: CC BY 4.0'],
    ['Rail service frequency', 'Operators\u2019 GTFS timetables (112 feeds via the Mobility Database catalogue and operators: SNCF, Renfe, IDFM, TfI, MTA, MBTA, GO, exo, VIA, Amtrak and others; Great Britain: the Rail Delivery Group timetable as GTFS by Catenary Transit); MTR frequencies from mtr.com.hk (exact for the Airport Express and High Speed Rail, whose timetables are published in full; other lines a lower bound, at least the service hours at the slowest published off-peak headway)', 'Each operator\u2019s open-data terms'],
    ['Ferry routes and terminals', 'OpenStreetMap ferry routes and route relations', '© OpenStreetMap contributors, ODbL'],
    ['Ferry sailings', 'Operators\u2019 published GTFS timetables (listed in each line\u2019s source: MBTA, NYC Ferry, NYC DOT, NY Waterway, STQ, Halifax Transit, BreizhGo, Bacs de Seine, Brittany Ferries, Transtejo Soflusa, Hong Kong Transport Department and others), operators\u2019 timetable pages, and OSM interval tags', 'Each operator\u2019s open-data terms'],
    ['Roadside buildings', 'Overture Maps Foundation buildings (OpenStreetMap, Microsoft and Google footprints)', 'ODbL · CDLA Permissive 2.0'],
  ];
  const d = h('dialog', { class: 'credits' },
    h('div', { class: 'hd' }, h('b', {}, 'Data sources & licences'), h('button', { class: 'x', title: 'Close', onclick: () => d.close() }, '×')),
    h('table', {},
      h('thead', {}, h('tr', {}, h('th', {}, 'What'), h('th', {}, 'Source'), h('th', {}, 'Licence / terms'))),
      h('tbody', {}, ...rows.map(([a, b, c]) => h('tr', {}, h('td', {}, a), h('td', {}, b), h('td', {}, c)))),
    ),
    h('p', { class: 'faint' }, 'For personal use: do not publish or redistribute the built data or a hosted copy of this map.'),
  );
  // Click on the backdrop closes it.
  d.addEventListener('click', (e) => {
    if (e.target === d) d.close();
  });
  return d;
}
