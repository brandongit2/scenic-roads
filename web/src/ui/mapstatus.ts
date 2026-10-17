// The status bar's connection items: the NAS or the map server when it can't be reached (map data
// not mirrored on this Mac may be missing), and new map data or a new app that wants a reload.
import type { CatalogWatch } from '../catalog';
import { h } from './dom';

export class MapStatus {
  private nas = h('span', { class: 'bs-nas' });
  private reload = h('button', { class: 'bs-reload', type: 'button', onclick: () => location.reload() });

  constructor(root: HTMLElement, private watch: CatalogWatch) {
    root.classList.add('build');
    root.append(this.reload, this.nas);
    this.render();
    watch.on(() => this.render());
  }

  private render() {
    const w = this.watch, c = w.status;
    this.reload.hidden = !w.reload;
    this.reload.textContent = `${w.reload} · Reload`;
    this.reload.title = `${w.reload}: part of it shows only after a reload`;
    // The server itself, else the NAS behind it.
    const down = w.unreachable || (c !== null && !c.online);
    this.nas.hidden = !down;
    this.nas.replaceChildren(h('i', { class: 'dot' }), h('span', {}, w.unreachable ? 'Map server not answering' : 'NAS offline'));
    this.nas.title = w.unreachable ? 'The map server isn’t answering (restarting?)' : 'The NAS can’t be reached: map data not mirrored on this Mac may be missing';
  }
}
