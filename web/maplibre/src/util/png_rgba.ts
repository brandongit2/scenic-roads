// Scenic Roads: raster tiles in Safari decoded to bytes on workers (png_rgba.worker.ts).
//
// Safari uploads an ImageBitmap to a texture by reading its pixels back on the main thread: about
// 2.8 ms for a 512 px water tile on an M1 Pro, 100 of them in a 5 s pan over Yosemite (a third of
// every frame). Bytes upload in a fraction of that. Reading the bitmap back on a worker
// (OffscreenCanvas) waits on the same GPU process and held loads up by tens of seconds, so the PNG
// is decoded here without the browser's decoder. Chrome uploads the bitmap on its GPU side and keeps
// it. The bytes are the pixels the browser would upload: plain PNGs only (8 bits, RGB or RGBA,
// untagged sRGB, which the browser leaves as they are); anything else answers null, for the usual
// decoder, as does a worker that fails or takes over DECODE_TIMEOUT_MS.
import {RGBAImage} from './image.ts';

const POOL = 2;
const DECODE_TIMEOUT_MS = 10000;
type Reply = {id: number; r: {width: number; height: number; data: Uint8Array} | null};
const workers: Worker[] = [];
let next = 0;
/** Each request waiting: its answer, and the worker it went to. */
const waiting = new Map<number, {res: (img: RGBAImage | null) => void; w: number; timer: ReturnType<typeof setTimeout>}>();

const answer = (id: number, img: RGBAImage | null) => {
    const x = waiting.get(id);
    if (!x) return;
    clearTimeout(x.timer);
    waiting.delete(id);
    x.res(img);
};

/** A worker that failed: everything sent to it answered null, and a new one in its place. */
const failed = (w: number) => {
    workers[w].terminate();
    for (const [id, x] of waiting) if (x.w === w) answer(id, null);
    workers[w] = start(w);
};

function start(w: number): Worker {
    const worker = new Worker(new URL('./png_rgba.worker.ts', import.meta.url), {type: 'module'});
    worker.onmessage = (e: MessageEvent<Reply>) => {
        const {id, r} = e.data;
        answer(id, r ? new RGBAImage({width: r.width, height: r.height}, r.data) : null);
    };
    worker.onerror = () => failed(w);
    worker.onmessageerror = () => failed(w);
    return worker;
}

/** Whether the bytes start as a PNG does. */
const isPNG = (buf: ArrayBuffer) => {
    if (buf.byteLength < 8) return false;
    const b = new Uint8Array(buf, 0, 8);
    return b[0] === 0x89 && b[1] === 0x50 && b[2] === 0x4e && b[3] === 0x47 && b[4] === 0x0d && b[5] === 0x0a && b[6] === 0x1a && b[7] === 0x0a;
};

/** The PNG's pixels (premultiplied when asked), or null when it isn't a plain one. */
export function pngToRGBA(buf: ArrayBuffer, premultiply: boolean): Promise<RGBAImage | null> {
    if (!isPNG(buf)) return Promise.resolve(null);
    if (!workers.length) for (let i = 0; i < POOL; i++) workers.push(start(i));
    const id = next++, w = id % POOL;
    return new Promise((res) => {
        waiting.set(id, {res, w, timer: setTimeout(() => answer(id, null), DECODE_TIMEOUT_MS)});
        // (a copy: the bytes stay here for the usual decoder should this answer null)
        workers[w].postMessage({id, buf: buf.slice(0), premultiply});
    });
}
