// On-screen view controls: tilt steps and the tilt (click: flatten), rotate steps and the compass
// (reset north / tilt), zoom. 3D terrain itself: Layers → Terrain.
import type { Map as MLMap } from 'maplibre-gl';
import type { CameraControls } from '../trackpad';
import { h } from './dom';

export class NavControls {
  private compass: HTMLButtonElement;
  private needle: HTMLSpanElement;
  private pitchOut: HTMLButtonElement;

  constructor(root: HTMLElement, private map: MLMap, private cam: CameraControls) {
    this.needle = h('span', { class: 'needle' });
    this.compass = h('button', { title: 'Reset north (click again to flatten)', onclick: () => this.resetNorth() }, this.needle);
    this.pitchOut = h('button', { class: 'pitch', title: 'Tilt · click to flatten (0°)', onclick: () => cam.orbitBy(0, -map.getPitch()) });
    // Tilt and rotate about the ground at the view centre, zoom toward it (a plain easeTo turns
    // about the sea-level pivot, which on the globe can be far below the ground).
    const tilt = (d: number) => cam.orbitBy(0, Math.max(0, Math.min(map.getMaxPitch(), map.getPitch() + d)) - map.getPitch());
    root.append(
      h('div', { class: 'grp' },
        h('button', { class: 'step', title: 'Tilt up (⌥ + two-finger drag ↕, right-drag, Ctrl-drag)', onclick: () => tilt(15) }, '⤒'),
        this.pitchOut,
        h('button', { class: 'step', title: 'Tilt down', onclick: () => tilt(-15) }, '⤓'),
      ),
      h('div', { class: 'grp' },
        h('button', { class: 'step', title: 'Rotate left (⌥ + two-finger drag ↔)', onclick: () => cam.orbitBy(-22.5, 0) }, '⟲'),
        this.compass,
        h('button', { class: 'step', title: 'Rotate right', onclick: () => cam.orbitBy(22.5, 0) }, '⟳'),
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

  update() {
    this.needle.style.transform = `rotate(${-this.map.getBearing()}deg)`;
    this.pitchOut.textContent = `${Math.round(this.map.getPitch())}°`;
  }
}
