// On-screen view controls: 2D/3D, compass (reset north / tilt), tilt and rotate steps, zoom.
import type { Map as MLMap } from 'maplibre-gl';
import type { Store } from '../state';
import { h } from './dom';

export class NavControls {
  private compass: HTMLButtonElement;
  private needle: HTMLSpanElement;
  private d3: HTMLButtonElement;
  private pitchOut: HTMLSpanElement;

  constructor(root: HTMLElement, private map: MLMap, private store: Store) {
    this.needle = h('span', { class: 'needle' });
    this.compass = h('button', { title: 'Reset north (click again to flatten)', onclick: () => this.resetNorth() }, this.needle);
    this.d3 = h('button', { class: 'txt', title: '3D terrain on/off', onclick: () => this.toggle3d() }, '3D');
    this.pitchOut = h('span', { class: 'pitch' });
    const ease = (o: { bearing?: number; pitch?: number; zoom?: number }) => map.easeTo({ ...o, duration: 350 });
    root.append(
      this.d3,
      h('div', { class: 'grp' },
        h('button', { title: 'Tilt up (⌥ + two-finger drag ↕, right-drag, Ctrl-drag)', onclick: () => ease({ pitch: Math.min(map.getMaxPitch(), map.getPitch() + 15) }) }, '⤒'),
        this.pitchOut,
        h('button', { title: 'Tilt down', onclick: () => ease({ pitch: Math.max(0, map.getPitch() - 15) }) }, '⤓'),
      ),
      h('div', { class: 'grp' },
        h('button', { title: 'Rotate left (⌥ + two-finger drag ↔)', onclick: () => ease({ bearing: map.getBearing() - 22.5 }) }, '⟲'),
        this.compass,
        h('button', { title: 'Rotate right', onclick: () => ease({ bearing: map.getBearing() + 22.5 }) }, '⟳'),
      ),
      h('div', { class: 'grp' },
        h('button', { title: 'Zoom in', onclick: () => ease({ zoom: map.getZoom() + 1 }) }, '+'),
        h('button', { title: 'Zoom out', onclick: () => ease({ zoom: map.getZoom() - 1 }) }, '−'),
      ),
    );
    map.on('rotate', () => this.update());
    map.on('pitch', () => this.update());
    this.update();
  }

  private resetNorth() {
    const m = this.map;
    if (Math.abs(m.getBearing()) > 0.5) m.easeTo({ bearing: 0, duration: 450 });
    else m.easeTo({ pitch: 0, duration: 450 });
  }

  private toggle3d() {
    const on = !this.store.s.terrain.on;
    this.store.terrain({ on });
    if (on && this.map.getPitch() < 20) this.map.easeTo({ pitch: 55, duration: 700 });
    if (!on) this.map.easeTo({ pitch: 0, duration: 500 });
  }

  update() {
    this.needle.style.transform = `rotate(${-this.map.getBearing()}deg)`;
    this.pitchOut.textContent = `${Math.round(this.map.getPitch())}°`;
    this.d3.classList.toggle('on', this.store.s.terrain.on);
  }
}
