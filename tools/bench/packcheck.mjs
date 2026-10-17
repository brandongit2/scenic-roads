// Checks web/src/packfloat.ts against Chrome's own conversion: the same floats uploaded to the GPU as
// 32-bit floats (converted by the browser) and packed by packfloat (uploaded as they are), read back
// texel by texel, must match bit for bit. R11F_G11F_B10F over every value the far field's worker can
// make (bldfield.worker.ts: v, w and f from each byte of its tiles); R16F over every binade it reaches,
// with the mantissas at each rounding boundary, of either sign, and random values to a million.
//
//   node tools/bench/packcheck.mjs [--port 9333]    (a Chrome as tools/bench/bench.mjs starts one)
import fs from 'node:fs';
import { stripTypeScriptTypes } from 'node:module';

const port = Number(process.argv.includes('--port') ? process.argv[process.argv.indexOf('--port') + 1] : 9333);
const src = stripTypeScriptTypes(fs.readFileSync(new URL('../../web/src/packfloat.ts', import.meta.url), 'utf8')).replace(/^export /gm, '');

const page = `${src}
const W_LOG = Math.log2(4097), LO = [0, 8, 20, 40, 100], HI = [8, 20, 40, 100, 1000];
// (the worker's rounding, bldfield.worker.ts)
const toMantissa = (x, bits) => { if (!(x > 0)) return 0; const e = Math.floor(Math.log2(x)); const q = 2 ** (e - bits); return Math.round(x / q) * q; };
const fs_ = [], ws = [], vs = [];
for (let R = 0; R < 256; R++) fs_.push(toMantissa((R / 255) ** 2, 5));
for (let G = 0; G < 256; G++) ws.push(toMantissa(2 ** ((G / 255) * W_LOG) - 1, 6));
for (let c = 0; c < 5; c++) for (let R = 0; R < 256; R++) for (let B = 0; B < 256; B++) vs.push(toMantissa((R / 255) ** 2 * (LO[c] + (B / 255) * (HI[c] - LO[c])), 6));
const T = 512, rows = Math.ceil(vs.length / T), n = T * rows;
const rgb = new Float32Array(n * 3);
for (let i = 0; i < n; i++) { rgb[i * 3] = vs[i % vs.length]; rgb[i * 3 + 1] = ws[i % 256]; rgb[i * 3 + 2] = fs_[(i * 7) % 256]; }
// Half floats: the edges, then a spread of magnitudes and signs (the roof ground: ± a few km × a share).
const hv = [0, -0, 1, -1, 65504, 65519.99, 65520, 70000, -70000, 2 ** -14, 2 ** -24, 2 ** -25, 2 ** -25 * 1.0001, 3 * 2 ** -25, 2 ** -26, 1e-10, -1e-10, 0.1, 0.2, 0.3, 1 / 3, 2049, 2051, 2053, 4097, 1e-5, 6.1e-5, 5.96e-8];
let seed = 1;
const rnd = () => ((seed = (seed * 1103515245 + 12345) % 2147483648) / 2147483648);
// (every exponent from 2^-40 to 2^24 with mantissas at and around each rounding boundary, both signs)
const bits = new Uint32Array(1), asF = new Float32Array(bits.buffer);
for (let ex = 87; ex <= 151; ex++) for (let k = 0; k < 1024; k++) for (const lo of [0, 1, 0xfff, 0x1000, 0x1001, 0x1fff]) for (const sg of [0, 0x80000000]) {
  bits[0] = (sg | (ex << 23) | (((k << 13) | lo) & 0x7fffff)) >>> 0;
  hv.push(asF[0]);
}
for (let i = 0; i < T * 16; i++) hv.push((i - T * 8) * 2 ** -22); // (around zero, subnormal ties)
while (hv.length % T || hv.length < T * 2048) { const m = 2 ** (rnd() * 46 - 30); hv.push((rnd() < 0.3 ? -1 : 1) * m * (1 + rnd())); }
const half = new Float32Array(hv);

const gl = new OffscreenCanvas(4, 4).getContext('webgl2');
if (!gl.getExtension('EXT_color_buffer_float')) return { error: 'no EXT_color_buffer_float' };
const sh = (type, s) => { const x = gl.createShader(type); gl.shaderSource(x, s); gl.compileShader(x); if (!gl.getShaderParameter(x, gl.COMPILE_STATUS)) throw new Error(gl.getShaderInfoLog(x)); return x; };
const prog = gl.createProgram();
gl.attachShader(prog, sh(gl.VERTEX_SHADER, '#version 300 es\\nvoid main(){vec2 p=vec2(gl_VertexID==1?3.:-1.,gl_VertexID==2?3.:-1.);gl_Position=vec4(p,0,1);}'));
gl.attachShader(prog, sh(gl.FRAGMENT_SHADER, '#version 300 es\\nprecision highp float;uniform highp sampler2D t;out vec4 o;void main(){o=texelFetch(t,ivec2(gl_FragCoord.xy),0);}'));
gl.linkProgram(prog);
gl.useProgram(prog);
const read = (fmt, w, h, type, data) => {
  const tx = gl.createTexture();
  gl.bindTexture(gl.TEXTURE_2D, tx);
  gl.pixelStorei(gl.UNPACK_ALIGNMENT, 1);
  gl.texImage2D(gl.TEXTURE_2D, 0, fmt, w, h, 0, fmt === gl.R16F ? gl.RED : gl.RGB, type, data);
  gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.NEAREST);
  gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.NEAREST);
  const rb = gl.createTexture();
  gl.bindTexture(gl.TEXTURE_2D, rb);
  gl.texStorage2D(gl.TEXTURE_2D, 1, gl.RGBA32F, w, h);
  const fb = gl.createFramebuffer();
  gl.bindFramebuffer(gl.FRAMEBUFFER, fb);
  gl.framebufferTexture2D(gl.FRAMEBUFFER, gl.COLOR_ATTACHMENT0, gl.TEXTURE_2D, rb, 0);
  gl.viewport(0, 0, w, h);
  gl.bindTexture(gl.TEXTURE_2D, tx);
  gl.drawArrays(gl.TRIANGLES, 0, 3);
  const out = new Float32Array(w * h * 4);
  gl.readPixels(0, 0, w, h, gl.RGBA, gl.FLOAT, out);
  gl.deleteTexture(tx); gl.deleteTexture(rb); gl.deleteFramebuffer(fb);
  return out;
};
const cmp = (a, b, comps) => {
  const ua = new Uint32Array(a.buffer), ub = new Uint32Array(b.buffer);
  let bad = 0; const ex = [];
  for (let i = 0; i < a.length; i++) if (i % 4 < comps && ua[i] !== ub[i]) { bad++; if (ex.length < 8) ex.push([i, a[i], b[i]]); }
  return { compared: (a.length / 4) * comps, bad, ex };
};
const r11 = cmp(read(gl.R11F_G11F_B10F, T, rows, gl.FLOAT, rgb), read(gl.R11F_G11F_B10F, T, rows, gl.UNSIGNED_INT_10F_11F_11F_REV, packR11G11B10(rgb)), 3);
const h16 = cmp(read(gl.R16F, T, half.length / T, gl.FLOAT, half), read(gl.R16F, T, half.length / T, gl.HALF_FLOAT, packHalf(half)), 1);
return { renderer: gl.getParameter(gl.RENDERER), r11, h16 };`;

const target = await (await fetch(`http://127.0.0.1:${port}/json/new?about:blank`, { method: 'PUT' })).json();
const ws = new WebSocket(target.webSocketDebuggerUrl);
await new Promise((r) => ws.addEventListener('open', r));
const keep = setInterval(() => {}, 1000);
try {
  const r = await new Promise((res) => {
    ws.addEventListener('message', (e) => { const m = JSON.parse(e.data); if (m.id === 1) res(m); });
    ws.send(JSON.stringify({ id: 1, method: 'Runtime.evaluate', params: { expression: `(() => { ${page} })()`, returnByValue: true } }));
  });
  console.log(JSON.stringify(r.result?.result?.value ?? r.result?.exceptionDetails ?? r.error, null, 1));
  const v = r.result?.result?.value;
  process.exitCode = v && !v.error && v.r11.bad === 0 && v.h16.bad === 0 ? 0 : 1;
} finally {
  clearInterval(keep);
  ws.close();
  await fetch(`http://127.0.0.1:${port}/json/close/${target.id}`).catch(() => {});
}
