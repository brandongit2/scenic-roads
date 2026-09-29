// On-screen view controls: 2D/3D, compass (reset north / tilt), tilt and rotate steps, zoom.
import type { Map as MLMap } from 'maplibre-gl';
import type { Store } from '../state';
import type { CameraControls } from '../trackpad';
import { h } from './dom';

export class NavControls {
  private compass: HTMLButtonElement;
  private needle: HTMLSpanElement;
  private d3: HTMLButtonElement;
  private pitchOut: HTMLSpanElement;

  constructor(root: HTMLElement, private map: MLMap, private store: Store, private cam: CameraControls) {
    this.needle = h('span', { class: 'needle' });
    this.compass = h('button', { title: 'Reset north (click again to flatten)', onclick: () => this.resetNorth() }, this.needle);
    this.d3 = h('button', { class: 'txt', title: '3D terrain on/off', onclick: () => this.toggle3d() }, '3D');
    this.pitchOut = h('span', { class: 'pitch' });
    // Tilt and rotate about the ground at the view centre, zoom toward it (a plain easeTo turns
    // about the sea-level pivot, which on the globe can be far below the ground).
    const tilt = (d: number) => cam.orbitBy(0, Math.max(0, Math.min(map.getMaxPitch(), map.getPitch() + d)) - map.getPitch());
    root.append(
      this.d3,
      h('div', { class: 'grp' },
        h('button', { title: 'Tilt up (⌥ + two-finger drag ↕, right-drag, Ctrl-drag)', onclick: () => tilt(15) }, '⤒'),
        this.pitchOut,
        h('button', { title: 'Tilt down', onclick: () => tilt(-15) }, '⤓'),
      ),
      h('div', { class: 'grp' },
        h('button', { title: 'Rotate left (⌥ + two-finger drag ↔)', onclick: () => cam.orbitBy(-22.5, 0) }, '⟲'),
        this.compass,
        h('button', { title: 'Rotate right', onclick: () => cam.orbitBy(22.5, 0) }, '⟳'),
      ),
      h('div', { class: 'grp' },
        h('button', { title: 'Zoom in', onclick: () => cam.zoomBy(1) }, '+'),
        h('button', { title: 'Zoom out', onclick: () => cam.zoomBy(-1) }, '−'),
      ),
    );
    map.on('rotate', () => this.update());
    map.on('pitch', () => this.update());
    this.update();
  }

  private resetNorth() {
    const m = this.map;
    const b = m.getBearing();
    if (Math.abs(b) > 0.5) this.cam.orbitBy(-(((b % 360) + 540) % 360 - 180), 0);
    else this.cam.orbitBy(0, -m.getPitch());
  }

  private toggle3d() {
    const on = !this.store.s.terrain.on;
    this.store.terrain({ on });
    if (on && this.map.getPitch() < 20) this.cam.orbitBy(0, 55 - this.map.getPitch());
    if (!on) this.cam.orbitBy(0, -this.map.getPitch());
  }

  update() {
    this.needle.style.transform = `rotate(${-this.map.getBearing()}deg)`;
    this.pitchOut.textContent = `${Math.round(this.map.getPitch())}°`;
    this.d3.classList.toggle('on', this.store.s.terrain.on);
  }
}
