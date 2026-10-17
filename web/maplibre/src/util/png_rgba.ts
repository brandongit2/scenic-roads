// Scenic Roads: raster tiles in Safari decoded to bytes on workers (png_rgba.worker.ts).
//
// Safari uploads an ImageBitmap to a texture by reading its pixels back on the main thread: about
// 2.8 ms for a 512 px water tile on an M1 Pro, 100 of them in a 5 s pan over Yosemite (a third of
// every frame). Bytes upload in a fraction of that. Reading the bitmap back on a worker
// (OffscreenCanvas) waits on the same GPU process and held loads up by tens of seconds, so the PNG
// is decoded here without the browser's decoder. Chrome uploads the bitmap on its GPU side and keeps
// it. The bytes are the pixels the browser would upload: plain PNGs only (8 bits, RGB or RGBA,
// untagged sRGB, which the browser leaves as they are); anything else answers null, for the usual
// decoder.
import {RGBAImage} from './image.ts';

const POOL = 2;
const workers: Worker[] = [];
let next = 0;
const waiting = new Map<number, (img: RGBAImage | null) => void>();

/** The PNG's pixels (premultiplied when asked), or null when it isn't a plain one. */
export function pngToRGBA(buf: ArrayBuffer, premultiply: boolean): Promise<RGBAImage | null> {
    if (!workers.length) {
        for (let i = 0; i < POOL; i++) {
            const w = new Worker(new URL('./png_rgba.worker.ts', import.meta.url), {type: 'module'});
            w.onmessage = (e: MessageEvent<{id: number; r: {width: number; height: number; data: Uint8Array} | null}>) => {
                const {id, r} = e.data;
                waiting.get(id)?.(r ? new RGBAImage({width: r.width, height: r.height}, r.data) : null);
                waiting.delete(id);
            };
            workers.push(w);
        }
    }
    const id = next++;
    return new Promise((res) => {
        waiting.set(id, res);
        // (a copy: the caller may still need the bytes for the usual decoder)
        workers[id % POOL].postMessage({id, buf: buf.slice(0), premultiply});
    });
}
