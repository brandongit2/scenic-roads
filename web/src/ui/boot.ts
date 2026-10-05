// Start-up overlay with a step list and progress bar.
export class Boot {
  private ol = document.getElementById('boot-steps')!;
  private bar = document.querySelector<HTMLDivElement>('.boot-bar div')!;
  private items: HTMLLIElement[] = [];

  constructor(private steps: string[]) {
    for (const s of steps) {
      const li = document.createElement('li');
      li.textContent = s;
      this.ol.append(li);
      this.items.push(li);
    }
  }

  at(i: number, detail?: string) {
    this.items.forEach((li, k) => {
      li.className = k < i ? 'done' : k === i ? 'now' : '';
      li.textContent = this.steps[k] + (k === i && detail ? ` — ${detail}` : '');
    });
    this.bar.style.width = `${(i / this.steps.length) * 100}%`;
  }

  /** Fractional progress within step i. */
  sub(i: number, f: number, detail?: string) {
    this.at(i, detail);
    this.bar.style.width = `${((i + Math.max(0, Math.min(1, f))) / this.steps.length) * 100}%`;
  }

  fail(i: number, msg: string) {
    this.items[i].className = 'err';
    this.items[i].textContent = `${this.steps[i]} — ${msg}`;
  }

  /** Step i waits on the person: `msg`, and a field for what it needs (the map's address on a
   * device that hasn't given its key), given to `submit`. */
  ask(i: number, msg: string, placeholder: string, submit: (v: string) => void) {
    this.items[i].className = 'err';
    this.items[i].textContent = `${this.steps[i]} — ${msg}`;
    const form = document.createElement('form');
    form.className = 'boot-ask';
    const input = Object.assign(document.createElement('input'), { type: 'url', placeholder, autocomplete: 'off', spellcheck: false });
    input.setAttribute('autocapitalize', 'off');
    const go = Object.assign(document.createElement('button'), { type: 'submit', textContent: 'Open the map' });
    form.append(input, go);
    form.onsubmit = (e) => {
      e.preventDefault();
      if (input.value.trim()) submit(input.value.trim());
    };
    this.ol.after(form);
    input.focus();
  }

  done() {
    this.at(this.steps.length);
    document.getElementById('boot')!.classList.add('done');
  }
}
