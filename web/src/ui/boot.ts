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

  done() {
    this.at(this.steps.length);
    document.getElementById('boot')!.classList.add('done');
  }
}
