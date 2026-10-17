// Scenic Roads: a PNG's pixels as bytes, decoded on a worker without the browser's image decoder
// (png_rgba.ts). Only the plain kind: 8 bits a channel, RGB or RGBA, not interlaced, and no colour
// profile, gamma or transparency chunk (whose colours the browser would change); anything else
// answers null and is decoded the usual way.
const paeth = (a: number, b: number, c: number): number => {
    const p = a + b - c, pa = Math.abs(p - a), pb = Math.abs(p - b), pc = Math.abs(p - c);
    return pa <= pb && pa <= pc ? a : pb <= pc ? b : c;
};

async function decode(buf: ArrayBuffer, premultiply: boolean): Promise<{width: number; height: number; data: Uint8Array} | null> {
    const b = new Uint8Array(buf), v = new DataView(buf);
    if (b.length < 33 || v.getUint32(0) !== 0x89504e47 || v.getUint32(4) !== 0x0d0a1a0a) return null;
    let width = 0, height = 0, ch = 0;
    const idat: Uint8Array[] = [];
    for (let i = 8; i + 8 <= b.length;) {
        const n = v.getUint32(i), type = String.fromCharCode(b[i + 4], b[i + 5], b[i + 6], b[i + 7]);
        const body = b.subarray(i + 8, i + 8 + n);
        if (type === 'IHDR') {
            width = v.getUint32(i + 8);
            height = v.getUint32(i + 12);
            const depth = body[8], colour = body[9], interlace = body[12];
            if (depth !== 8 || interlace !== 0 || (colour !== 6 && colour !== 2)) return null;
            ch = colour === 6 ? 4 : 3;
        } else if (type === 'IDAT') idat.push(body);
        else if (type === 'iCCP' || type === 'gAMA' || type === 'cHRM' || type === 'sRGB' || type === 'tRNS' || type === 'PLTE') return null;
        else if (type === 'IEND') break;
        i += 12 + n;
    }
    if (!width || !idat.length) return null;
    const raw = new Uint8Array(await new Response(new Blob(idat as BlobPart[]).stream().pipeThrough(new DecompressionStream('deflate'))).arrayBuffer());
    const stride = width * ch;
    if (raw.length < (stride + 1) * height) return null;
    const out = new Uint8Array(width * height * 4);
    let prev = new Uint8Array(stride), cur = new Uint8Array(stride);
    for (let y = 0; y < height; y++) {
        const f = raw[y * (stride + 1)], row = raw.subarray(y * (stride + 1) + 1, (y + 1) * (stride + 1));
        for (let x = 0; x < stride; x++) {
            const a = x >= ch ? cur[x - ch] : 0, up = prev[x], c = x >= ch ? prev[x - ch] : 0;
            const r = row[x];
            cur[x] = f === 0 ? r : f === 1 ? r + a : f === 2 ? r + up : f === 3 ? r + ((a + up) >> 1) : r + paeth(a, up, c);
        }
        const o = y * width * 4;
        if (ch === 4) {
            out.set(cur, o);
            if (premultiply) {
                for (let x = o; x < o + width * 4; x += 4) {
                    const al = out[x + 3];
                    if (al === 255) continue;
                    // (as the browser premultiplies an upload: rounded to the nearest)
                    out[x] = Math.round((out[x] * al) / 255);
                    out[x + 1] = Math.round((out[x + 1] * al) / 255);
                    out[x + 2] = Math.round((out[x + 2] * al) / 255);
                }
            }
        } else {
            for (let x = 0, k = o; x < stride; x += 3, k += 4) {
                out[k] = cur[x];
                out[k + 1] = cur[x + 1];
                out[k + 2] = cur[x + 2];
                out[k + 3] = 255;
            }
        }
        [prev, cur] = [cur, prev];
    }
    return {width, height, data: out};
}

self.onmessage = async (e: MessageEvent<{id: number; buf: ArrayBuffer; premultiply: boolean}>) => {
    const {id, buf, premultiply} = e.data;
    let r: Awaited<ReturnType<typeof decode>> = null;
    try {
        r = await decode(buf, premultiply);
    } catch {
        r = null;
    }
    (self as unknown as Worker).postMessage({id, r}, r ? [r.data.buffer] : []);
};
